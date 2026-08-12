pub mod templates;
pub mod handlers;

use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::web_ui::tasks::templates::{CalendarDay, CalendarTaskEntry, DateType, TaskRow, TaskRowsFragmentTemplate};
use super::dashboard::{detail_url, list_url_for};
use super::nav::{self, ActiveContext, SidebarSection};
use super::{TzOffset, to_local};
use crate::service::items::{self as item_service, ItemError, top_level_anchor};
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::sync::Arc;
use super::templates::*;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being a Task — this screen's forms hardcode
/// `itemType: TASK` on every create/update (no Kind selector, mirroring `events.rs`'s
/// `require_event`), so an Event or Simple item's id reaching one of these handlers must 404
/// rather than render a form that would silently reclassify it back to Task on save.
fn require_task(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Task {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Duplicated from `items.rs` rather than shared, matching the precedent `events.rs`/
// `team_items.rs` already set for this exact helper set.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskForm {
    name: Option<String>,
    description: Option<String>,
    due_date: Option<String>,
    due_time: Option<String>,
    scheduled_date: Option<String>,
    scheduled_time: Option<String>,
    scheduled_end_date: Option<String>,
    scheduled_end_time: Option<String>,
    complete: Option<String>,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    due_offset_days: Option<String>,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    /// Set on the standalone `/tasks/new` page's create forms (redirect to the list after
    /// creating) and on every edit form's "Save and close" submission (redirect to the item's
    /// own detail page after saving) — in both cases, "this form is done, navigate away rather
    /// than re-rendering in place." Never present on a bare checkbox PUT (those send only
    /// `complete` via `hx-vals`), so the two call sites never collide on the same field.
    redirect: Option<String>,
}

fn non_empty(v: &Option<String>) -> Option<String> {
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

fn create_params_from_form(
    user_id: &str,
    form: &TaskForm,
    tz: i32,
) -> item_service::CreateItemParams {
    item_service::CreateItemParams {
        user_id: user_id.to_string(),
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
        recurrence: non_empty(&form.recurrence),
        recurrence_basis: non_empty(&form.recurrence_basis),
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
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    }
}

fn update_params_from_form(
    user_id: &str,
    item_id: &str,
    current: &Item,
    form: &TaskForm,
    tz: i32,
) -> item_service::UpdateItemParams {
    item_service::UpdateItemParams {
        user_id: user_id.to_string(),
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
        recurrence: overlay_str(&form.recurrence, current.recurrence_pattern()),
        recurrence_basis: overlay_str(&form.recurrence_basis, current.recurrence_basis()),
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
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    }
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(items: &[Item], show_complete: bool, tz: i32) -> Result<Vec<String>, ItemError> {
    let visible: Vec<&Item> = items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .collect();
    visible
        .iter()
        .map(|i| TaskRow::from_item(i, &visible, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// `repo.list` already scopes to top-level, non-Template items — this narrows further to
/// `Task` and sorts by due date (undated tasks last), mirroring `events.rs`'s `list_events`/
/// `sort_key` pattern for its own scheduled-primary sort.
fn sort_key(item: &Item) -> i64 {
    item.due_date().map(|d| d.timestamp()).unwrap_or(i64::MAX)
}

async fn list_tasks(repo: &Arc<dyn ItemRepo>, user_id: &str) -> Result<Vec<Item>, ItemError> {
    let mut items = repo.list(user_id).await.map_err(ItemError::from)?;
    items.retain(|i| i.kind() == ItemKind::Task);
    items.sort_by_key(sort_key);
    Ok(items)
}

/// The full sibling group (including the item itself) a given item belongs to — either its
/// parent's children, or the top-level task list if it has none. Used to rebuild a single
/// row's "subordinate under…" picker (see `TaskRow`) after an in-place edit, where the caller
/// only has the one updated item on hand, not the list it was originally rendered alongside.
async fn sibling_group(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    parent_item_id: Option<&str>,
) -> Result<Vec<Item>, ItemError> {
    match parent_item_id {
        Some(pid) => repo.list_children(pid).await.map_err(ItemError::from),
        None => list_tasks(repo, user_id).await,
    }
}

fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

/// Builds the 42-cell (6-week, Monday-start) grid for `year`/`month`, bucketing `items` by
/// local calendar day off `due_date` — tasks are due-date-primary (unlike Events' scheduled-
/// window-primary `calendar_date`, see `events.rs`), so no scheduled-date fallback is needed
/// here. Same fixed 6-row layout as `events.rs`'s `build_calendar_days` for consistency
/// between the two calendar views.
fn build_calendar_days(
    year: i32,
    month: u32,
    items: &[Item],
    tz: i32,
    today: NaiveDate,
) -> Vec<CalendarDay> {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    let grid_start = first_of_month - chrono::Duration::days(leading as i64);

    let mut by_date: std::collections::HashMap<NaiveDate, Vec<CalendarTaskEntry>> =
        std::collections::HashMap::new();
    for item in items {
        if let Some(dt) = item.due_date() {
            let local = to_local(dt, tz);
            let time_label = item
                .has_due_time()
                .then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarTaskEntry {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    time_label,
                    date_type: DateType::Due,
                    has_end: item.scheduled_end_date().is_some(),
                });
        }
        if let Some(dt) = item.scheduled_date() {
            let local = to_local(dt, tz);
            let time_label = item
                .has_scheduled_time()
                .then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarTaskEntry {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    time_label,
                    date_type: DateType::ScheduledStart,
                    has_end: item.scheduled_end_date().is_some(),
                });
        }
        if let Some(dt) = item.scheduled_end_date() {
            let local = to_local(dt, tz);
            let time_label = item
                .has_scheduled_time()
                .then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarTaskEntry {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    time_label,
                    date_type: DateType::ScheduledEnd,
                    has_end: true,
                });
        }
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

async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    parent_item_id: Option<&str>,
    show_complete: bool,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            repo.list_children(parent_id)
                .await
                .map_err(ItemError::from)?,
            "No sub-items yet.",
        )
    } else {
        (list_tasks(repo, user_id).await?, "No tasks yet.")
    };
    let rows = render_rows(&items, parent_item_id.is_some() || show_complete, tz)?;
    render(TaskRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
}

// ---- handlers -----------------------------------------------------------------

