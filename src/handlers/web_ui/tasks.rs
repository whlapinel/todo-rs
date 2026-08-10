use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::handlers::web_ui::dashboard::{detail_url, list_url_for};
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::items::{self as item_service, top_level_anchor, ItemError};
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::sync::Arc;

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

// ---- templates --------------------------------------------------------------

fn recurrence_basis_label(recurrence_basis: &Option<String>) -> String {
    match recurrence_basis.as_deref() {
        Some("COMPLETION_DATE") => "completion date".to_string(),
        Some("SCHEDULED_DATE") => "scheduled date".to_string(),
        Some(other) if other != "DUE_DATE" => other.to_string(),
        _ => "due date".to_string(),
    }
}

fn offset_label_for(item: &Item) -> Option<String> {
    if !item.is_offset_driven() {
        return None;
    }
    Some(match item.due_offset_days() {
        Some(0) => "on due date".to_string(),
        Some(n) if n > 0 => format!("+{n}d"),
        Some(n) => format!("{n}d"),
        None => "no offset".to_string(),
    })
}

#[derive(Template)]
#[template(path = "tasks/row.html")]
struct TaskRow {
    id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    overdue: bool,
    scheduled_date: Option<String>,
    has_children: bool,
    offset_label: Option<String>,
    recurrence: Option<String>,
    toggle_complete_json: String,
    /// (id, name) of every other item rendered alongside this one in the same list —
    /// i.e. this item's actual siblings, since `render_rows` is only ever called with a
    /// single sibling group (a full top-level list or one parent's children) at a time.
    /// Populates the row's "subordinate under…" picker (see `subordinate_task_form`);
    /// empty for an only child / sole top-level item.
    siblings: Vec<(String, String)>,
    /// True if this task references an Event via `sourceEventId` — its row hides the
    /// "subordinate under…" picker even when siblings exist, since giving it a
    /// `parentItemId` too would conflict with the reference (see `Item::validate`).
    is_source_event_linked: bool,
}

