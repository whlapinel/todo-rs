use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemType};
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::items::{self as item_service, ItemError};
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being a Task — this screen's forms hardcode
/// `itemType: TASK` on every create/update (no Kind selector, mirroring `events.rs`'s
/// `require_event`), so an Event or Simple item's id reaching one of these handlers must 404
/// rather than render a form that would silently reclassify it back to Task on save.
fn require_task(item: Item) -> Result<Item, ItemError> {
    if item.item_type == ItemType::Task {
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
    event_type: Option<String>,
    show_complete: Option<String>,
    /// Present only on the standalone `/tasks/new` page's forms — see `items.rs`'s identical
    /// field for the full rationale.
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
        item_type: Some(ItemType::Task),
        event_type: non_empty(&form.event_type),
        due_offset_days: form
            .due_offset_days
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        timezone_offset_minutes: Some(tz),
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
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, current.due_date),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            current.scheduled_date,
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            current.scheduled_end_date,
        ),
        complete: overlay_bool(&form.complete, current.complete),
        recurrence: overlay_str(&form.recurrence, current.recurrence.clone()),
        recurrence_basis: overlay_str(&form.recurrence_basis, current.recurrence_basis.clone()),
        has_due_time: Some(overlay_has_due_time(&form.due_time, current.has_due_time)),
        has_scheduled_time: Some(overlay_has_due_time(
            &form.scheduled_time,
            current.has_scheduled_time,
        )),
        has_end_time: Some(overlay_has_due_time(
            &form.scheduled_end_time,
            current.has_end_time,
        )),
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(ItemType::Task),
        event_type: overlay_str(&form.event_type, current.event_type.clone()),
        due_offset_days: overlay_i32(&form.due_offset_days, current.due_offset_days),
        timezone_offset_minutes: Some(tz),
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
    item.parent_item_id
        .as_ref()
        .map(|_| match item.due_offset_days {
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
    scheduled_date: Option<String>,
    has_children: bool,
    offset_label: Option<String>,
    recurrence: Option<String>,
    toggle_complete_json: String,
}

impl TaskRow {
    fn from_item(item: &Item, tz: i32) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            scheduled_date: item.scheduled_date.map(|d| {
                let local = to_local(d, tz);
                if item.has_scheduled_time {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            has_children: item.has_children,
            offset_label: offset_label_for(item),
            recurrence: item.recurrence.clone(),
            toggle_complete_json: (!item.complete).to_string(),
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
    complete: bool,
    is_top_level: bool,
    due_date_input: String,
    due_time_input: String,
    scheduled_date_input: String,
    scheduled_time_input: String,
    scheduled_end_date_input: String,
    scheduled_end_time_input: String,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    due_offset_days_input: String,
    event_type_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    just_saved: bool,
}

impl TaskDetailFields {
    fn from_item(item: &Item, tz: i32, just_saved: bool) -> Self {
        let local_due_date = item.due_date.map(|d| to_local(d, tz));
        let due_date_input = local_due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time {
            local_due_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_date = item.scheduled_date.map(|d| to_local(d, tz));
        let scheduled_date_input = local_scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_time_input = if item.has_scheduled_time {
            local_scheduled_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_end_date = item.scheduled_end_date.map(|d| to_local(d, tz));
        let scheduled_end_date_input = local_scheduled_end_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_end_time_input = if item.has_end_time {
            local_scheduled_end_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            is_top_level: item.parent_item_id.is_none(),
            due_date_input,
            due_time_input,
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            recurrence: item.recurrence.clone(),
            recurrence_basis: item.recurrence_basis.clone(),
            due_offset_days_input: format_offset_input(item.due_offset_days),
            event_type_input: item.event_type.clone().unwrap_or_default(),
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
    complete: bool,
    toggle_complete_json: String,
    due_date: Option<String>,
    scheduled_date: Option<String>,
    scheduled_end_date: Option<String>,
    is_top_level: bool,
    recurrence: Option<String>,
    recurrence_basis_label: String,
    offset_label: Option<String>,
    event_type: Option<String>,
}

impl TaskDetailView {
    fn from_item(item: &Item, tz: i32) -> Self {
        let due_date = item.due_date.map(|d| {
            let local = to_local(d, tz);
            if item.has_due_time {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_date = item.scheduled_date.map(|d| {
            let local = to_local(d, tz);
            if item.has_scheduled_time {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_end_date = item.scheduled_end_date.map(|d| {
            let local = to_local(d, tz);
            if item.has_end_time {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        Self {
            id: item.id.clone(),
            complete: item.complete,
            toggle_complete_json: (!item.complete).to_string(),
            due_date,
            scheduled_date,
            scheduled_end_date,
            is_top_level: item.parent_item_id.is_none(),
            recurrence: item.recurrence.clone(),
            recurrence_basis_label: recurrence_basis_label(&item.recurrence_basis),
            offset_label: offset_label_for(item),
            event_type: item.event_type.clone(),
        }
    }
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
    blank_event_type_input: String,
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

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(items: &[Item], show_complete: bool, tz: i32) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .map(|i| TaskRow::from_item(i, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// `repo.list` already scopes to top-level, non-Template items — this narrows further to
/// `Task` and sorts by due date (undated tasks last), mirroring `events.rs`'s `list_events`/
/// `sort_key` pattern for its own scheduled-primary sort.
fn sort_key(item: &Item) -> i64 {
    item.due_date.map(|d| d.timestamp()).unwrap_or(i64::MAX)
}

async fn list_tasks(repo: &Arc<dyn ItemRepo>, user_id: &str) -> Result<Vec<Item>, ItemError> {
    let mut items = repo.list(user_id).await.map_err(ItemError::from)?;
    items.retain(|i| i.item_type == ItemType::Task);
    items.sort_by_key(sort_key);
    Ok(items)
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
        blank_event_type_input: String::new(),
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
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
    let view = TaskDetailView::from_item(&item, tz).render()?;
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
    let children = repo
        .list_children(&item_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&children, true, tz)?;
    render(TaskRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
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
            item_type: Some(ItemType::Task),
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
    TzOffset(tz): TzOffset,
    Form(form): Form<TaskForm>,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let params = update_params_from_form(&auth_user.user_id, &item_id, &current, &form, tz);
    item_service::update_item(&repo, params).await?;

    match repo.get(&auth_user.user_id, &item_id).await {
        Ok(updated) => {
            let row = TaskRow::from_item(&updated, tz).render()?;
            let fields = TaskDetailFields::from_item(&updated, tz, true).render()?;
            let view = TaskDetailView::from_item(&updated, tz).render()?;
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

pub async fn save_task_as_checklist(
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
            source_item_id: Some(item_id),
            event_type: None,
        },
    )
    .await?;
    Ok(Html(
        r#"<span class="text-xs text-green-600">Saved</span>"#.to_string(),
    ))
}
