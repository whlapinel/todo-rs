pub mod templates;
pub mod handlers;

use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::ProjectOccurrence;
use crate::service::project_items::list_project_items_unchecked;
use crate::service::teams as team_service;
use crate::storage::sqlite::{ItemRepo, TeamRepo};
use crate::web_ui::project_tasks::templates::{
    CalendarDay, CalendarTaskEntry, DateType, ProjectTaskRow, ProjectTaskRowsFragmentTemplate,
    ProjectTaskVirtualRow,
};
use askama::Template;
use axum::response::Html;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being a Task — mirrors
/// `tasks::require_task`/`team_tasks::require_team_task`.
pub(crate) fn require_task(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Task {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Duplicated from tasks/team_tasks rather than shared, matching the precedent those two
// modules already set for this exact helper set.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskForm {
    name: Option<String>,
    description: Option<String>,
    due_date: Option<String>,
    due_time: Option<String>,
    scheduled_date: Option<String>,
    scheduled_time: Option<String>,
    scheduled_end_date: Option<String>,
    scheduled_end_time: Option<String>,
    complete: Option<String>,
    due_offset_days: Option<String>,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    /// Only present/honored server-side on a team-backed project — see
    /// `service::team_items::create_team_item`/`update_team_item`'s own admin gate.
    assigned_to_user_id: Option<String>,
    /// Same team-only caveat as `assigned_to_user_id`.
    points: Option<String>,
    /// See `tasks::TaskForm`'s identical field for the redirect-vs-in-place-fragment
    /// rationale.
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

fn overlay_i32(form_value: &Option<String>, current: Option<i32>) -> Option<i32> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => s.trim().parse().ok().or(current),
    }
}

fn overlay_bool(form_value: &Option<String>, current: bool) -> bool {
    match form_value.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => current,
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
    form: &ProjectTaskForm,
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
        complete: form.complete.as_deref().map(|s| s == "true"),
        has_due_time: form.due_time.as_deref().map(|t| !t.trim().is_empty()),
        has_scheduled_time: form.scheduled_time.as_deref().map(|t| !t.trim().is_empty()),
        has_end_time: form
            .scheduled_end_time
            .as_deref()
            .map(|t| !t.trim().is_empty()),
        parent_item_id: non_empty(&form.parent_item_id),
        item_type: Some(ItemKind::Task),
        due_offset_days: form
            .due_offset_days
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        assigned_to_user_id: non_empty(&form.assigned_to_user_id),
        points: form
            .points
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    }
}

pub(crate) fn update_params_from_form(
    project_id: &str,
    item_id: &str,
    current: &Item,
    form: &ProjectTaskForm,
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
        complete: overlay_bool(&form.complete, current.complete),
        has_due_time: Some(overlay_has_due_time(&form.due_time, current.has_due_time())),
        has_scheduled_time: Some(overlay_has_due_time(
            &form.scheduled_time,
            current.has_scheduled_time(),
        )),
        has_end_time: Some(overlay_has_due_time(
            &form.scheduled_end_time,
            current.has_end_time(),
        )),
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(ItemKind::Task),
        due_offset_days: overlay_i32(&form.due_offset_days, current.due_offset_days()),
        assigned_to_user_id: overlay_str(&form.assigned_to_user_id, current.assigned_to_user_id()),
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        // No points input renders on a non-admin's/personal-project's form — `overlay_i32`
        // falls back to `current.points` when absent, mirroring `team_tasks.rs`'s identical
        // comment: a plain edit here can't silently wipe it, and the service layer's own
        // admin gate is what actually decides whether a *changed* value is honored.
        points: overlay_i32(&form.points, current.points()),
        event_type: current.event_type(),
    }
}

