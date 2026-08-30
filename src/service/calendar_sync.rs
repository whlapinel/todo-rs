//! Google Calendar (iCal) import — see docs/google-calendar-import-plan.md, Stages 3 & 7.
//!
//! Fetches a project admin's private iCal feed, parses it, and diffs it against the
//! `Item` rows already imported from that `CalendarSubscription` (keyed on
//! `google_event_id`). Deliberately writes through `ItemRepo` directly rather than
//! `service::project_items`'s `create_project_item`/`update_project_item` — those
//! enforce per-request membership/role checks that don't apply to a background sync
//! process, and Stage 4's read-only guard on those functions would otherwise reject
//! the sync's own writes to an already-imported item.
//!
//! All-day (`VALUE=DATE`) `DTSTART`/`DTEND`/`RECURRENCE-ID`/`EXDATE` values carry no
//! timezone of their own per RFC 5545, but this app still has to pick *some* UTC instant
//! to store them as — and, unlike a manually-created all-day item (whose creator's
//! browser bakes its own tz offset in via `combine_local_to_utc`), a background sync has
//! no request-scoped viewer to borrow an offset from. `sync_subscription` resolves the
//! subscription creator's `User::timezone` (falling back to UTC if unset/unparseable)
//! and threads it through `parse_ical` as `all_day_tz`, used only for all-day dates —
//! `resolve_all_day_instant` localizes local midnight (or end-of-day, for an inclusive
//! multi-day end) in that zone before converting to UTC, matching the display layer's
//! `to_local` (`src/web_ui/mod.rs`) so the imported date renders as the correct calendar
//! day for that user. Fixed 2026-08-22 per issue #1 in docs/issues_and_features.md — an
//! all-day event imported with the old UTC-midnight-always parsing displayed a day early
//! for any viewer behind UTC. A genuinely different viewer's timezone can still disagree
//! with the subscription creator's, but that's the same, already-accepted limitation the
//! rest of this app's date-only values carry (see root CLAUDE.md's "Known design
//! tension" note in the Scheduled start/end section) — not a new gap this fix opens.
//!
//! RRULE-bearing (recurring) master events are expanded into one synthetic pseudo-event
//! per occurrence (`expand_master_event`, Stage 7) within a tight, sliding window
//! (`RECURRING_WINDOW_PAST_DAYS`/`RECURRING_WINDOW_FUTURE_DAYS`) — each occurrence gets a
//! deterministic `"{uid}::{occurrence_start_rfc3339}"` id derived from its own
//! *unmodified* RRULE-computed timestamp, so the existing `google_event_id`-keyed diff in
//! `run_diff` handles create/update/delete for expanded occurrences with no changes of its
//! own. `RECURRENCE-ID`-bearing override VEVENTs (RFC 5545's "this one occurrence was
//! moved/renamed/cancelled" mechanism) are matched to their slot by that same original
//! timestamp and swapped in before the diff runs; a `STATUS:CANCELLED` override removes
//! its slot entirely, the same as an `EXDATE`. `SyncSummary::skipped_recurring` counts
//! master events whose `RRULE` failed to parse/expand (not, as in Stage 3, every
//! recurring event unconditionally).

use crate::domain::calendar_subscription::CalendarSubscription;
use crate::domain::item::{EventItem, Item, ItemType, Recurrence, Schedule};
use crate::storage::sqlite::{CalendarSubscriptionRepo, ItemRepo, RepoError, UserRepo};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use rrule::{RRule, RRuleError, Tz as RTz, Unvalidated};
use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::str::FromStr;

/// Non-recurring events older than this (relative to "now" at sync time) are treated
/// as no longer importable — an already-imported one ages out via the ordinary
/// diff-delete path, a not-yet-imported one is simply skipped. Keeps the `items` table
/// from growing unboundedly off a feed containing years of history.
const IMPORT_WINDOW_PAST_DAYS: i64 = 30;
/// Non-recurring events further in the future than this are not imported. Generous on
/// purpose (a year out) since these are real, one-shot rows, not per-occurrence
/// expansions — unlike Stage 7's much tighter recurring-occurrence window.
const IMPORT_WINDOW_FUTURE_DAYS: i64 = 365;

/// Recurring-event occurrences older than this (relative to "now" at sync time) stop
/// being expanded/tracked. Much tighter than the non-recurring window above since a
/// recurring series generates one row *per occurrence* — an aged-out occurrence simply
/// falls out of `run_diff`'s freshly-parsed set and is deleted via the ordinary
/// diff-delete path, exactly like any other no-longer-present event (expected, routine
/// churn for a recurring series, not a sign of anything wrong).
const RECURRING_WINDOW_PAST_DAYS: i64 = 7;
/// Recurring-event occurrences further in the future than this are not expanded. On the
/// next sync the window has slid forward, so newly-in-range future occurrences appear —
/// no special "window sliding" logic needed beyond re-running the same fixed-width query
/// relative to the new "now".
const RECURRING_WINDOW_FUTURE_DAYS: i64 = 180;
/// Upper bound passed to `RRuleSet::all` — the widest plausible recurring window
/// (187 days) expanded at a daily frequency is ~187 occurrences, so this is a generous
/// safety cap against a degenerate/malformed RRULE (e.g. sub-hourly), not a limit
/// expected to bind in practice. `RRuleResult::limited` is logged if it ever does.
const RECURRING_MAX_OCCURRENCES: u16 = 500;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedIcalEvent {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub all_day: bool,
    /// `true` if the VEVENT carries an `RRULE` — this is the *master* event of a
    /// recurring series, expanded by `expand_master_event` rather than imported as-is.
    pub has_rrule: bool,
    /// The raw `RRULE` value string, present only when `has_rrule`. Stage 7.
    pub rrule: Option<String>,
    /// `EXDATE` instants (UTC), present only when `has_rrule` — occurrences to exclude
    /// from this master's expansion. Stage 7.
    pub exdates: Vec<DateTime<Utc>>,
    /// The zone `DTSTART` (and so `RRULE`/`EXDATE`) was resolved in. Recurrence must
    /// expand against local wall-clock time, not a fixed UTC offset, so e.g. a weekly
    /// 9am `America/New_York` event stays 9am local across a DST transition rather than
    /// drifting by an hour in UTC. Stage 7.
    pub tz: Tz,
    /// `Some(original_occurrence_start)` when this VEVENT is a `RECURRENCE-ID`-bearing
    /// override of one occurrence of a recurring series sharing the same `uid` — the
    /// value is that occurrence's *original*, unmodified start time, which is how it's
    /// matched back to the expanded slot it overrides. Stage 7.
    pub recurrence_id: Option<DateTime<Utc>>,
    /// `true` for a `RECURRENCE-ID`-bearing override whose own `STATUS` is `CANCELLED` —
    /// meaning this one occurrence should be dropped from its series, like an implicit
    /// `EXDATE`. Never `true` on anything else: a `STATUS:CANCELLED` VEVENT with no
    /// `RECURRENCE-ID` is filtered out of `parse_ical`'s output entirely (unchanged from
    /// Stage 3 — see `parse_vevent`), since only an override's cancellation needs to
    /// survive as data (to know *which* slot to drop). Stage 7.
    pub cancelled: bool,
}

