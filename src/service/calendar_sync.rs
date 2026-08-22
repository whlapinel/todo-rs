//! Google Calendar (iCal) import — see docs/google-calendar-import-plan.md, Stage 3.
//!
//! Fetches a project admin's private iCal feed, parses it, and diffs it against the
//! `Item` rows already imported from that `CalendarSubscription` (keyed on
//! `google_event_id`). Deliberately writes through `ItemRepo` directly rather than
//! `service::project_items`'s `create_project_item`/`update_project_item` — those
//! enforce per-request membership/role checks that don't apply to a background sync
//! process, and Stage 4's read-only guard on those functions would otherwise reject
//! the sync's own writes to an already-imported item.
//!
//! RRULE-bearing (recurring) events are parsed but never imported here — they're
//! counted into `SyncSummary::skipped_recurring` and otherwise ignored until Stage 7
//! adds RRULE expansion.

use crate::domain::calendar_subscription::CalendarSubscription;
use crate::domain::item::{Item, ItemType, Recurrence, Schedule};
use crate::storage::sqlite::{CalendarSubscriptionRepo, ItemRepo, RepoError};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
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

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedIcalEvent {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub all_day: bool,
    /// `true` if the VEVENT carries an `RRULE` — Stage 3 never imports these, only
    /// counts them (see `SyncSummary::skipped_recurring`). Stage 7 expands them.
    pub has_rrule: bool,
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
pub fn parse_ical(content: &str) -> Vec<ParsedIcalEvent> {
    let mut events = Vec::new();
    for calendar in ical::IcalParser::new(BufReader::new(content.as_bytes())).flatten() {
        for vevent in &calendar.events {
            if let Some(parsed) = parse_vevent(vevent) {
                events.push(parsed);
            }
        }
    }
    events
}

fn parse_vevent(vevent: &ical::parser::ical::component::IcalEvent) -> Option<ParsedIcalEvent> {
    let find_prop = |name: &str| vevent.properties.iter().find(|p| p.name == name);
    let prop_value = |name: &str| find_prop(name).and_then(|p| p.value.as_deref());

    if prop_value("STATUS") == Some("CANCELLED") {
        return None;
    }

    let uid = prop_value("UID")?.to_string();
    let summary = prop_value("SUMMARY").unwrap_or("(no title)").to_string();
    let description = prop_value("DESCRIPTION")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let has_rrule = find_prop("RRULE").is_some();

    let (start, all_day) = parse_dt_property(find_prop("DTSTART")?)?;

    let parsed_end = find_prop("DTEND")
        .and_then(parse_dt_property)
        .map(|(dt, _)| dt);
    // RFC 5545: an all-day VEVENT with no DTEND defaults to a 1-day duration — its
    // (exclusive) end is simply the day after DTSTART.
    let end = parsed_end.or_else(|| all_day.then(|| start + Duration::days(1)));

    Some(ParsedIcalEvent {
        uid,
        summary,
        description,
        start,
        end,
        all_day,
        has_rrule,
    })
}

/// Resolves one `DTSTART`/`DTEND`-shaped property to (UTC instant, is-all-day).
///
/// TZID handling is the actual fix over `family-board`'s `parse_ical_dt` (which
/// silently treated any non-`Z` local time as UTC): a `TZID` param, when present, is
/// resolved via `chrono_tz` and the naive local time is localized in that zone before
/// converting to UTC. Falls back to treating the naive value as UTC only when the
/// value already ends in `Z`, or there's no usable `TZID` at all.
fn parse_dt_property(prop: &ical::property::Property) -> Option<(DateTime<Utc>, bool)> {
    let val = prop.value.as_deref()?;

    let is_all_day = prop
        .params
        .as_ref()
        .and_then(|params| params.iter().find(|(k, _)| k == "VALUE"))
        .map(|(_, vals)| vals.iter().any(|v| v == "DATE"))
        .unwrap_or(val.len() == 8);

    if is_all_day {
        let date = NaiveDate::parse_from_str(val, "%Y%m%d").ok()?;
        return Some((Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?), true));
    }

    if let Some(stripped) = val.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&naive), false));
    }

    let naive = NaiveDateTime::parse_from_str(val, "%Y%m%dT%H%M%S").ok()?;
    let tzid = prop
        .params
        .as_ref()
        .and_then(|params| params.iter().find(|(k, _)| k == "TZID"))
        .and_then(|(_, vals)| vals.first());

    let localized = tzid.and_then(|tzid| Tz::from_str(tzid).ok()).and_then(|tz| {
        match tz.from_local_datetime(&naive) {
            chrono::LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
            // A local time that maps to two UTC instants (a fall-back DST transition) —
            // picking the earlier one is an arbitrary but harmless tie-break; RFC 5545
            // itself doesn't disambiguate this case either.
            chrono::LocalResult::Ambiguous(dt, _) => Some(dt.with_timezone(&Utc)),
            chrono::LocalResult::None => None,
        }
    });

    Some((localized.unwrap_or_else(|| Utc.from_utc_datetime(&naive)), false))
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
    pub skipped_recurring: usize,
}

