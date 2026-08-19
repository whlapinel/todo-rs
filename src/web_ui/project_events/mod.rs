pub mod templates;
pub mod handlers;

use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::storage::sqlite::ItemRepo;
use crate::web_ui::project_events::templates::{CalendarDay, CalendarEventEntry, ProjectEventRow};
use askama::Template;
use axum::response::Html;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
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
pub(crate) fn local_date_to_utc(date: NaiveDate, time: chrono::NaiveTime, tz_offset_minutes: i32) -> DateTime<Utc> {
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
) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .map(|i| ProjectEventRow::from_item(i, project_id, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
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
        crate::service::project_items::list_project_items_unchecked(repo, project_id, None)
            .await?;
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

fn calendar_has_time(item: &Item) -> bool {
    if item.scheduled_date().is_some() {
        item.has_scheduled_time()
    } else {
        item.has_due_time()
    }
}

/// The first (Monday-start) cell of the 6-row grid for `year`/`month` — hoisted out of
/// `build_calendar_days` so the handler can compute the same grid's UTC date range before
/// calling it (to bound the virtual-occurrence lookup).
pub(crate) fn grid_start_for(year: i32, month: u32) -> NaiveDate {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    first_of_month - chrono::Duration::days(leading as i64)
}

/// Builds the 42-cell (6-week, Monday-start) grid for `year`/`month`, bucketing `items` by
/// local calendar day via `calendar_date` — mirrors `events::build_calendar_days`/
/// `team_events::build_calendar_days` exactly. `virtual_occurrences` (Stage 5 of
/// docs/recurring-events-virtual-occurrences-rough-plan.md) are bucketed the same way; a
/// materialized occurrence never appears here since it's already a real `items` row covered
/// by `items` above (see `event_series::list_virtual_occurrences_for_project_unchecked`).
pub(crate) fn build_calendar_days(
    year: i32,
    month: u32,
    project_id: &str,
    items: &[Item],
    virtual_occurrences: &[crate::service::item_series::ProjectOccurrence],
    tz: i32,
    today: NaiveDate,
) -> Vec<CalendarDay> {
    let grid_start = grid_start_for(year, month);

    let mut by_date: std::collections::HashMap<NaiveDate, Vec<CalendarEventEntry>> =
        std::collections::HashMap::new();
    for item in items {
        if let Some(dt) = calendar_date(item) {
            let local = crate::web_ui::to_local(dt, tz);
            let time_label = calendar_has_time(item).then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarEventEntry {
                    entry_id: format!("cal-item-{}", item.id),
                    href: format!("/web/projects/{project_id}/events/{}", item.id),
                    name: item.name.clone(),
                    time_label,
                    materialize_url: None,
                    skip_url: None,
                    is_virtual: false,
                    is_skipped: false,
                    unskip_url: None,
                });
        }
    }
    // Stage B of docs/unify-virtual-materialized-occurrences-plan.md: callers are expected
    // to have already filtered `virtual_occurrences` to `OccurrenceState::{Virtual, Skipped}`
    // — a `Materialized` date already renders above via `items`.
    for occ in virtual_occurrences {
        let local = crate::web_ui::to_local(occ.occurrence_date, tz);
        let is_skipped = matches!(
            occ.state,
            crate::service::item_series::OccurrenceState::Skipped
        );
        by_date
            .entry(local.date_naive())
            .or_default()
            .push(CalendarEventEntry {
                entry_id: format!("cal-virtual-{}-{}", occ.series_id, occ.occurrence_date.timestamp()),
                href: "#".to_string(),
                name: occ.series_name.clone(),
                time_label: Some(local.format("%H:%M").to_string()),
                materialize_url: Some(format!(
                    "/web/projects/{project_id}/series/{}/occurrences/{}",
                    occ.series_id,
                    occ.occurrence_date.timestamp(),
                )),
                skip_url: Some(format!(
                    "/web/projects/{project_id}/series/{}/occurrences/{}/skip",
                    occ.series_id,
                    occ.occurrence_date.timestamp(),
                )),
                is_virtual: true,
                is_skipped,
                unskip_url: Some(format!(
                    "/web/projects/{project_id}/series/{}/occurrences/{}/unskip",
                    occ.series_id,
                    occ.occurrence_date.timestamp(),
                )),
            });
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + chrono::Duration::days(i);
        let mut events = by_date.remove(&date).unwrap_or_default();
        events.sort_by(|a, b| a.time_label.cmp(&b.time_label));
        days.push(CalendarDay {
            date: date.format("%Y-%m-%d").to_string(),
            day_number: date.day(),
            is_current_month: date.month() == month && date.year() == year,
            is_today: date == today,
            events,
        });
    }
    days
}