#[derive(Debug)]
pub enum CalendarSyncError {
    /// Fetching or reading the iCal feed itself failed (non-2xx, timeout, network
    /// error, etc.) — recorded on the subscription via `last_sync_error` without
    /// touching any previously-imported items (a transient outage shouldn't delete
    /// anyone's calendar).
    Fetch(String),
    /// A storage-layer call failed mid-sync.
    Repo(String),
}

impl std::fmt::Display for CalendarSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalendarSyncError::Fetch(msg) => write!(f, "failed to fetch calendar feed: {msg}"),
            CalendarSyncError::Repo(msg) => write!(f, "calendar sync storage error: {msg}"),
        }
    }
}

impl std::error::Error for CalendarSyncError {}

fn repo_err(e: RepoError) -> CalendarSyncError {
    CalendarSyncError::Repo(format!("{e:?}"))
}

pub async fn fetch_ical(url: &str) -> Result<String, CalendarSyncError> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| CalendarSyncError::Fetch(e.to_string()))?;
    if !response.status().is_success() {
        return Err(CalendarSyncError::Fetch(format!(
            "unexpected status {}",
            response.status()
        )));
    }
    response
        .text()
        .await
        .map_err(|e| CalendarSyncError::Fetch(e.to_string()))
}

/// Parses raw iCal text into every non-`CANCELLED` `VEVENT` it contains, RRULE-bearing
/// ones included (flagged via `has_rrule`, not expanded — see module docs). Applies no
/// time-window filtering of its own; that's `sync_subscription`'s job, so this function
/// stays pure and trivially testable against fixture text.
///
/// `all_day_tz` is used only to resolve `VALUE=DATE` (all-day) properties — see module
/// docs. It has no bearing on timed (`DTSTART`/`DTEND` with a real time-of-day)
/// properties, which always resolve via their own `TZID` param regardless.
pub fn parse_ical(content: &str, all_day_tz: Tz) -> Vec<ParsedIcalEvent> {
    let mut events = Vec::new();
    for calendar in ical::IcalParser::new(BufReader::new(content.as_bytes())).flatten() {
        for vevent in &calendar.events {
            if let Some(parsed) = parse_vevent(vevent, all_day_tz) {
                events.push(parsed);
            }
        }
    }
    events
}

fn parse_vevent(
    vevent: &ical::parser::ical::component::IcalEvent,
    all_day_tz: Tz,
) -> Option<ParsedIcalEvent> {
    let find_prop = |name: &str| vevent.properties.iter().find(|p| p.name == name);
    let prop_value = |name: &str| find_prop(name).and_then(|p| p.value.as_deref());

    let recurrence_id = find_prop("RECURRENCE-ID")
        .and_then(|p| parse_dt_property(p, all_day_tz))
        .map(|(dt, _, _, _)| dt);
    let is_cancelled = prop_value("STATUS") == Some("CANCELLED");
    // A plain (non-override) cancelled event is dropped entirely, same as Stage 3 — it
    // was never imported and shouldn't be now. A cancelled *override* (RECURRENCE-ID
    // set) has to survive as data, though: `sync_subscription` needs it to know which
    // occurrence of the series to drop, so it's parsed fully with `cancelled: true`
    // rather than filtered out here.
    if is_cancelled && recurrence_id.is_none() {
        return None;
    }

    let uid = prop_value("UID")?.to_string();
    let summary = prop_value("SUMMARY").unwrap_or("(no title)").to_string();
    let description = prop_value("DESCRIPTION")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let has_rrule = find_prop("RRULE").is_some();
    let rrule = prop_value("RRULE").map(str::to_string);

    let (start, all_day, tz, start_date) = parse_dt_property(find_prop("DTSTART")?, all_day_tz)?;

    let parsed_end = find_prop("DTEND").and_then(|p| parse_dt_property(p, all_day_tz));
    let end = match (all_day, parsed_end, start_date) {
        // RFC 5545: an all-day DTEND is *exclusive* — the day after the last actual
        // day. Only surface it as a visible end date when it represents a real,
        // multi-day span (more than one calendar day past DTSTART); a single-day
        // all-day event (no DTEND, or an explicit DTEND of DTSTART + 1 day) shows only
        // a start date, per issue #1 in docs/issues_and_features.md. When it is a real
        // span, convert the exclusive end date to an inclusive one (the actual last
        // day) at end-of-day in the same zone DTSTART resolved in — matching how
        // `scheduled_end_date` defaults to end-of-day everywhere else in this app (see
        // root CLAUDE.md's Scheduled start/end section).
        (true, Some((_, _, _, Some(end_date))), Some(start_date))
            if end_date > start_date + Duration::days(1) =>
        {
            Some(resolve_all_day_instant(
                end_date - Duration::days(1),
                tz,
                true,
            ))
        }
        (true, _, _) => None,
        (false, Some((dt, _, _, _)), _) => Some(dt),
        (false, None, _) => None,
    };

    let exdates = vevent
        .properties
        .iter()
        .filter(|p| p.name == "EXDATE")
        .flat_map(|p| parse_exdate_values(p, all_day_tz))
        .collect();

    Some(ParsedIcalEvent {
        uid,
        summary,
        description,
        start,
        end,
        all_day,
        has_rrule,
        rrule,
        exdates,
        tz,
        recurrence_id,
        cancelled: is_cancelled,
    })
}

/// Resolves one `DTSTART`/`DTEND`/`RECURRENCE-ID`-shaped property to (UTC instant,
/// is-all-day, resolved zone, and — only when all-day — the raw calendar date, needed by
/// `parse_vevent` to decide whether a DTEND represents a real multi-day span).
///
/// TZID handling is the actual fix over `family-board`'s `parse_ical_dt` (which
/// silently treated any non-`Z` local time as UTC): a `TZID` param, when present, is
/// resolved via `chrono_tz` and the naive local time is localized in that zone before
/// converting to UTC. Falls back to treating the naive value as UTC only when the
/// value already ends in `Z`, or there's no usable `TZID` at all. An all-day value has
/// no `TZID` of its own (RFC 5545) — it's resolved in `all_day_tz` instead, see module
/// docs.
fn parse_dt_property(
    prop: &ical::property::Property,
    all_day_tz: Tz,
) -> Option<(DateTime<Utc>, bool, Tz, Option<NaiveDate>)> {
    parse_dt_value(prop.value.as_deref()?, prop.params.as_ref(), all_day_tz)
}