impl TaskRow {
    fn from_item(item: &Item, siblings: &[&Item], tz: i32) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date()
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            overdue: item.is_overdue(Utc::now()),
            scheduled_date: item.scheduled_date().map(|d| {
                let local = to_local(d, tz);
                if item.has_scheduled_time() {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            has_children: item.has_children,
            offset_label: offset_label_for(item),
            recurrence: item.recurrence_pattern(),
            toggle_complete_json: (!item.complete).to_string(),
            siblings: siblings
                .iter()
                .filter(|s| s.id != item.id)
                .map(|s| (s.id.clone(), s.name.clone()))
                .collect(),
            is_source_event_linked: item.source_event_id().is_some(),
        }
    }
}

fn format_offset_input(due_offset_days: Option<i32>) -> String {
    due_offset_days.map(|d| d.to_string()).unwrap_or_default()
}

#[derive(Template)]
#[template(path = "tasks/detail_fields.html")]
struct TaskDetailFields {
    id: String,
    name: String,
    description: String,
    complete: bool,
    is_top_level: bool,
    /// True for a structural child or an event-linked task — its due date is always
    /// computed from `due_offset_days`, never manually typed (issues #21/#22), so the
    /// template swaps the free-form due-date/scheduled-window inputs for a read-only
    /// computed display and shows the offset field instead of recurrence controls.
    is_offset_driven: bool,
    due_date_input: String,
    due_time_input: String,
    scheduled_date_input: String,
    scheduled_time_input: String,
    scheduled_end_date_input: String,
    scheduled_end_time_input: String,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    due_offset_days_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    just_saved: bool,
}

impl TaskDetailFields {
    fn from_item(item: &Item, tz: i32, just_saved: bool) -> Self {
        let local_due_date = item.due_date().map(|d| to_local(d, tz));
        let due_date_input = local_due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time() {
            local_due_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_date = item.scheduled_date().map(|d| to_local(d, tz));
        let scheduled_date_input = local_scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_time_input = if item.has_scheduled_time() {
            local_scheduled_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_end_date = item.scheduled_end_date().map(|d| to_local(d, tz));
        let scheduled_end_date_input = local_scheduled_end_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_end_time_input = if item.has_end_time() {
            local_scheduled_end_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            complete: item.complete,
            is_top_level: item.parent_item_id.is_none(),
            is_offset_driven: item.is_offset_driven(),
            due_date_input,
            due_time_input,
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            recurrence: item.recurrence_pattern(),
            recurrence_basis: item.recurrence_basis(),
            due_offset_days_input: format_offset_input(item.due_offset_days()),
            just_saved,
        }
    }
}

/// Read-only counterpart to `TaskDetailFields` — see `items.rs`'s `DetailView` for the
/// row-editing convention this mirrors (complete-toggle lives here too).
#[derive(Template)]
#[template(path = "tasks/detail_view.html")]
struct TaskDetailView {
    id: String,
    description: Option<String>,
    complete: bool,
    toggle_complete_json: String,
    due_date: Option<String>,
    overdue: bool,
    scheduled_date: Option<String>,
    scheduled_end_date: Option<String>,
    is_top_level: bool,
    is_offset_driven: bool,
    recurrence: Option<String>,
    recurrence_basis_label: String,
    offset_label: Option<String>,
    /// (name, detail-page URL) of the Event this task references via `sourceEventId`,
    /// resolved by the caller (a plain lookup, since this struct's own `from_item` stays
    /// pure/repo-free like every other `*_view`/`*_fields` struct in this module).
    linked_event: Option<(String, String)>,
}

impl TaskDetailView {
    fn from_item(item: &Item, tz: i32, linked_event: Option<(String, String)>) -> Self {
        let due_date = item.due_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_due_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_date = item.scheduled_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_scheduled_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_end_date = item.scheduled_end_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_end_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        Self {
            id: item.id.clone(),
            description: item.description.clone(),
            complete: item.complete,
            toggle_complete_json: (!item.complete).to_string(),
            due_date,
            overdue: item.is_overdue(Utc::now()),
            scheduled_date,
            scheduled_end_date,
            is_top_level: item.parent_item_id.is_none(),
            is_offset_driven: item.is_offset_driven(),
            recurrence: item.recurrence_pattern(),
            recurrence_basis_label: recurrence_basis_label(&item.recurrence_basis()),
            offset_label: offset_label_for(item),
            linked_event,
        }
    }
}

/// Resolves the (name, detail-page URL) of the Event a task references via `sourceEventId`,
/// for `TaskDetailView`'s read-only "Linked to event" line. `None` if the task doesn't
/// reference one at all.
async fn resolve_linked_event(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(event_id) = item.source_event_id() else {
        return Ok(None);
    };
    let event = repo.get(user_id, &event_id).await.map_err(ItemError::from)?;
    Ok(Some((event.name.clone(), detail_url(&event))))
}

#[derive(Template)]
#[template(path = "tasks/rows_fragment.html")]
struct TaskRowsFragmentTemplate {
    rows: Vec<String>,
    empty_message: String,
}

#[derive(Template)]
#[template(path = "tasks/list_page.html")]
struct TasksListPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "tasks/new_page.html")]
struct NewTaskPageTemplate {
    show_complete: bool,
    blank_recurrence: Option<String>,
    blank_recurrence_basis: Option<String>,
    blank_due_offset_days_input: String,
    blank_scheduled_date_input: String,
    blank_scheduled_time_input: String,
    blank_scheduled_end_date_input: String,
    blank_scheduled_end_time_input: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "tasks/detail_page.html")]
struct TaskDetailPageTemplate {
    id: String,
    name: String,
    complete: bool,
    view: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "tasks/edit_page.html")]
struct TaskEditPageTemplate {
    id: String,
    name: String,
    fields: String,
    nav_html: String,
}

struct CalendarTaskEntry {
    id: String,
    name: String,
    time_label: Option<String>,
}

struct CalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    tasks: Vec<CalendarTaskEntry>,
}

#[derive(Template)]
#[template(path = "tasks/calendar_page.html")]
struct TasksCalendarPageTemplate {
    month_label: String,
    month_iso: String,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<CalendarDay>,
    nav_html: String,
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(items: &[Item], show_complete: bool, tz: i32) -> Result<Vec<String>, ItemError> {
    let visible: Vec<&Item> = items.iter().filter(|i| show_complete || !i.complete).collect();
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
    if month == 1 { (year - 1, 12) } else { (year, month - 1) }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 { (year + 1, 1) } else { (year, month + 1) }
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
            let time_label = item.has_due_time().then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarTaskEntry {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    time_label,
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowCompleteQuery {
    show_complete: Option<String>,
}

pub async fn tasks_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let show_complete = q.show_complete.is_some();
    let items = list_tasks(&repo, &auth_user.user_id).await?;
    let rows = render_rows(&items, show_complete, tz)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(TasksListPageTemplate {
        rows,
        show_complete,
        nav_html,
    })
}

pub async fn new_task_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(NewTaskPageTemplate {
        show_complete: q.show_complete.is_some(),
        blank_recurrence: None,
        blank_recurrence_basis: Some("SCHEDULED_DATE".to_string()),
        blank_due_offset_days_input: String::new(),
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
        nav_html,
    })
}

#[derive(serde::Deserialize)]
pub struct CalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
}