/// (user_id, display name) for every *active* member of `team_id` — the assignee dropdown's
/// candidate list. `None` inputs (personal project) are never called with this.
pub(crate) async fn active_member_options(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<Vec<(String, String)>, ItemError> {
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;
    Ok(members
        .into_iter()
        .filter(|m| m.status == "ACTIVE")
        .map(|m| {
            (
                m.user.id,
                format!("{} {}", m.user.first_name, m.user.last_name),
            )
        })
        .collect())
}

/// Unfiltered id -> display-name map (including inactive members), for resolving an
/// already-assigned member's name even if they've since left the team.
pub(crate) async fn names_for(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<HashMap<String, String>, ItemError> {
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;
    Ok(members
        .into_iter()
        .map(|m| {
            (
                m.user.id.clone(),
                format!("{} {}", m.user.first_name, m.user.last_name),
            )
        })
        .collect())
}

// ---- shared rendering helpers ------------------------------------------------

pub(crate) fn render_rows(
    items: &[Item],
    project_id: &str,
    names: &HashMap<String, String>,
    show_complete: bool,
    tz: i32,
    skip_urls: &HashMap<String, String>,
) -> Result<Vec<String>, ItemError> {
    let visible: Vec<&Item> = items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .collect();
    visible
        .iter()
        .map(|i| {
            ProjectTaskRow::from_item(i, project_id, names, &visible, tz, skip_urls.get(&i.id).cloned())
                .render()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// Stage 10 gap 2: the flat Tasks list's own version of `render_rows`, merging in each
/// Task-typed series' single current virtual occurrence (if any) alongside real items —
/// mirrors `project_dashboard::render_rows`'s exact merge pattern (render each kind to
/// `(timestamp, html)` pairs, concatenate, sort by timestamp, discard the timestamp). Kept
/// separate from `render_rows` rather than adding a parameter to it, since `render_rows` has
/// three other call sites in this module (children/subordinate task lists) where virtual
/// occurrences don't apply.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_rows_with_virtual(
    items: &[Item],
    virtual_occurrences: &[ProjectOccurrence],
    project_id: &str,
    names: &HashMap<String, String>,
    show_complete: bool,
    tz: i32,
    skip_urls: &HashMap<String, String>,
) -> Result<Vec<String>, ItemError> {
    let visible: Vec<&Item> = items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .collect();
    let mut entries: Vec<(i64, String)> = visible
        .iter()
        .map(|i| {
            ProjectTaskRow::from_item(i, project_id, names, &visible, tz, skip_urls.get(&i.id).cloned())
                .render()
                .map(|html| (sort_key(i), html))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for occ in virtual_occurrences {
        entries.push((
            occ.occurrence_date.timestamp(),
            ProjectTaskVirtualRow::from_occurrence(occ, project_id, tz).render()?,
        ));
    }
    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

/// `list_project_items_unchecked` already scopes to top-level, non-Template items — this narrows
/// further to `Task` and sorts by due date (undated tasks last), mirroring
/// `tasks::list_tasks`/`team_tasks::list_team_tasks`.
fn sort_key(item: &Item) -> i64 {
    item.due_date().map(|d| d.timestamp()).unwrap_or(i64::MAX)
}

pub(crate) async fn list_project_tasks(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
) -> Result<Vec<Item>, ItemError> {
    let mut items = list_project_items_unchecked(repo, project_id, None).await?;
    items.retain(|i| i.kind() == ItemKind::Task);
    items.sort_by_key(sort_key);
    Ok(items)
}

/// The full sibling group (including the item itself) a given item belongs to — see
/// `tasks::sibling_group`'s identical rationale.
pub(crate) async fn sibling_group(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    parent_item_id: Option<&str>,
) -> Result<Vec<Item>, ItemError> {
    match parent_item_id {
        Some(pid) => {
            list_project_items_unchecked(repo, project_id, Some(pid.to_string())).await
        }
        None => list_project_tasks(repo, project_id).await,
    }
}

pub(crate) async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    team_id: Option<&str>,
    requester_user_id: &str,
    parent_item_id: Option<&str>,
    show_complete: bool,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            list_project_items_unchecked(repo, project_id, Some(parent_id.to_string())).await?,
            "No sub-items yet.",
        )
    } else {
        (list_project_tasks(repo, project_id).await?, "No tasks yet.")
    };
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let rows = render_rows(
        &items,
        project_id,
        &names,
        parent_item_id.is_some() || show_complete,
        tz,
        &HashMap::new(),
    )?;
    render(ProjectTaskRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
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

/// The first (Monday-start) cell of the 6-row grid for `year`/`month` — hoisted out of
/// `build_calendar_days` so the handler can compute the same grid's UTC date range before
/// calling it (to bound the virtual-occurrence lookup), mirroring
/// `project_events::grid_start_for`/`project_dashboard::grid_start_for`.
pub(crate) fn grid_start_for(year: i32, month: u32) -> NaiveDate {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    first_of_month - chrono::Duration::days(leading as i64)
}

/// Converts a local calendar date + time-of-day into the UTC instant it represents, given
/// `tz_offset_minutes` — same convention as `project_events::local_date_to_utc`.
pub(crate) fn local_date_to_utc(date: NaiveDate, time: chrono::NaiveTime, tz_offset_minutes: i32) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(date.and_time(time), Utc)
        + chrono::Duration::minutes(tz_offset_minutes as i64)
}

/// Builds the 42-cell (6-week, Monday-start) grid for `year`/`month`, bucketing `items` by
/// local calendar day off `due_date` — mirrors `tasks::build_calendar_days` exactly (Tasks are
/// due-date-primary); team-backed projects never had a calendar view before this stage, so
/// this is a new capability for them, not a port of an existing one. `virtual_occurrences`
/// (Stage 8 of docs/recurring-events-virtual-occurrences-rough-plan.md) are bucketed into the
/// same per-day list as real tasks (`CalendarDay::tasks`) since Stage D of
/// `docs/unify-virtual-materialized-occurrences-plan.md` collapsed `CalendarTaskEntry`/
/// `CalendarVirtualTaskEntry` into one shape — see `CalendarTaskEntry`'s own doc comment.
/// Callers are expected to have already filtered `virtual_occurrences` to `item_type == Task`
/// — the past-date clamp itself (Stage 8) now lives inside
/// `item_series::list_virtual_occurrences_for_project_unchecked` (Stage 9), which also computes
/// `is_current` per occurrence, so this function no longer needs to re-derive either.
pub(crate) fn build_calendar_days(
    year: i32,
    month: u32,
    project_id: &str,
    items: &[Item],
    virtual_occurrences: &[ProjectOccurrence],
    tz: i32,
    today: NaiveDate,
) -> Vec<CalendarDay> {
    let grid_start = grid_start_for(year, month);

    let mut by_date: std::collections::HashMap<NaiveDate, Vec<CalendarTaskEntry>> =
        std::collections::HashMap::new();
    for item in items {
        let href = format!("/web/projects/{project_id}/tasks/{}", item.id);
        if let Some(dt) = item.due_date() {
            let local = crate::web_ui::to_local(dt, tz);
            let time_label = item
                .has_due_time()
                .then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarTaskEntry {
                    entry_id: format!("cal-item-{}", item.id),
                    href: href.clone(),
                    name: item.name.clone(),
                    time_label,
                    date_type: Some(DateType::Due),
                    has_end: item.scheduled_end_date().is_some(),
                    complete: item.complete,
                    materialize_url: None,
                    skip_url: None,
                    is_virtual: false,
                    is_current: false,
                    is_skipped: false,
                    unskip_url: None,
                });
        }
        if let Some(dt) = item.scheduled_date() {
            let local = crate::web_ui::to_local(dt, tz);
            let time_label = item
                .has_scheduled_time()
                .then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarTaskEntry {
                    entry_id: format!("cal-item-{}-start", item.id),
                    href: href.clone(),
                    name: item.name.clone(),
                    time_label,
                    date_type: Some(DateType::ScheduledStart),
                    has_end: item.scheduled_end_date().is_some(),
                    complete: item.complete,
                    materialize_url: None,
                    skip_url: None,
                    is_virtual: false,
                    is_current: false,
                    is_skipped: false,
                    unskip_url: None,
                });
        }
        if let Some(dt) = item.scheduled_end_date() {
            let local = crate::web_ui::to_local(dt, tz);
            let time_label = item
                .has_scheduled_time()
                .then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarTaskEntry {
                    entry_id: format!("cal-item-{}-end", item.id),
                    href: href.clone(),
                    name: item.name.clone(),
                    time_label,
                    date_type: Some(DateType::ScheduledEnd),
                    has_end: true,
                    complete: item.complete,
                    materialize_url: None,
                    skip_url: None,
                    is_virtual: false,
                    is_current: false,
                    is_skipped: false,
                    unskip_url: None,
                });
        }
    }

    for occ in virtual_occurrences {
        let local = crate::web_ui::to_local(occ.occurrence_date, tz);
        by_date
            .entry(local.date_naive())
            .or_default()
            .push(CalendarTaskEntry {
                entry_id: occ.calendar_entry_id(),
                href: "#".to_string(),
                name: occ.series_name.clone(),
                time_label: Some(local.format("%H:%M").to_string()),
                date_type: None,
                has_end: false,
                complete: false,
                materialize_url: Some(occ.materialize_url(project_id)),
                skip_url: Some(occ.skip_url(project_id)),
                is_virtual: true,
                is_current: occ.is_current,
                is_skipped: occ.is_skipped(),
                unskip_url: Some(occ.unskip_url(project_id)),
            });
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + chrono::Duration::days(i);
        let mut tasks = by_date.remove(&date).unwrap_or_default();
        tasks.sort_by(|a, b| a.time_label.cmp(&b.time_label));
        days.push(CalendarDay {
            date: date.format("%Y-%m-%d").to_string(),
            day_number: date.day(),
            is_current_month: date.month() == month && date.year() == year,
            is_today: date == today,
            tasks,
        });
    }
    days
}
