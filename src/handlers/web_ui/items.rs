use crate::auth::AuthUser;
use crate::domain::item::Item;
use crate::handlers::web_ui::TzOffset;
use crate::service::items::{self as item_service, ItemError};
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::{ItemRepo, RepoError};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, StatusCode> {
    t.render()
        .map(Html)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn repo_status(e: RepoError) -> StatusCode {
    match e {
        RepoError::NotFound => StatusCode::NOT_FOUND,
        RepoError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn service_status(e: ItemError) -> StatusCode {
    match e {
        ItemError::NotFound => StatusCode::NOT_FOUND,
        ItemError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ItemError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Every field below is `Option<String>` (never `Option<bool>`/`Option<i32>` directly) and
// parsed manually. This gives a consistent three-way read for every field, used throughout
// this file: form key absent (`None`) => this request didn't touch that field, keep whatever
// the current item already has; present but empty (`Some("")`) => explicit clear; present
// with content => set it. That distinction is what lets a single `PUT /web/items/:id`
// endpoint serve both the full edit form (every field present) and the row-level
// click-to-edit / complete-toggle interactions (exactly one field present).
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ItemForm {
    name: Option<String>,
    due_date: Option<String>,
    due_time: Option<String>,
    complete: Option<String>,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    due_offset_days: Option<String>,
    has_tasks: Option<String>,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
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

fn overlay_has_tasks(form_value: &Option<String>, current: bool) -> bool {
    match form_value.as_deref() {
        Some("simple") => false,
        Some("tasks") => true,
        _ => current,
    }
}

fn overlay_has_due_time(form_time: &Option<String>, current: bool) -> bool {
    match form_time {
        None => current,
        Some(s) => !s.trim().is_empty(),
    }
}

/// Combines a "YYYY-MM-DD" date and optional "HH:MM" time — both always local wall-clock
/// values from an `<input type="date">`/`<input type="time">`, never zone-aware — into a
/// `DateTime<Utc>`, using the same `UTC = local + tzOffsetMinutes` convention as
/// `domain::recurrence` (see its `apply_end_of_day`). No time given means "due by end of
/// that day", matching how the recurrence engine treats a plain date.
fn combine_local_to_utc(
    date: &str,
    time: Option<&str>,
    tz_offset_minutes: i32,
) -> Option<DateTime<Utc>> {
    let naive_date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let naive_time = time
        .filter(|t| !t.trim().is_empty())
        .and_then(|t| chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M").ok())
        .unwrap_or_else(|| chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap());
    let naive = naive_date.and_time(naive_time);
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Some(as_utc + chrono::Duration::minutes(tz_offset_minutes as i64))
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
        Some(s) => combine_local_to_utc(s.trim(), form_time.as_deref(), tz_offset_minutes),
    }
}

fn create_params_from_form(user_id: &str, form: &ItemForm, tz: i32) -> item_service::CreateItemParams {
    item_service::CreateItemParams {
        user_id: user_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, None),
        complete: form.complete.as_deref().map(|s| s == "true"),
        recurrence: non_empty(&form.recurrence),
        recurrence_basis: non_empty(&form.recurrence_basis),
        has_due_time: form.due_time.as_deref().map(|t| !t.trim().is_empty()),
        has_tasks: match form.has_tasks.as_deref() {
            Some("simple") => Some(false),
            Some("tasks") => Some(true),
            _ => None,
        },
        parent_item_id: non_empty(&form.parent_item_id),
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
    form: &ItemForm,
    tz: i32,
) -> item_service::UpdateItemParams {
    item_service::UpdateItemParams {
        user_id: user_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, current.due_date),
        complete: overlay_bool(&form.complete, current.complete),
        recurrence: overlay_str(&form.recurrence, current.recurrence.clone()),
        recurrence_basis: overlay_str(&form.recurrence_basis, current.recurrence_basis.clone()),
        has_due_time: Some(overlay_has_due_time(&form.due_time, current.has_due_time)),
        has_tasks: Some(overlay_has_tasks(&form.has_tasks, current.has_tasks)),
        parent_item_id: current.parent_item_id.clone(),
        due_offset_days: overlay_i32(&form.due_offset_days, current.due_offset_days),
        timezone_offset_minutes: Some(tz),
    }
}

// ---- templates --------------------------------------------------------------

#[derive(Template)]
#[template(path = "items/row.html")]
struct ItemRow {
    id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    has_children: bool,
    offset_label: Option<String>,
    recurrence: Option<String>,
    toggle_complete_json: String,
}

impl ItemRow {
    fn from_item(item: &Item) -> Self {
        let offset_label = item
            .parent_item_id
            .as_ref()
            .map(|_| match item.due_offset_days {
                Some(0) => "on due date".to_string(),
                Some(n) if n > 0 => format!("+{n}d"),
                Some(n) => format!("{n}d"),
                None => "no offset".to_string(),
            });
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date
                .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()),
            has_children: item.has_children,
            offset_label,
            recurrence: item.recurrence.clone(),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

fn format_offset_input(due_offset_days: Option<i32>) -> String {
    due_offset_days.map(|d| d.to_string()).unwrap_or_default()
}

#[derive(Template)]
#[template(path = "items/detail_fields.html")]
struct DetailFields {
    id: String,
    name: String,
    complete: bool,
    has_tasks: bool,
    is_top_level: bool,
    due_date_input: String,
    due_time_input: String,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    due_offset_days_input: String,
}

impl DetailFields {
    fn from_item(item: &Item) -> Self {
        let due_date_input = item
            .due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time {
            item.due_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            has_tasks: item.has_tasks,
            is_top_level: item.parent_item_id.is_none(),
            due_date_input,
            due_time_input,
            recurrence: item.recurrence.clone(),
            recurrence_basis: item.recurrence_basis.clone(),
            due_offset_days_input: format_offset_input(item.due_offset_days),
        }
    }
}

#[derive(Template)]
#[template(path = "items/rows_fragment.html")]
struct RowsFragmentTemplate {
    rows: Vec<String>,
    empty_message: String,
}

#[derive(Template)]
#[template(path = "items/list_page.html")]
struct ItemsListPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    blank_recurrence: Option<String>,
    blank_recurrence_basis: Option<String>,
    blank_due_offset_days_input: String,
}

#[derive(Template)]
#[template(path = "items/detail_page.html")]
struct ItemDetailPageTemplate {
    id: String,
    name: String,
    fields: String,
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(items: &[Item], show_complete: bool) -> Result<Vec<String>, StatusCode> {
    items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .map(|i| ItemRow::from_item(i).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    parent_item_id: Option<&str>,
    show_complete: bool,
) -> Result<Html<String>, StatusCode> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            repo.list_children(parent_id)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            "No sub-items yet.",
        )
    } else {
        (
            repo.list(user_id)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            "No items yet.",
        )
    };
    let rows = render_rows(&items, parent_item_id.is_some() || show_complete)?;
    render(RowsFragmentTemplate {
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

pub async fn items_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, StatusCode> {
    let show_complete = q.show_complete.is_some();
    let items = repo
        .list(&auth_user.user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = render_rows(&items, show_complete)?;
    render(ItemsListPageTemplate {
        rows,
        show_complete,
        blank_recurrence: None,
        blank_recurrence_basis: None,
        blank_due_offset_days_input: String::new(),
    })
}

pub async fn item_detail_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, StatusCode> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(repo_status)?;
    let fields = DetailFields::from_item(&item)
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    render(ItemDetailPageTemplate {
        id: item.id,
        name: item.name,
        fields,
    })
}

pub async fn children_fragment(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, StatusCode> {
    // Ownership gate: list_children itself isn't scoped by user, so confirm the caller owns
    // the parent before listing its children.
    repo.get(&auth_user.user_id, &item_id)
        .await
        .map_err(repo_status)?;
    let children = repo
        .list_children(&item_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = render_rows(&children, true)?;
    render(RowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

pub async fn create_item_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ItemForm>,
) -> Result<Html<String>, StatusCode> {
    let show_complete = form.show_complete.is_some();
    let params = create_params_from_form(&auth_user.user_id, &form, tz);
    let parent_item_id = params.parent_item_id.clone();
    item_service::create_item(&repo, params)
        .await
        .map_err(service_status)?;
    render_scope_fragment(
        &repo,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        show_complete,
    )
    .await
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchForm {
    names: String,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
}

pub async fn create_items_batch(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchForm>,
) -> Result<Html<String>, StatusCode> {
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
            timezone_offset_minutes: Some(tz),
            ..Default::default()
        };
        item_service::create_item(&repo, params)
            .await
            .map_err(service_status)?;
    }
    render_scope_fragment(
        &repo,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
    )
    .await
}

pub async fn update_item_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ItemForm>,
) -> Result<Response, StatusCode> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(repo_status)?;
    let params = update_params_from_form(&auth_user.user_id, &item_id, &current, &form, tz);
    item_service::update_item(&repo, params)
        .await
        .map_err(service_status)?;

    match repo.get(&auth_user.user_id, &item_id).await {
        Ok(updated) => {
            let row = ItemRow::from_item(&updated)
                .render()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let fields = DetailFields::from_item(&updated)
                .render()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(Html(format!("{row}{fields}")).into_response())
        }
        // The item was recurring, just got marked complete, and the service layer replaced
        // it with a fresh successor under a new id (see service::items::update_item) — there
        // is no single row/fields fragment left to swap back in under the old id, so ask the
        // client to reload rather than guessing at the new one.
        Err(RepoError::NotFound) => Ok((
            [(
                axum::http::header::HeaderName::from_static("hx-refresh"),
                "true",
            )],
            Html(String::new()),
        )
            .into_response()),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn delete_item_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, StatusCode> {
    item_service::delete_item(&repo, &auth_user.user_id, &item_id)
        .await
        .map_err(service_status)?;
    Ok(Html(String::new()))
}

pub async fn save_as_checklist(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, StatusCode> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(repo_status)?;
    template_service::create_template(
        &repo,
        CreateTemplateParams {
            user_id: auth_user.user_id.clone(),
            name: item.name.clone(),
            source_item_id: Some(item_id),
        },
    )
    .await
    .map_err(service_status)?;
    Ok(Html(
        r#"<span class="text-xs text-green-600">Saved</span>"#.to_string(),
    ))
}