pub async fn tasks_calendar_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let today = to_local(Utc::now(), tz).date_naive();
    let year = q.year.unwrap_or_else(|| today.year());
    let month = q
        .month
        .filter(|m| (1..=12).contains(m))
        .unwrap_or_else(|| today.month());

    let items = list_tasks(&repo, &auth_user.user_id).await?;
    let days = build_calendar_days(year, month, &items, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;

    render(TasksCalendarPageTemplate {
        month_label: NaiveDate::from_ymd_opt(year, month, 1)
            .unwrap()
            .format("%B %Y")
            .to_string(),
        month_iso: format!("{year:04}-{month:02}"),
        prev_year,
        prev_month,
        next_year,
        next_month,
        days,
        nav_html,
    })
}

pub async fn task_detail_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_task(item)?;
    let linked_event = resolve_linked_event(&repo, &auth_user.user_id, &item).await?;
    let view = TaskDetailView::from_item(&item, tz, linked_event).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(TaskDetailPageTemplate {
        id: item.id,
        name: item.name,
        complete: item.complete,
        view,
        nav_html,
    })
}

pub async fn task_edit_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_task(item)?;
    let fields = TaskDetailFields::from_item(&item, tz, false).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(TaskEditPageTemplate {
        id: item.id,
        name: item.name,
        fields,
        nav_html,
    })
}

/// Renders a parent item's children as `TaskRow`s — children of any dedicated screen's item
/// (Task, Event, Simple) are always plain `Task`-typed, so this is reused directly by
/// `events.rs`/`team_events.rs` (which have no children concept of their own) rather than
/// duplicating `TaskRow`/its template there. Callers are responsible for their own
/// ownership gate before calling this (see `task_children_fragment` below).
pub(crate) async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    parent_item_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let children = repo
        .list_children(parent_item_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&children, true, tz)?;
    render(TaskRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

/// Renders every task that references `event_id` via `sourceEventId` as `TaskRow`s — the
/// reference-based counterpart to `render_children_fragment` above, used by an Event's own
/// "Linked tasks" section. Reuses `render_rows` (and so computes real siblings among the
/// linked tasks themselves), but that's harmless: `TaskRow`'s "Move under…" picker is hidden
/// unconditionally for a `sourceEventId`-linked row regardless of its siblings list (see
/// `TaskRow::is_source_event_linked` and `Item::validate`'s "can't have both a parent and an
/// event reference" rule) — subordinating one here would create exactly that conflict.
pub(crate) async fn render_source_event_fragment(
    repo: &Arc<dyn ItemRepo>,
    event_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let tasks = repo
        .list_by_source_event(event_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&tasks, true, tz)?;
    render(TaskRowsFragmentTemplate {
        rows,
        empty_message: "No linked tasks yet.".to_string(),
    })
}

pub async fn task_children_fragment(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    // Ownership gate: list_children itself isn't scoped by user, so confirm the caller owns
    // the parent before listing its children.
    repo.get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    render_children_fragment(&repo, &item_id, tz).await
}

/// Redirect back to the tasks list (via the `hx-redirect` header, same mechanism
/// `items.rs`'s `redirect_to_items` uses) after a create from the standalone `/tasks/new`
/// page.
fn redirect_to_tasks(show_complete: bool) -> Response {
    let location = if show_complete {
        "/web/tasks?showComplete=1".to_string()
    } else {
        "/web/tasks".to_string()
    };
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            location,
        )],
        Html(String::new()),
    )
        .into_response()
}

pub async fn create_task_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TaskForm>,
) -> Result<Response, ItemError> {
    let show_complete = form.show_complete.is_some();
    let redirect = form.redirect.is_some();
    let params = create_params_from_form(&auth_user.user_id, &form, tz);
    let parent_item_id = params.parent_item_id.clone();
    item_service::create_item(&repo, params).await?;
    if redirect {
        return Ok(redirect_to_tasks(show_complete));
    }
    Ok(render_scope_fragment(
        &repo,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        show_complete,
        tz,
    )
    .await?
    .into_response())
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchForm {
    names: String,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    redirect: Option<String>,
}

pub async fn create_tasks_batch(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchForm>,
) -> Result<Response, ItemError> {
    let parent_item_id = non_empty(&form.parent_item_id);
    for line in form.names.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let params = item_service::CreateItemParams {
            user_id: auth_user.user_id.clone(),
            name: name.to_string(),
            parent_item_id: parent_item_id.clone(),
            item_type: Some(ItemKind::Task),
            timezone_offset_minutes: Some(tz),
            ..Default::default()
        };
        item_service::create_item(&repo, params).await?;
    }
    if form.redirect.is_some() {
        return Ok(redirect_to_tasks(form.show_complete.is_some()));
    }
    Ok(render_scope_fragment(
        &repo,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
        tz,
    )
    .await?
    .into_response())
}