fn build_imported_item(subscription: &CalendarSubscription, event: &ParsedIcalEvent) -> Item {
    let mut item = Item::new_project_item(&subscription.project_id, &event.summary);
    item.description = event.description.clone();
    item.google_event_id = Some(event.uid.clone());
    item.calendar_subscription_id = Some(subscription.id.clone());
    item.item_type = ItemType::Event {
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
    };
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

pub async fn sync_subscription(
    subscription: &CalendarSubscription,
    item_repo: &dyn ItemRepo,
    calendar_repo: &dyn CalendarSubscriptionRepo,
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

    let parsed = parse_ical(&content);
    let (recurring, non_recurring): (Vec<_>, Vec<_>) =
        parsed.into_iter().partition(|e| e.has_rrule);
    let skipped_recurring = recurring.len();

    let importable: Vec<ParsedIcalEvent> = non_recurring
        .into_iter()
        .filter(|e| within_import_window(e.start, now))
        .collect();

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
                    updated_item.item_type = ItemType::Event {
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
                    };
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
        let events = parse_ical(&ics);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.uid, "evt-1");
        assert_eq!(e.summary, "Test Event");
        assert!(!e.all_day);
        assert!(!e.has_rrule);
        assert_eq!(
            e.start,
            Utc.with_ymd_and_hms(2026, 9, 1, 14, 0, 0).unwrap()
        );
        assert_eq!(
            e.end,
            Some(Utc.with_ymd_and_hms(2026, 9, 1, 15, 0, 0).unwrap())
        );
    }

    #[test]
    fn parses_an_all_day_event() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-allday\r\n",
            "DTSTART;VALUE=DATE:20260910\r\nDTEND;VALUE=DATE:20260911\r\n",
            "SUMMARY:All Day\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert!(e.all_day);
        assert_eq!(e.start, Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap());
        assert_eq!(e.end, Some(Utc.with_ymd_and_hms(2026, 9, 11, 0, 0, 0).unwrap()));
    }

    #[test]
    fn all_day_event_with_no_dtend_defaults_to_one_day() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-allday-2\r\n",
            "DTSTART;VALUE=DATE:20260910\r\nSUMMARY:All Day No End\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].end,
            Some(Utc.with_ymd_and_hms(2026, 9, 11, 0, 0, 0).unwrap())
        );
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
        let events = parse_ical(&ics);
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
        assert!(parse_ical(ics).is_empty());
    }

    #[test]
    fn flags_but_does_not_expand_an_rrule_bearing_event() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-recurring\r\n",
            "DTSTART:20260901T140000Z\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Standup\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let events = parse_ical(ics);
        assert_eq!(events.len(), 1);
        assert!(events[0].has_rrule);
    }

    fn imported_item(id: &str, uid: &str, sub_id: &str, start: DateTime<Utc>) -> Item {
        Item {
            id: id.to_string(),
            project_id: Some("project-1".to_string()),
            name: "Test Event".to_string(),
            google_event_id: Some(uid.to_string()),
            calendar_subscription_id: Some(sub_id.to_string()),
            item_type: ItemType::Event {
                schedule: Schedule {
                    scheduled_date: Some(start),
                    has_scheduled_time: true,
                    scheduled_end_date: Some(start + Duration::hours(1)),
                    has_end_time: true,
                    ..Schedule::default()
                },
                recurrence: Recurrence::default(),
                event_type: None,
            },
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

        let event = ParsedIcalEvent {
            uid: "evt-1".to_string(),
            summary: "Test Event".to_string(),
            description: None,
            start: Utc::now() + Duration::days(1),
            end: Some(Utc::now() + Duration::days(1) + Duration::hours(1)),
            all_day: false,
            has_rrule: false,
        };
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

        let event = ParsedIcalEvent {
            uid: "evt-1".to_string(),
            summary: "Renamed Event".to_string(),
            description: None,
            start,
            end: Some(start + Duration::hours(1)),
            all_day: false,
            has_rrule: false,
        };
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

        let event = ParsedIcalEvent {
            uid: "evt-1".to_string(),
            summary: "Test Event".to_string(),
            description: None,
            start,
            end: Some(start + Duration::hours(1)),
            all_day: false,
            has_rrule: false,
        };
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
}
