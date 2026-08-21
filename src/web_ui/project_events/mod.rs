pub mod handlers;
pub mod templates;

use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{self as item_series_service, ProjectOccurrence};
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo};
use crate::web_ui::project_events::templates::{CalendarDay, ProjectEventRow, ProjectEventVirtualRow};
use askama::Template;
use axum::response::Html;
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being an Event — mirrors
/// `events::require_event`/`team_events::require_team_event`.
pub(crate) fn require_event(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Event {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Duplicated from events/team_events rather than shared, matching the precedent those two
// modules (and project_tasks) already set for this exact helper set. An Event is never a
// child and never carries assignment/points (see events.rs/team_events.rs's own comments on
// hardcoding `itemType: EVENT`/`parentItemId: None` and dropping assignment entirely).
//
// Deliberately no `complete`/`showComplete` fields — `Item::validate` rejects `complete: true`
// outright for `ItemType::Event` (mirroring the Simple-item precedent), so there's nothing for
// a form on this screen to ever legitimately set.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEventForm {
    name: Option<String>,
    description: Option<String>,
    scheduled_date: Option<String>,
    scheduled_time: Option<String>,
    scheduled_end_date: Option<String>,
    scheduled_end_time: Option<String>,
    due_date: Option<String>,
    due_time: Option<String>,
    event_type: Option<String>,
    /// See `project_tasks::ProjectTaskForm`'s identical field for the redirect-vs-in-place-
    /// fragment rationale.
    redirect: Option<String>,
}

pub(crate) fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn overlay_str(form_value: &Option<String>, current: Option<String>) -> Option<String> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
}

fn overlay_required_str(form_value: &Option<String>, current: &str) -> String {
    match form_value {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => current.to_string(),
    }
}

fn overlay_has_due_time(form_time: &Option<String>, current: bool) -> bool {
    match form_time {
        None => current,
        Some(s) => !s.trim().is_empty(),
    }
}

fn combine_local_to_utc(
    date: &str,
    time: Option<&str>,
    tz_offset_minutes: i32,
    default_time: chrono::NaiveTime,
) -> Option<DateTime<Utc>> {
    let naive_date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let naive_time = time
        .filter(|t| !t.trim().is_empty())
        .and_then(|t| chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M").ok())
        .unwrap_or(default_time);
    let naive = naive_date.and_time(naive_time);
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Some(as_utc + chrono::Duration::minutes(tz_offset_minutes as i64))
}

fn end_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap()
}

fn start_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}

/// Converts a local calendar date + time-of-day into the UTC instant it represents, given
/// `tz_offset_minutes` — same `local + offset = utc` convention as `combine_local_to_utc`
/// above, just taking a `NaiveDate` directly instead of parsing one from a form string.
pub(crate) fn local_date_to_utc(
    date: NaiveDate,
    time: chrono::NaiveTime,
    tz_offset_minutes: i32,
) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(date.and_time(time), Utc)
        + chrono::Duration::minutes(tz_offset_minutes as i64)
}

fn overlay_due_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            end_of_day(),
        ),
    }
}

fn overlay_scheduled_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            start_of_day(),
        ),
    }
}

fn overlay_scheduled_end_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            end_of_day(),
        ),
    }
}

pub(crate) fn create_params_from_form(
    project_id: &str,
    form: &ProjectEventForm,
    tz: i32,
) -> crate::service::project_items::CreateProjectItemParams {
    crate::service::project_items::CreateProjectItemParams {
        project_id: project_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        description: non_empty(&form.description),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, None),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            None,
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            None,
        ),
        complete: None,
        has_due_time: form.due_time.as_deref().map(|t| !t.trim().is_empty()),
        has_scheduled_time: form.scheduled_time.as_deref().map(|t| !t.trim().is_empty()),
        has_end_time: form
            .scheduled_end_time
            .as_deref()
            .map(|t| !t.trim().is_empty()),
        parent_item_id: None,
        item_type: Some(ItemKind::Event),
        event_type: non_empty(&form.event_type),
        due_offset_days: None,
        assigned_to_user_id: None,
        source_event_id: None,
        timezone_offset_minutes: Some(tz),
        points: None,
        series_id: None,
    }
}