/// A single comma-separated `EXDATE` value can list multiple instants sharing the same
/// `TZID`/`VALUE` params — this parses every one of them (an `EXDATE` property with a
/// single value is just the one-element case).
fn parse_exdate_values(prop: &ical::property::Property, all_day_tz: Tz) -> Vec<DateTime<Utc>> {
    let Some(val) = prop.value.as_deref() else {
        return Vec::new();
    };
    val.split(',')
        .filter_map(|part| parse_dt_value(part.trim(), prop.params.as_ref(), all_day_tz))
        .map(|(dt, _, _, _)| dt)
        .collect()
}

fn parse_dt_value(
    val: &str,
    params: Option<&Vec<(String, Vec<String>)>>,
    all_day_tz: Tz,
) -> Option<(DateTime<Utc>, bool, Tz, Option<NaiveDate>)> {
    let is_all_day = params
        .and_then(|params| params.iter().find(|(k, _)| k == "VALUE"))
        .map(|(_, vals)| vals.iter().any(|v| v == "DATE"))
        .unwrap_or(val.len() == 8);

    if is_all_day {
        let date = NaiveDate::parse_from_str(val, "%Y%m%d").ok()?;
        return Some((
            resolve_all_day_instant(date, all_day_tz, false),
            true,
            all_day_tz,
            Some(date),
        ));
    }

    if let Some(stripped) = val.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&naive), false, Tz::UTC, None));
    }

    let naive = NaiveDateTime::parse_from_str(val, "%Y%m%dT%H%M%S").ok()?;
    let tzid = params
        .and_then(|params| params.iter().find(|(k, _)| k == "TZID"))
        .and_then(|(_, vals)| vals.first());
    let resolved_tz = tzid.and_then(|tzid| Tz::from_str(tzid).ok());

    let localized = resolved_tz.and_then(|tz| {
        match tz.from_local_datetime(&naive) {
            chrono::LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
            // A local time that maps to two UTC instants (a fall-back DST transition) —
            // picking the earlier one is an arbitrary but harmless tie-break; RFC 5545
            // itself doesn't disambiguate this case either.
            chrono::LocalResult::Ambiguous(dt, _) => Some(dt.with_timezone(&Utc)),
            chrono::LocalResult::None => None,
        }
    });

    Some((
        localized.unwrap_or_else(|| Utc.from_utc_datetime(&naive)),
        false,
        resolved_tz.unwrap_or(Tz::UTC),
        None,
    ))
}

/// Localizes `date` at either local midnight (`end_of_day: false`, used for every
/// all-day `DTSTART`/`RECURRENCE-ID`/`EXDATE`, and a real multi-day DTEND's *start*
/// side is never called this way — only its already-decremented inclusive end date is)
/// or 23:59:59 local (`end_of_day: true`, used only for a real multi-day span's
/// inclusive end date) in `tz`, then converts to UTC.
fn resolve_all_day_instant(date: NaiveDate, tz: Tz, end_of_day: bool) -> DateTime<Utc> {
    let time = if end_of_day {
        chrono::NaiveTime::from_hms_opt(23, 59, 59)
    } else {
        chrono::NaiveTime::from_hms_opt(0, 0, 0)
    }
    .expect("valid constant time");
    let naive = date.and_time(time);
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        // A local wall-clock time that a DST spring-forward skipped entirely (rare at
        // midnight, but not impossible) — falling back to treating it as UTC is a
        // harmless, arbitrary tie-break, same precedent as the timed-value path above.
        chrono::LocalResult::None => Utc.from_utc_datetime(&naive),
    }
}

fn within_import_window(start: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let window_start = now - Duration::days(IMPORT_WINDOW_PAST_DAYS);
    let window_end = now + Duration::days(IMPORT_WINDOW_FUTURE_DAYS);
    start >= window_start && start <= window_end
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Master events whose `RRULE` failed to parse or expand — every other recurring
    /// event is expanded into per-occurrence pseudo-events and imported normally (Stage
    /// 7). Not a count of "recurring events, unconditionally skipped" the way it was in
    /// Stage 3.
    pub skipped_recurring: usize,
}

/// Expands one `RRULE`-bearing master event into its individual occurrences within the
/// recurring import window, applying any `RECURRENCE-ID` override (and dropping any
/// `STATUS:CANCELLED` override's slot) along the way. Each occurrence comes back as an
/// already-"final" `ParsedIcalEvent` — `has_rrule: false`, `uid` replaced with a
/// deterministic `"{uid}::{original_occurrence_start_rfc3339}"` synthetic id — so
/// `run_diff`'s existing `google_event_id`-keyed diff needs no changes to create/update/
/// delete these exactly like any plain, non-recurring imported event.
///
/// `Err` only when the `RRULE` string itself can't be parsed/validated by the `rrule`
/// crate (a malformed feed) — the caller counts this into `SyncSummary::skipped_recurring`
/// rather than failing the whole sync.
fn expand_master_event(
    master: &ParsedIcalEvent,
    overrides: &HashMap<(String, DateTime<Utc>), ParsedIcalEvent>,
    now: DateTime<Utc>,
) -> Result<Vec<ParsedIcalEvent>, String> {
    let rrule_str = master
        .rrule
        .as_deref()
        .ok_or_else(|| "master event has no RRULE".to_string())?;
    let rtz: RTz = master.tz.into();
    let dt_start = master.start.with_timezone(&rtz);

    let unvalidated: RRule<Unvalidated> = rrule_str
        .parse()
        .map_err(|e: RRuleError| format!("invalid RRULE {rrule_str:?}: {e}"))?;
    let mut set = unvalidated
        .build(dt_start)
        .map_err(|e| format!("invalid RRULE {rrule_str:?} for DTSTART {dt_start}: {e}"))?;
    for exdate in &master.exdates {
        set = set.exdate(exdate.with_timezone(&rtz));
    }

    let window_start = (now - Duration::days(RECURRING_WINDOW_PAST_DAYS)).with_timezone(&rtz);
    let window_end = (now + Duration::days(RECURRING_WINDOW_FUTURE_DAYS)).with_timezone(&rtz);
    let result = set
        .after(window_start)
        .before(window_end)
        .all(RECURRING_MAX_OCCURRENCES);
    if result.limited {
        tracing::warn!(
            uid = %master.uid,
            cap = RECURRING_MAX_OCCURRENCES,
            "recurring calendar event hit the expansion cap within its import window"
        );
    }

    // A master's own DTEND (if any) is a fixed offset from its DTSTART — each generated
    // occurrence's end is that same offset applied to its own (possibly overridden)
    // start, not the master's literal DTEND instant.
    let duration = master.end.map(|end| end - master.start);

    Ok(result
        .dates
        .into_iter()
        .filter_map(|occurrence| {
            let original_start = occurrence.with_timezone(&Utc);
            let synthetic_id = format!("{}::{}", master.uid, original_start.to_rfc3339());

            match overrides.get(&(master.uid.clone(), original_start)) {
                // A cancelled override removes this one occurrence from the series,
                // exactly like an EXDATE would have.
                Some(ov) if ov.cancelled => None,
                Some(ov) => Some(ParsedIcalEvent {
                    uid: synthetic_id,
                    summary: ov.summary.clone(),
                    description: ov.description.clone(),
                    start: ov.start,
                    end: ov.end,
                    all_day: ov.all_day,
                    has_rrule: false,
                    rrule: None,
                    exdates: Vec::new(),
                    tz: ov.tz,
                    recurrence_id: None,
                    cancelled: false,
                }),
                None => Some(ParsedIcalEvent {
                    uid: synthetic_id,
                    summary: master.summary.clone(),
                    description: master.description.clone(),
                    start: original_start,
                    end: duration.map(|d| original_start + d),
                    all_day: master.all_day,
                    has_rrule: false,
                    rrule: None,
                    exdates: Vec::new(),
                    tz: master.tz,
                    recurrence_id: None,
                    cancelled: false,
                }),
            }
        })
        .collect())
}