pub async fn update_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TaskForm>,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&auth_user.user_id, &item_id, &current, &form, tz);
    item_service::update_item(&repo, params).await?;

    match repo.get(&auth_user.user_id, &item_id).await {
        Ok(updated) if close => {
            let linked_event = resolve_linked_event(&repo, &auth_user.user_id, &updated).await?;
            let view = TaskDetailView::from_item(&updated, tz, linked_event).render()?;
            let nav_html = nav::build_nav_html(
                &team_repo,
                &auth_user.user_id,
                ActiveContext::Personal,
                SidebarSection::Tasks,
            )
            .await?;
            Ok(render(TaskDetailPageTemplate {
                id: updated.id.clone(),
                name: updated.name.clone(),
                complete: updated.complete,
                view,
                nav_html,
            })?
            .into_response())
        }
        Ok(updated) => {
            let siblings =
                sibling_group(&repo, &auth_user.user_id, updated.parent_item_id.as_deref())
                    .await?;
            let siblings_ref: Vec<&Item> = siblings.iter().collect();
            let row = TaskRow::from_item(&updated, &siblings_ref, tz).render()?;
            let fields = TaskDetailFields::from_item(&updated, tz, true).render()?;
            let linked_event = resolve_linked_event(&repo, &auth_user.user_id, &updated).await?;
            let view = TaskDetailView::from_item(&updated, tz, linked_event).render()?;
            Ok(Html(format!("{row}{fields}{view}")).into_response())
        }
        // The task was recurring, just got marked complete, and the service layer replaced
        // it with a fresh successor under a new id (see service::items::update_item) — same
        // situation `items.rs`'s `update_item_form` handles, and the same fix: ask the client
        // to reload rather than guessing at the new id.
        Err(RepoError::NotFound) => Ok((
            [(
                axum::http::header::HeaderName::from_static("hx-refresh"),
                "true",
            )],
            Html(String::new()),
        )
            .into_response()),
        Err(e) => Err(ItemError::from(e)),
    }
}

pub async fn delete_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    require_task(current)?;
    item_service::delete_item(&repo, &auth_user.user_id, &item_id).await?;
    Ok(Html(String::new()))
}

/// Builds the params for a reparent-only update — every other field round-tripped unchanged
/// from `current`, same convention `dashboard.rs`'s checkbox-toggle handlers already use for
/// their own single-field updates. Shared by `promote_task_form`/`subordinate_task_form`.
///
/// A reparent changes which top-level item a `due_offset_days`-bearing item's own due date is
/// measured from, so `due_date`/`due_offset_days` aren't simply round-tripped like everything
/// else here: `offset_anchor` (the new top-level ancestor's own anchor — see
/// `top_level_anchor`, which callers must resolve *before* calling this, since walking the
/// parent chain needs `repo` and this function is deliberately kept sync) becomes the new
/// offset root, and the due date is recomputed against it — the same `deadline_from_offset`
/// math `sync_offset_children` uses to keep a child's due date pinned to its ancestor, applied
/// once up front here since a plain reparent doesn't touch `current`'s own anchor and so would
/// never trigger `update_item`'s own automatic sync otherwise. `new_parent_item_id: None`
/// (promoting all the way to top level) clears the offset entirely — there's no longer
/// anything to offset from, and top-level items don't carry one — but leaves the item's
/// last-known due date alone as its own now-independent deadline rather than blanking it. An
/// item with no offset in the first place round-trips its due date unchanged either way,
/// matching `sync_offset_children`'s own "manually-dated child is left alone" rule. If
/// `offset_anchor` is `None` (the new top-level ancestor has no due/scheduled date of its own),
/// the due date is cleared (`None`) rather than left stale, since there's nothing left to
/// compute it against.
fn reparent_params(
    user_id: &str,
    item_id: &str,
    current: &Item,
    new_parent_item_id: Option<String>,
    offset_anchor: Option<DateTime<Utc>>,
    tz: i32,
) -> item_service::UpdateItemParams {
    let (due_date, due_offset_days) = match (current.due_offset_days(), &new_parent_item_id) {
        (None, _) => (current.due_date(), None),
        (Some(_), Some(_)) => (
            offset_anchor.and_then(|anchor| current.deadline_from_offset(anchor, tz)),
            current.due_offset_days(),
        ),
        (Some(_), None) => (current.due_date(), None),
    };
    item_service::UpdateItemParams {
        user_id: user_id.to_string(),
        item_id: item_id.to_string(),
        name: current.name.clone(),
        description: current.description.clone(),
        due_date,
        scheduled_date: current.scheduled_date(),
        scheduled_end_date: current.scheduled_end_date(),
        complete: current.complete,
        recurrence: current.recurrence_pattern(),
        recurrence_basis: current.recurrence_basis(),
        has_due_time: Some(current.has_due_time()),
        has_scheduled_time: Some(current.has_scheduled_time()),
        has_end_time: Some(current.has_end_time()),
        parent_item_id: new_parent_item_id,
        item_type: Some(current.kind()),
        event_type: current.event_type(),
        due_offset_days,
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
    }
}