pub(crate) fn update_params_from_form(
    project_id: &str,
    item_id: &str,
    current: &Item,
    form: &ProjectEventForm,
    tz: i32,
) -> crate::service::project_items::UpdateProjectItemParams {
    crate::service::project_items::UpdateProjectItemParams {
        project_id: project_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        description: overlay_str(&form.description, current.description.clone()),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, current.due_date()),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            current.scheduled_date(),
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            current.scheduled_end_date(),
        ),
        complete: false,
        has_due_time: Some(overlay_has_due_time(&form.due_time, current.has_due_time())),
        has_scheduled_time: Some(overlay_has_due_time(
            &form.scheduled_time,
            current.has_scheduled_time(),
        )),
        has_end_time: Some(overlay_has_due_time(
            &form.scheduled_end_time,
            current.has_end_time(),
        )),
        parent_item_id: None,
        item_type: Some(ItemKind::Event),
        event_type: overlay_str(&form.event_type, current.event_type()),
        due_offset_days: None,
        assigned_to_user_id: None,
        source_event_id: None,
        timezone_offset_minutes: Some(tz),
        points: None,
    }
}

// ---- shared rendering helpers ------------------------------------------------

pub(crate) fn render_rows(
    items: &[Item],
    project_id: &str,
    tz: i32,
    skip_urls: &HashMap<String, String>,
) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .map(|i| {
            ProjectEventRow::from_item(i, project_id, tz, skip_urls.get(&i.id).cloned()).render()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// Sane forward window for the virtual/skipped Event-series occurrences the (otherwise
/// unbounded) flat list view now shows too, as of Stage D of
/// `docs/unify-virtual-materialized-occurrences-plan.md` — matches
/// `project_dashboard::VIRTUAL_OCCURRENCE_DEFAULT_WINDOW_DAYS`'s identical rationale and
/// value; duplicated per this module's own convention of not sharing small per-screen
/// helpers (see e.g. `local_date_to_utc`'s neighboring comment) rather than importing it.
const VIRTUAL_OCCURRENCE_WINDOW_DAYS: i64 = 90;

pub(crate) fn virtual_occurrence_window(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        now,
        now + chrono::Duration::days(VIRTUAL_OCCURRENCE_WINDOW_DAYS),
    )
}

/// The Events list-view counterpart to `project_tasks::render_rows_with_virtual` — merges
/// each real top-level Event with every still-`Virtual`/`Skipped` Event-series occurrence
/// within `virtual_occurrence_window`'s forward window, sorted together by date. Unlike
/// Tasks (whose flat list only ever shows a Task-typed series' single current occurrence, by
/// construction of that series' cursor), an Event-typed series has no cursor/"current"
/// concept at all (see `ItemSeries::cursor_date`'s doc comment) — every occurrence in the
/// window is shown, exactly as the Events calendar already does, just without a month grid
/// bounding it.
pub(crate) fn render_rows_with_virtual(
    items: &[Item],
    virtual_occurrences: &[ProjectOccurrence],
    project_id: &str,
    tz: i32,
    skip_urls: &HashMap<String, String>,
) -> Result<Vec<String>, ItemError> {
    let mut entries: Vec<(i64, String)> = items
        .iter()
        .map(|i| {
            ProjectEventRow::from_item(i, project_id, tz, skip_urls.get(&i.id).cloned())
                .render()
                .map(|html| (sort_key(i), html))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for occ in virtual_occurrences {
        entries.push((
            occ.occurrence_date.timestamp(),
            ProjectEventVirtualRow::from_occurrence(occ, project_id, tz).render()?,
        ));
    }
    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

/// The calendar's per-day panel — see `project_tasks::day_list_rows`'s identical rationale.
/// Filters down to exactly `date` (a local calendar day, off `calendar_date`) and renders with
/// the same full-featured `ProjectEventRow`/`ProjectEventVirtualRow` the flat Events list uses.
pub(crate) async fn day_list_rows(
    repo: &Arc<dyn ItemRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    users: &Arc<dyn crate::storage::sqlite::UserRepo>,
    project_id: &str,
    date: NaiveDate,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    let items = list_project_events(repo, project_id).await?;
    let day_items: Vec<Item> = items
        .into_iter()
        .filter(|item| {
            calendar_date(item).map(|d| crate::web_ui::to_local(d, tz).date_naive()) == Some(date)
        })
        .collect();
    let range_start = local_date_to_utc(date, NaiveTime::from_hms_opt(0, 0, 0).unwrap(), tz);
    let range_end = local_date_to_utc(date, NaiveTime::from_hms_opt(23, 59, 59).unwrap(), tz);
    let virtual_occurrences: Vec<_> = item_series_service::list_occurrence_states_for_project(
        series,
        users,
        project_id,
        range_start,
        range_end,
        tz,
    )
    .await?
    .into_iter()
    .filter(|occ| occ.item_type == ItemKind::Event)
    .filter(|occ| {
        !matches!(
            occ.state,
            item_series_service::OccurrenceState::Materialized { .. }
        )
    })
    // See `project_tasks::day_list_rows`'s identical filter and its doc comment — defensive
    // here too, in case a future range-independent case is ever added for Event series.
    .filter(|occ| crate::web_ui::to_local(occ.occurrence_date, tz).date_naive() == date)
    .collect();
    let mut skip_urls: HashMap<String, String> = HashMap::new();
    for item in &day_items {
        if let Some(url) = item_series_service::skip_url_for_item(series, item, project_id).await?
        {
            skip_urls.insert(item.id.clone(), url);
        }
    }
    render_rows_with_virtual(&day_items, &virtual_occurrences, project_id, tz, &skip_urls)
}

/// Sort key for the events list: primary date is `scheduled_date` (falling back to
/// `due_date`), undated events last — mirrors `events::sort_key`/`team_events::sort_key`.
fn sort_key(item: &Item) -> i64 {
    item.scheduled_date()
        .or(item.due_date())
        .map(|d| d.timestamp())
        .unwrap_or(i64::MAX)
}

/// `list_project_items_unchecked` already scopes to top-level, non-Template items — this narrows
/// further to `Event` and re-sorts by the scheduled-primary key above, mirroring
/// `events::list_events`/`team_events::list_team_events`.
pub(crate) async fn list_project_events(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
) -> Result<Vec<Item>, ItemError> {
    let mut items =
        crate::service::project_items::list_project_items_unchecked(repo, project_id, None).await?;
    items.retain(|i| i.kind() == ItemKind::Event);
    items.sort_by_key(sort_key);
    Ok(items)
}

pub(crate) fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

pub(crate) fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

/// The date each event displays under: `scheduled_date` if set, else `due_date` — mirrors
/// `events::calendar_date`/`team_events::calendar_date`.
fn calendar_date(item: &Item) -> Option<DateTime<Utc>> {
    item.scheduled_date().or(item.due_date())
}

/// The first (Monday-start) cell of the 6-row grid for `year`/`month` — hoisted out of
/// `build_calendar_days` so the handler can compute the same grid's UTC date range before
/// calling it (to bound the virtual-occurrence lookup).
pub(crate) fn grid_start_for(year: i32, month: u32) -> NaiveDate {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    first_of_month - chrono::Duration::days(leading as i64)
}

/// Builds the 42-cell (6-week, Monday-start) grid for `year`/`month`, counting `items` per
/// local calendar day via `calendar_date` — mirrors `project_tasks::build_calendar_days`'s
/// identical redesign rationale (per docs/issues_and_features.md's calendar-view entry): a
/// cell only needs a tally now, the full list for a clicked day renders separately via
/// `day_list_rows`. A materialized occurrence never appears in `virtual_occurrences` — it's
/// already a real `items` row covered above (see
/// `series::list_virtual_occurrences_for_project_unchecked`).
pub(crate) fn build_calendar_days(
    year: i32,
    month: u32,
    items: &[Item],
    virtual_occurrences: &[crate::service::item_series::ProjectOccurrence],
    tz: i32,
    today: NaiveDate,
    selected_date: Option<NaiveDate>,
) -> Vec<CalendarDay> {
    let grid_start = grid_start_for(year, month);

    let mut counts: std::collections::HashMap<NaiveDate, usize> =
        std::collections::HashMap::new();
    for item in items {
        if let Some(dt) = calendar_date(item) {
            *counts
                .entry(crate::web_ui::to_local(dt, tz).date_naive())
                .or_default() += 1;
        }
    }
    for occ in virtual_occurrences {
        let local = crate::web_ui::to_local(occ.occurrence_date, tz);
        *counts.entry(local.date_naive()).or_default() += 1;
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + chrono::Duration::days(i);
        days.push(CalendarDay {
            date: date.format("%Y-%m-%d").to_string(),
            day_number: date.day(),
            is_current_month: date.month() == month && date.year() == year,
            is_today: date == today,
            is_selected: Some(date) == selected_date,
            event_count: counts.remove(&date).unwrap_or(0),
        });
    }
    days
}