fn build_imported_item(subscription: &CalendarSubscription, event: &ParsedIcalEvent) -> Item {
    let mut item = Item::new_project_item(&subscription.project_id, &event.summary);
    item.description = event.description.clone();
    item.google_event_id = Some(event.uid.clone());
    item.calendar_subscription_id = Some(subscription.id.clone());
    item.item_type = ItemType::Event(EventItem {
        schedule: Schedule {
            due_date: None,
            has_due_time: false,
            scheduled_date: Some(event.start),
            has_scheduled_time: !event.all_day,
            scheduled_end_date: event.end,
            has_end_time: event.end.is_some() && !event.all_day,
        },
        recurrence: Recurrence::default(),
        event_type: None,
        series_id: None,
    });
    item
}

/// `true` if `event`'s display-relevant fields diverge from `existing`'s — i.e. an
/// `update_by_project` write is actually needed. Avoids a needless write on every
/// ~15-minute sweep for an event whose upstream data hasn't changed.
fn event_differs_from_item(event: &ParsedIcalEvent, existing: &Item) -> bool {
    let schedule = existing.item_type.schedule();
    event.summary != existing.name
        || event.description != existing.description
        || schedule.map(|s| s.scheduled_date) != Some(Some(event.start))
        || schedule.and_then(|s| s.scheduled_end_date) != event.end
        || schedule.map(|s| s.has_scheduled_time) != Some(!event.all_day)
}

/// The zone to resolve this subscription's all-day event dates in — the creator's own
/// `User::timezone` if they've set one, UTC otherwise (see module docs). A missing user
/// record (shouldn't happen in practice, but `created_by_user_id` is a plain string, not
/// a foreign key) falls back to UTC the same way.
async fn resolve_all_day_tz(subscription: &CalendarSubscription, user_repo: &dyn UserRepo) -> Tz {
    match user_repo.get(&subscription.created_by_user_id).await {
        Ok(user) => user
            .timezone
            .as_deref()
            .and_then(|tz| Tz::from_str(tz).ok())
            .unwrap_or(Tz::UTC),
        Err(_) => Tz::UTC,
    }
}

pub async fn sync_subscription(
    subscription: &CalendarSubscription,
    item_repo: &dyn ItemRepo,
    calendar_repo: &dyn CalendarSubscriptionRepo,
    user_repo: &dyn UserRepo,
) -> Result<SyncSummary, CalendarSyncError> {
    let now = Utc::now();

    let content = match fetch_ical(&subscription.ical_url).await {
        Ok(content) => content,
        Err(e) => {
            // Best-effort: a failed `record_sync_result` write shouldn't mask the
            // original fetch error the caller actually needs to see/log.
            let _ = calendar_repo
                .record_sync_result(&subscription.id, now, Some(e.to_string()))
                .await;
            return Err(e);
        }
    };

    let all_day_tz = resolve_all_day_tz(subscription, user_repo).await;
    let parsed = parse_ical(&content, all_day_tz);

    let mut masters: Vec<ParsedIcalEvent> = Vec::new();
    let mut overrides: HashMap<(String, DateTime<Utc>), ParsedIcalEvent> = HashMap::new();
    let mut non_recurring: Vec<ParsedIcalEvent> = Vec::new();
    for event in parsed {
        if let Some(recurrence_id) = event.recurrence_id {
            overrides.insert((event.uid.clone(), recurrence_id), event);
        } else if event.has_rrule {
            masters.push(event);
        } else {
            non_recurring.push(event);
        }
    }

    let mut skipped_recurring = 0usize;
    let mut expanded: Vec<ParsedIcalEvent> = Vec::new();
    for master in &masters {
        match expand_master_event(master, &overrides, now) {
            Ok(occurrences) => expanded.extend(occurrences),
            Err(e) => {
                tracing::warn!(
                    uid = %master.uid,
                    error = %e,
                    "failed to expand recurring calendar event, skipping"
                );
                skipped_recurring += 1;
            }
        }
    }

    // Expanded occurrences are already window-filtered by `expand_master_event`'s own
    // `after`/`before` query (the tighter recurring-occurrence window) — only the plain,
    // non-recurring events need the wider `within_import_window` check here.
    let mut importable: Vec<ParsedIcalEvent> = non_recurring
        .into_iter()
        .filter(|e| within_import_window(e.start, now))
        .collect();
    importable.extend(expanded);

    let sync_result = run_diff(subscription, &importable, item_repo).await;

    match &sync_result {
        Ok(_) => {
            calendar_repo
                .record_sync_result(&subscription.id, now, None)
                .await
                .map_err(repo_err)?;
        }
        Err(e) => {
            let _ = calendar_repo
                .record_sync_result(&subscription.id, now, Some(e.to_string()))
                .await;
        }
    }

    sync_result.map(|(created, updated, deleted)| SyncSummary {
        created,
        updated,
        deleted,
        skipped_recurring,
    })
}