/// Tells the client to navigate to `location` via a fresh GET — same `hx-redirect` mechanism
/// `redirect_to_tasks` above uses, reused here because the destination of a promote/subordinate
/// can be any of the eight dedicated screens (whichever type the new parent turns out to be),
/// not just this one — see `dashboard::detail_url`/`list_url_for`.
fn hx_redirect(location: String) -> Response {
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            location,
        )],
        Html(String::new()),
    )
        .into_response()
}

/// Promotes a child item to a sibling of its own parent (i.e. moves it up one level in the
/// hierarchy) — a no-picker convenience action since the destination is fully determined by
/// the item's current position. Rejects items that have no parent to promote from.
pub async fn promote_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let Some(parent_id) = current.parent_item_id.clone() else {
        return Err(ItemError::Invalid(
            "item has no parent to promote from".to_string(),
        ));
    };
    let parent = repo
        .get(&auth_user.user_id, &parent_id)
        .await
        .map_err(ItemError::from)?;
    let grandparent = match parent.parent_item_id {
        Some(gp_id) => Some(
            repo.get(&auth_user.user_id, &gp_id)
                .await
                .map_err(ItemError::from)?,
        ),
        None => None,
    };
    let offset_anchor = match &grandparent {
        Some(gp) => top_level_anchor(&repo, &auth_user.user_id, gp).await?,
        None => None,
    };
    let params = reparent_params(
        &auth_user.user_id,
        &item_id,
        &current,
        grandparent.as_ref().map(|gp| gp.id.clone()),
        offset_anchor,
        tz,
    );
    item_service::update_item(&repo, params).await?;

    let location = match &grandparent {
        Some(gp) => detail_url(gp),
        None => list_url_for(current.kind(), None),
    };
    Ok(hx_redirect(location))
}

#[derive(serde::Deserialize, Debug)]
pub struct SubordinateForm {
    new_parent_id: String,
}

/// Subordinates a sibling to become a child of another sibling — `new_parent_id` must
/// currently share this item's own `parent_item_id` (enforced below), so this can only ever
/// move an item within its existing sibling group, never to an arbitrary item elsewhere in
/// the tree (and, by construction, can never create a cycle — a sibling can't already be this
/// item's own ancestor or descendant).
pub async fn subordinate_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<SubordinateForm>,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let new_parent = repo
        .get(&auth_user.user_id, &form.new_parent_id)
        .await
        .map_err(ItemError::from)?;
    if new_parent.parent_item_id != current.parent_item_id {
        return Err(ItemError::Invalid(
            "target is not a sibling of this item".to_string(),
        ));
    }
    let offset_anchor = top_level_anchor(&repo, &auth_user.user_id, &new_parent).await?;
    let params = reparent_params(
        &auth_user.user_id,
        &item_id,
        &current,
        Some(new_parent.id.clone()),
        offset_anchor,
        tz,
    );
    item_service::update_item(&repo, params).await?;
    Ok(hx_redirect(detail_url(&new_parent)))
}

pub async fn save_task_as_template(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    template_service::create_template(
        &repo,
        CreateTemplateParams {
            user_id: auth_user.user_id.clone(),
            name: item.name.clone(),
            description: None,
            source_item_id: Some(item_id),
            event_type: None,
        },
    )
    .await?;
    Ok(Html(
        r#"<span class="text-xs text-green-600">Saved</span>"#.to_string(),
    ))
}