/// The create/update/delete diff itself, factored out of `sync_subscription` so the
/// success/failure bookkeeping around `record_sync_result` above stays in one place.
/// Returns `(created, updated, deleted)` counts.
async fn run_diff(
    subscription: &CalendarSubscription,
    importable: &[ParsedIcalEvent],
    item_repo: &dyn ItemRepo,
) -> Result<(usize, usize, usize), CalendarSyncError> {
    let existing = item_repo
        .list_by_calendar_subscription(&subscription.id)
        .await
        .map_err(repo_err)?;

    let existing_by_uid: HashMap<&str, &Item> = existing
        .iter()
        .filter_map(|item| item.google_event_id.as_deref().map(|uid| (uid, item)))
        .collect();

    let mut created = 0usize;
    let mut updated = 0usize;
    let mut seen_uids: HashSet<&str> = HashSet::new();

    for event in importable {
        seen_uids.insert(event.uid.as_str());
        match existing_by_uid.get(event.uid.as_str()) {
            None => {
                let item = build_imported_item(subscription, event);
                item_repo.create(&item).await.map_err(repo_err)?;
                created += 1;
            }
            Some(existing_item) => {
                if event_differs_from_item(event, existing_item) {
                    let mut updated_item = (*existing_item).clone();
                    updated_item.name = event.summary.clone();
                    updated_item.description = event.description.clone();
                    updated_item.item_type = ItemType::Event(EventItem {
                        schedule: Schedule {
                            due_date: None,
                            has_due_time: false,
                            scheduled_date: Some(event.start),
                            has_scheduled_time: !event.all_day,
                            scheduled_end_date: event.end,
                            has_end_time: event.end.is_some() && !event.all_day,
                        },
                        recurrence: Recurrence::default(),
                        event_type: None,
                        series_id: existing_item.series_id(),
                    });
                    item_repo
                        .update_by_project(&updated_item)
                        .await
                        .map_err(repo_err)?;
                    updated += 1;
                }
            }
        }
    }

    let mut deleted = 0usize;
    for item in &existing {
        let Some(uid) = item.google_event_id.as_deref() else {
            continue;
        };
        if !seen_uids.contains(uid) {
            item_repo.delete(&item.id).await.map_err(repo_err)?;
            deleted += 1;
        }
    }

    Ok((created, updated, deleted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::ItemKind;
    use crate::storage::sqlite::{MockCalendarSubscriptionRepo, MockItemRepo};
    use mockall::predicate::eq;

    fn subscription() -> CalendarSubscription {
        CalendarSubscription {
            id: "sub-1".to_string(),
            project_id: "project-1".to_string(),
            ical_url: "https://example.com/cal.ics".to_string(),
            created_by_user_id: "user-1".to_string(),
            created_at: Utc::now(),
            last_synced_at: None,
            last_sync_error: None,
        }
    }

    fn timed_event_ics(uid: &str, dtstart: &str, dtend: &str, tzid: Option<&str>) -> String {
        let (start_line, end_line) = match tzid {
            Some(tz) => (
                format!("DTSTART;TZID={tz}:{dtstart}"),
                format!("DTEND;TZID={tz}:{dtend}"),
            ),
            None => (format!("DTSTART:{dtstart}"), format!("DTEND:{dtend}")),
        };
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\n{start_line}\r\n{end_line}\r\nSUMMARY:Test Event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    #[test]
    fn parses_a_plain_timed_utc_event() {
        let ics = timed_event_ics("evt-1", "20260901T140000Z", "20260901T150000Z", None);
        let events = parse_ical(&ics, Tz::UTC);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.uid, "evt-1");
        assert_eq!(e.summary, "Test Event");
        assert!(!e.all_day);
        assert!(!e.has_rrule);
        assert_eq!(e.start, Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap());
        assert_eq!(
            e.end,
            Some(Utc.with_ymd_and_hms(2026, 9, 1, 15, 0, 0).unwrap())
        );
    }

    #[test]
    fn single_day_all_day_event_has_no_end_date() {
        // Google Calendar's real export always includes an explicit DTEND even for a
        // single-day all-day event (DTSTART + 1 day, RFC 5545's exclusive-end
        // convention) — per issue #1 in docs/issues_and_features.md, this should show
        // only a start date, not a spurious "2-day" end.
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-allday\r\n",
            "DTSTART;VALUE=DATE:20260910\r\nDTEND;VALUE=DATE:20260911\r\n",
            "SUMMARY:All Day\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics, Tz::UTC);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert!(e.all_day);
        assert_eq!(e.start, Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap());
        assert_eq!(e.end, None);
    }

    #[test]
    fn all_day_event_with_no_dtend_has_no_end_date() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-allday-2\r\n",
            "DTSTART;VALUE=DATE:20260910\r\nSUMMARY:All Day No End\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics, Tz::UTC);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].end, None);
    }

    #[test]
    fn multi_day_all_day_event_keeps_an_inclusive_end_date() {
        // A real 3-day span (Sept 10-12 inclusive) — Google encodes DTEND as the
        // exclusive day after the last day (Sept 13). The stored end should be the
        // actual last day (Sept 12) at end-of-day, not the exclusive Sept 13.
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-trip\r\n",
            "DTSTART;VALUE=DATE:20260910\r\nDTEND;VALUE=DATE:20260913\r\n",
            "SUMMARY:Trip\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics, Tz::UTC);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.start, Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap());
        assert_eq!(
            e.end,
            Some(Utc.with_ymd_and_hms(2026, 9, 12, 23, 59, 59).unwrap())
        );
    }

    #[test]
    fn all_day_event_is_resolved_in_the_given_all_day_tz_not_utc() {
        // The regression this fixes (issue #1): an all-day date parsed as literal UTC
        // midnight displays a day early for any viewer behind UTC. Resolving it in the
        // subscription creator's own timezone instead keeps the UTC instant on the
        // correct side of midnight once the display layer's `to_local` (which expects
        // every stored instant to carry a real, undoable offset) converts it back.
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-allday-ny\r\n",
            "DTSTART;VALUE=DATE:20260910\r\nSUMMARY:All Day NY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let ny = Tz::America__New_York;
        let events = parse_ical(ics, ny);
        assert_eq!(events.len(), 1);
        // 2026-09-10 00:00 America/New_York (EDT, UTC-4) == 04:00 UTC — NOT the literal
        // 2026-09-10 00:00 UTC the old, tz-agnostic parsing produced.
        assert_eq!(
            events[0].start,
            Utc.with_ymd_and_hms(2026, 9, 10, 4, 0, 0).unwrap()
        );
        assert_eq!(events[0].tz, ny);
    }

    #[test]
    fn resolves_a_tzid_bearing_event_in_a_non_utc_zone() {
        // 2026-09-01 09:00 America/New_York (EDT, UTC-4) == 2026-09-01 13:00 UTC.
        let ics = timed_event_ics(
            "evt-tz",
            "20260901T090000",
            "20260901T100000",
            Some("America/New_York"),
        );
        let events = parse_ical(&ics, Tz::UTC);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].start,
            Utc.with_ymd_and_hms(2026, 9, 1, 13, 0, 0).unwrap()
        );
        assert_eq!(
            events[0].end,
            Some(Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap())
        );
    }

    #[test]
    fn skips_a_cancelled_event() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-cancelled\r\n",
            "DTSTART:20260901T140000Z\r\nSTATUS:CANCELLED\r\nSUMMARY:Nope\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        assert!(parse_ical(ics, Tz::UTC).is_empty());
    }

    #[test]
    fn flags_an_rrule_bearing_event_and_captures_its_rrule_string() {
        // `parse_ical` only flags/captures a recurring master event — expansion into
        // per-occurrence pseudo-events is `expand_master_event`'s job (exercised
        // separately below), so this only asserts the parse step.
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-recurring\r\n",
            "DTSTART:20260901T140000Z\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Standup\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics, Tz::UTC);
        assert_eq!(events.len(), 1);
        assert!(events[0].has_rrule);
        assert_eq!(events[0].rrule.as_deref(), Some("FREQ=WEEKLY"));
    }

    #[test]
    fn captures_exdates_and_a_recurrence_id_override() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
            "BEGIN:VEVENT\r\nUID:evt-recurring\r\nDTSTART:20260901T140000Z\r\n",
            "RRULE:FREQ=WEEKLY\r\nEXDATE:20260908T140000Z,20260915T140000Z\r\n",
            "SUMMARY:Standup\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:evt-recurring\r\nRECURRENCE-ID:20260922T140000Z\r\n",
            "DTSTART:20260922T150000Z\r\nSUMMARY:Standup (moved)\r\nEND:VEVENT\r\n",
            "END:VCALENDAR\r\n"
        );
        let events = parse_ical(ics, Tz::UTC);
        assert_eq!(events.len(), 2);

        let master = events.iter().find(|e| e.has_rrule).unwrap();
        assert_eq!(
            master.exdates,
            vec![
                Utc.with_ymd_and_hms(2026, 9, 8, 14, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 9, 15, 14, 0, 0).unwrap(),
            ]
        );

        let over_ride = events.iter().find(|e| e.recurrence_id.is_some()).unwrap();
        assert_eq!(
            over_ride.recurrence_id,
            Some(Utc.with_ymd_and_hms(2026, 9, 22, 14, 0, 0).unwrap())
        );
        assert_eq!(
            over_ride.start,
            Utc.with_ymd_and_hms(2026, 9, 22, 15, 0, 0).unwrap()
        );
        assert_eq!(over_ride.summary, "Standup (moved)");
        assert!(!over_ride.cancelled);
    }

    #[test]
    fn keeps_a_cancelled_recurrence_id_override_as_data() {
        // Unlike a plain cancelled event (dropped entirely, see `skips_a_cancelled_event`
        // above), a cancelled *override* has to survive parsing so the expansion step
        // knows which occurrence to drop.
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-recurring\r\n",
            "RECURRENCE-ID:20260922T140000Z\r\nDTSTART:20260922T140000Z\r\n",
            "STATUS:CANCELLED\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics, Tz::UTC);
        assert_eq!(events.len(), 1);
        assert!(events[0].cancelled);
        assert!(events[0].recurrence_id.is_some());
    }

    /// A minimal plain (non-recurring) `ParsedIcalEvent` for `run_diff` tests, with every
    /// Stage-7-only field at its "not recurring" default.
    fn plain_event(
        uid: &str,
        summary: &str,
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
    ) -> ParsedIcalEvent {
        ParsedIcalEvent {
            uid: uid.to_string(),
            summary: summary.to_string(),
            description: None,
            start,
            end,
            all_day: false,
            has_rrule: false,
            rrule: None,
            exdates: Vec::new(),
            tz: Tz::UTC,
            recurrence_id: None,
            cancelled: false,
        }
    }

    /// A minimal `RRULE`-bearing master `ParsedIcalEvent` for `expand_master_event` tests.
    fn recurring_master(
        uid: &str,
        rrule: &str,
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
        tz: Tz,
    ) -> ParsedIcalEvent {
        ParsedIcalEvent {
            uid: uid.to_string(),
            summary: "Standup".to_string(),
            description: None,
            start,
            end,
            all_day: false,
            has_rrule: true,
            rrule: Some(rrule.to_string()),
            exdates: Vec::new(),
            tz,
            recurrence_id: None,
            cancelled: false,
        }
    }

    fn imported_item(id: &str, uid: &str, sub_id: &str, start: DateTime<Utc>) -> Item {
        Item {
            id: id.to_string(),
            project_id: Some("project-1".to_string()),
            name: "Test Event".to_string(),
            google_event_id: Some(uid.to_string()),
            calendar_subscription_id: Some(sub_id.to_string()),
            item_type: ItemType::Event(EventItem {
                schedule: Schedule {
                    scheduled_date: Some(start),
                    has_scheduled_time: true,
                    scheduled_end_date: Some(start + Duration::hours(1)),
                    has_end_time: true,
                    ..Schedule::default()
                },
                recurrence: Recurrence::default(),
                event_type: None,
                series_id: None,
            }),
            ..Item::default()
        }
    }

    #[tokio::test]
    async fn creates_a_new_event_not_previously_imported() {
        let sub = subscription();
        let mut item_repo = MockItemRepo::new();
        item_repo
            .expect_list_by_calendar_subscription()
            .with(eq("sub-1"))
            .returning(|_| Ok(vec![]));
        item_repo
            .expect_create()
            .withf(|item: &Item| {
                item.google_event_id.as_deref() == Some("evt-1")
                    && item.calendar_subscription_id.as_deref() == Some("sub-1")
                    && item.kind() == ItemKind::Event
            })
            .returning(|_| Ok("new-id".to_string()));

        let mut calendar_repo = MockCalendarSubscriptionRepo::new();
        calendar_repo
            .expect_record_sync_result()
            .withf(|id, _, error| id == "sub-1" && error.is_none())
            .returning(|_, _, _| Ok(()));

        let event = plain_event(
            "evt-1",
            "Test Event",
            Utc::now() + Duration::days(1),
            Some(Utc::now() + Duration::days(1) + Duration::hours(1)),
        );
        // Exercise run_diff directly (the pure diff, no network fetch) via the public
        // sync_subscription would require mocking `fetch_ical`'s reqwest call, which
        // this module deliberately doesn't abstract behind a trait (see Stage 3 plan) —
        // so these tests drive `run_diff` directly instead.
        let (created, updated, deleted) = run_diff(&sub, &[event], &item_repo).await.unwrap();
        assert_eq!((created, updated, deleted), (1, 0, 0));
        // calendar_repo isn't touched by run_diff itself; drop it unused here.
        drop(calendar_repo);
    }

    #[tokio::test]
    async fn updates_an_existing_event_whose_summary_changed() {
        let sub = subscription();
        let start = Utc::now() + Duration::days(1);
        let existing = imported_item("item-1", "evt-1", "sub-1", start);

        let mut item_repo = MockItemRepo::new();
        item_repo
            .expect_list_by_calendar_subscription()
            .with(eq("sub-1"))
            .returning(move |_| Ok(vec![existing.clone()]));
        item_repo
            .expect_update_by_project()
            .withf(|item: &Item| item.id == "item-1" && item.name == "Renamed Event")
            .returning(|_| Ok(()));

        let event = plain_event(
            "evt-1",
            "Renamed Event",
            start,
            Some(start + Duration::hours(1)),
        );
        let (created, updated, deleted) = run_diff(&sub, &[event], &item_repo).await.unwrap();
        assert_eq!((created, updated, deleted), (0, 1, 0));
    }

    #[tokio::test]
    async fn no_op_when_nothing_changed() {
        let sub = subscription();
        let start = Utc::now() + Duration::days(1);
        let existing = imported_item("item-1", "evt-1", "sub-1", start);

        let mut item_repo = MockItemRepo::new();
        item_repo
            .expect_list_by_calendar_subscription()
            .with(eq("sub-1"))
            .returning(move |_| Ok(vec![existing.clone()]));
        // No `expect_update_by_project`/`expect_create` set up at all — a call to
        // either would panic the mock, which is exactly the assertion here.

        let event = plain_event(
            "evt-1",
            "Test Event",
            start,
            Some(start + Duration::hours(1)),
        );
        let (created, updated, deleted) = run_diff(&sub, &[event], &item_repo).await.unwrap();
        assert_eq!((created, updated, deleted), (0, 0, 0));
    }

    #[tokio::test]
    async fn deletes_a_previously_imported_event_no_longer_in_the_feed() {
        let sub = subscription();
        let existing = imported_item("item-1", "evt-1", "sub-1", Utc::now() + Duration::days(1));

        let mut item_repo = MockItemRepo::new();
        item_repo
            .expect_list_by_calendar_subscription()
            .with(eq("sub-1"))
            .returning(move |_| Ok(vec![existing.clone()]));
        item_repo
            .expect_delete()
            .with(eq("item-1"))
            .returning(|_| Ok(()));

        let (created, updated, deleted) = run_diff(&sub, &[], &item_repo).await.unwrap();
        assert_eq!((created, updated, deleted), (0, 0, 1));
    }

    #[test]
    fn import_window_bounds_are_respected() {
        let now = Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        assert!(within_import_window(now, now));
        assert!(within_import_window(now + Duration::days(364), now));
        assert!(!within_import_window(now + Duration::days(366), now));
        assert!(within_import_window(now - Duration::days(29), now));
        assert!(!within_import_window(now - Duration::days(31), now));
    }

    #[test]
    fn expand_master_event_generates_weekly_occurrences_within_the_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap();
        let master = recurring_master(
            "evt-recurring",
            "FREQ=WEEKLY",
            start,
            Some(start + Duration::hours(1)),
            Tz::UTC,
        );

        let occurrences = expand_master_event(&master, &HashMap::new(), now).unwrap();

        assert!(
            occurrences.len() > 20,
            "expected many weekly occurrences within a ~187-day window, got {}",
            occurrences.len()
        );
        assert_eq!(occurrences[0].start, start);
        assert_eq!(occurrences[0].end, Some(start + Duration::hours(1)));
        assert_eq!(
            occurrences[0].uid,
            format!("evt-recurring::{}", start.to_rfc3339())
        );
        assert!(!occurrences[0].has_rrule);
        for pair in occurrences.windows(2) {
            assert_eq!(pair[1].start - pair[0].start, Duration::days(7));
        }
        let window_start = now - Duration::days(RECURRING_WINDOW_PAST_DAYS);
        let window_end = now + Duration::days(RECURRING_WINDOW_FUTURE_DAYS);
        for occ in &occurrences {
            assert!(occ.start >= window_start && occ.start <= window_end);
        }
    }

    #[test]
    fn expand_master_event_skips_exdate_occurrences() {
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap();
        let excluded = start + Duration::days(7);
        let mut master = recurring_master("evt-recurring", "FREQ=WEEKLY", start, None, Tz::UTC);
        master.exdates = vec![excluded];

        let occurrences = expand_master_event(&master, &HashMap::new(), now).unwrap();

        assert!(occurrences.iter().all(|o| o.start != excluded));
        assert!(occurrences.iter().any(|o| o.start == start));
        assert!(
            occurrences
                .iter()
                .any(|o| o.start == start + Duration::days(14))
        );
    }

    #[test]
    fn expand_master_event_applies_a_recurrence_id_override() {
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap();
        let original_occurrence = start + Duration::days(7);
        let master = recurring_master(
            "evt-recurring",
            "FREQ=WEEKLY",
            start,
            Some(start + Duration::hours(1)),
            Tz::UTC,
        );

        let moved_start = original_occurrence + Duration::hours(1);
        let mut overrides = HashMap::new();
        overrides.insert(
            ("evt-recurring".to_string(), original_occurrence),
            ParsedIcalEvent {
                uid: "evt-recurring".to_string(),
                summary: "Standup (moved)".to_string(),
                description: None,
                start: moved_start,
                end: Some(moved_start + Duration::hours(1)),
                all_day: false,
                has_rrule: false,
                rrule: None,
                exdates: Vec::new(),
                tz: Tz::UTC,
                recurrence_id: Some(original_occurrence),
                cancelled: false,
            },
        );

        let occurrences = expand_master_event(&master, &overrides, now).unwrap();

        // The overridden slot keeps the id derived from its ORIGINAL timestamp (not the
        // moved one) — that's what lets `run_diff` update the existing imported item in
        // place instead of creating a duplicate.
        let overridden = occurrences
            .iter()
            .find(|o| o.uid == format!("evt-recurring::{}", original_occurrence.to_rfc3339()))
            .expect("overridden slot should still be present under its original-timestamp id");
        assert_eq!(overridden.start, moved_start);
        assert_eq!(overridden.summary, "Standup (moved)");
        // Every other occurrence is untouched.
        assert!(occurrences.iter().any(|o| o.start == start));
    }

    #[test]
    fn expand_master_event_drops_a_cancelled_override_slot() {
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap();
        let cancelled_occurrence = start + Duration::days(7);
        let master = recurring_master("evt-recurring", "FREQ=WEEKLY", start, None, Tz::UTC);

        let mut overrides = HashMap::new();
        overrides.insert(
            ("evt-recurring".to_string(), cancelled_occurrence),
            ParsedIcalEvent {
                uid: "evt-recurring".to_string(),
                summary: "Standup".to_string(),
                description: None,
                start: cancelled_occurrence,
                end: None,
                all_day: false,
                has_rrule: false,
                rrule: None,
                exdates: Vec::new(),
                tz: Tz::UTC,
                recurrence_id: Some(cancelled_occurrence),
                cancelled: true,
            },
        );

        let occurrences = expand_master_event(&master, &overrides, now).unwrap();

        assert!(occurrences.iter().all(|o| o.start != cancelled_occurrence));
        assert!(occurrences.iter().any(|o| o.start == start));
    }

    /// The correctness case this stage's own plan flagged as easy to get subtly wrong:
    /// a weekly local-time event must stay pinned to the same wall-clock hour across a
    /// DST transition, not drift by an hour in UTC.
    #[test]
    fn expand_master_event_respects_local_wall_clock_across_a_dst_transition() {
        let ny = Tz::America__New_York;
        let now = Utc.with_ymd_and_hms(2026, 10, 25, 0, 0, 0).unwrap();
        // 2026-10-27 09:00 America/New_York (EDT, UTC-4) == 13:00 UTC.
        let start = Utc.with_ymd_and_hms(2026, 10, 27, 13, 0, 0).unwrap();
        let master = recurring_master("evt-standup", "FREQ=WEEKLY", start, None, ny);

        let occurrences = expand_master_event(&master, &HashMap::new(), now).unwrap();

        assert!(occurrences.iter().any(|o| o.start == start));
        // A week later (2026-11-03), America/New_York has fallen back to EST (UTC-5):
        // 09:00 local == 14:00 UTC — NOT 13:00 UTC, which would be the wrong,
        // fixed-UTC-offset behavior `family-board`'s own version doesn't guard against.
        let after_transition = Utc.with_ymd_and_hms(2026, 11, 3, 14, 0, 0).unwrap();
        assert!(
            occurrences.iter().any(|o| o.start == after_transition),
            "expected the Nov 3 occurrence pinned to 9am America/New_York (14:00 UTC \
             after the fall-back); got starts: {:?}",
            occurrences.iter().map(|o| o.start).collect::<Vec<_>>()
        );
    }

    #[test]
    fn expand_master_event_returns_err_for_an_unparseable_rrule() {
        let now = Utc::now();
        let master = recurring_master("evt-bad", "NOT-A-VALID-RRULE", now, None, Tz::UTC);
        assert!(expand_master_event(&master, &HashMap::new(), now).is_err());
    }

    #[tokio::test]
    async fn run_diff_creates_items_for_each_expanded_recurring_occurrence() {
        // Demonstrates the plan's core claim: expanded occurrences carry a deterministic
        // synthetic `google_event_id`, so `run_diff`'s existing uid-keyed diff needs no
        // changes of its own to create/update/delete them.
        let sub = subscription();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap();
        let master = recurring_master("evt-recurring", "FREQ=WEEKLY;COUNT=3", start, None, Tz::UTC);
        let occurrences = expand_master_event(&master, &HashMap::new(), now).unwrap();
        assert_eq!(occurrences.len(), 3);

        let mut item_repo = MockItemRepo::new();
        item_repo
            .expect_list_by_calendar_subscription()
            .with(eq("sub-1"))
            .returning(|_| Ok(vec![]));
        item_repo
            .expect_create()
            .times(3)
            .returning(|_| Ok("new-id".to_string()));

        let (created, updated, deleted) = run_diff(&sub, &occurrences, &item_repo).await.unwrap();
        assert_eq!((created, updated, deleted), (3, 0, 0));
    }

    fn user_with_timezone(timezone: Option<&str>) -> crate::domain::user::User {
        crate::domain::user::User {
            id: "user-1".to_string(),
            first_name: "Test".to_string(),
            last_name: "User".to_string(),
            email: None,
            google_id: None,
            timezone: timezone.map(str::to_string),
            personal_project_id: None,
        }
    }

    #[tokio::test]
    async fn resolve_all_day_tz_uses_the_subscription_creators_timezone() {
        use crate::storage::sqlite::MockUserRepo;
        let mut user_repo = MockUserRepo::new();
        user_repo
            .expect_get()
            .with(eq("user-1"))
            .returning(|_| Ok(user_with_timezone(Some("America/New_York"))));

        let tz = resolve_all_day_tz(&subscription(), &user_repo).await;
        assert_eq!(tz, Tz::America__New_York);
    }

    #[tokio::test]
    async fn resolve_all_day_tz_falls_back_to_utc_when_unset() {
        use crate::storage::sqlite::MockUserRepo;
        let mut user_repo = MockUserRepo::new();
        user_repo
            .expect_get()
            .with(eq("user-1"))
            .returning(|_| Ok(user_with_timezone(None)));

        let tz = resolve_all_day_tz(&subscription(), &user_repo).await;
        assert_eq!(tz, Tz::UTC);
    }

    #[tokio::test]
    async fn resolve_all_day_tz_falls_back_to_utc_when_unparseable() {
        use crate::storage::sqlite::MockUserRepo;
        let mut user_repo = MockUserRepo::new();
        user_repo
            .expect_get()
            .with(eq("user-1"))
            .returning(|_| Ok(user_with_timezone(Some("Not/A_Zone"))));

        let tz = resolve_all_day_tz(&subscription(), &user_repo).await;
        assert_eq!(tz, Tz::UTC);
    }

    #[tokio::test]
    async fn resolve_all_day_tz_falls_back_to_utc_when_user_missing() {
        use crate::storage::sqlite::MockUserRepo;
        let mut user_repo = MockUserRepo::new();
        user_repo
            .expect_get()
            .with(eq("user-1"))
            .returning(|_| Err(RepoError::NotFound));

        let tz = resolve_all_day_tz(&subscription(), &user_repo).await;
        assert_eq!(tz, Tz::UTC);
    }
}
