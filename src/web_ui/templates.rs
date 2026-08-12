use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::domain::recurrence;
use super::TzOffset;
use super::nav::{self, ActiveContext, SidebarSection};
use crate::service::items::{self as item_service, ItemError};
use crate::service::templates::{self as template_service, CreateTemplateParams, UpdateTemplateParams};
use crate::storage::sqlite::{ItemRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// A plain "YYYY-MM-DD" local date (the "Use" form only ever collects a date, never a time —
/// templates carry no `hasDueTime` concept) into an end-of-local-day UTC instant, same
/// `UTC = local + tzOffsetMinutes` convention used everywhere else in `web_ui`.
fn parse_use_due_date(date: &str, tz_offset_minutes: i32) -> Option<DateTime<Utc>> {
    let naive_date = chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let naive = naive_date.and_hms_opt(0, 0, 0)?;
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Some(recurrence::apply_end_of_day(
        as_utc + chrono::Duration::minutes(tz_offset_minutes as i64),
        tz_offset_minutes,
    ))
}

// ---- templates --------------------------------------------------------------

#[derive(Template)]
#[template(path = "templates/row.html")]
struct TemplateRow {
    id: String,
    name: String,
    recurrence: Option<String>,
}

impl TemplateRow {
    fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            recurrence: item.recurrence_pattern(),
        }
    }
}

#[derive(Template)]
#[template(path = "templates/list_page.html")]
struct TemplatesListPageTemplate {
    rows: Vec<String>,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "templates/rows_fragment.html")]
struct TemplatesRowsFragmentTemplate {
    rows: Vec<String>,
}

#[derive(Template)]
#[template(path = "templates/child_row.html")]
struct TemplateChildRow {
    template_id: String,
    id: String,
    name: String,
    offset_label: String,
}

fn format_offset_label(due_offset_days: Option<i32>) -> String {
    match due_offset_days {
        Some(0) => "on due date".to_string(),
        Some(n) if n > 0 => format!("+{n}d"),
        Some(n) => format!("{n}d"),
        None => "no offset".to_string(),
    }
}

impl TemplateChildRow {
    fn from_item(template_id: &str, item: &Item) -> Self {
        Self {
            template_id: template_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            offset_label: format_offset_label(item.due_offset_days()),
        }
    }
}

#[derive(Template)]
#[template(path = "templates/children_fragment.html")]
struct ChildrenFragmentTemplate {
    rows: Vec<String>,
}

#[derive(Template)]
#[template(path = "templates/detail_page.html")]
struct TemplateDetailPageTemplate {
    id: String,
    name: String,
    description: Option<String>,
    event_type: Option<String>,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "templates/edit_page.html")]
struct TemplateEditPageTemplate {
    id: String,
    name: String,
    description: String,
    event_type: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "templates/child_detail_fields.html")]
struct TemplateChildDetailFields {
    template_id: String,
    id: String,
    name: String,
    due_offset_days_input: String,
    /// Set only on the fragment returned by a successful save (never on a plain page load) —
    /// same convention as `items::DetailFields::just_saved`.
    just_saved: bool,
}

impl TemplateChildDetailFields {
    fn from_item(template_id: &str, item: &Item, just_saved: bool) -> Self {
        Self {
            template_id: template_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            due_offset_days_input: item
                .due_offset_days()
                .map(|d| d.to_string())
                .unwrap_or_default(),
            just_saved,
        }
    }
}

/// Read-only counterpart to `TemplateChildDetailFields` — just the offset label, since
/// that's the only thing besides the name (already shown via `<h1>`) the edit form sets.
/// No complete-toggle: template children have no `complete` concept in their form at all
/// (see `update_template_child_form`, which hardcodes it to `false`).
#[derive(Template)]
#[template(path = "templates/child_detail_view.html")]
struct TemplateChildDetailView {
    id: String,
    offset_label: String,
}

impl TemplateChildDetailView {
    fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            offset_label: format_offset_label(item.due_offset_days()),
        }
    }
}

#[derive(Template)]
#[template(path = "templates/child_detail_page.html")]
struct TemplateChildDetailPageTemplate {
    template_id: String,
    id: String,
    name: String,
    view: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "templates/child_edit_page.html")]
struct TemplateChildEditPageTemplate {
    template_id: String,
    id: String,
    name: String,
    fields: String,
    nav_html: String,
}

// ---- shared rendering helpers ------------------------------------------------

fn render_template_rows(templates: &[Item]) -> Result<Vec<String>, ItemError> {
    templates
        .iter()
        .map(|i| TemplateRow::from_item(i).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

async fn render_templates_rows_fragment(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
) -> Result<Html<String>, ItemError> {
    let templates = repo.list_templates(user_id).await.map_err(ItemError::from)?;
    let rows = render_template_rows(&templates)?;
    render(TemplatesRowsFragmentTemplate { rows })
}

fn render_children_rows(template_id: &str, children: &[Item]) -> Result<Vec<String>, ItemError> {
    children
        .iter()
        .map(|i| TemplateChildRow::from_item(template_id, i).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    template_id: &str,
) -> Result<Html<String>, ItemError> {
    let children = repo.list_children(template_id).await.map_err(ItemError::from)?;
    let rows = render_children_rows(template_id, &children)?;
    render(ChildrenFragmentTemplate { rows })
}

// ---- handlers -----------------------------------------------------------------

pub async fn templates_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let templates = repo
        .list_templates(&auth_user.user_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_template_rows(&templates)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Templates,
    )
    .await?;
    render(TemplatesListPageTemplate { rows, nav_html })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTemplateForm {
    name: String,
    description: Option<String>,
    event_type: Option<String>,
}

pub async fn create_template_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Form(form): Form<CreateTemplateForm>,
) -> Result<Html<String>, ItemError> {
    let event_type = form
        .event_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let description = form
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    template_service::create_template(
        &repo,
        CreateTemplateParams {
            user_id: auth_user.user_id.clone(),
            name: form.name,
            description,
            source_item_id: None,
            event_type,
        },
    )
    .await?;
    render_templates_rows_fragment(&repo, &auth_user.user_id).await
}

pub async fn delete_template_form(
    Path(template_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    item_service::delete_item(&repo, &auth_user.user_id, &template_id).await?;
    Ok(Html(String::new()))
}

pub async fn template_detail_page(
    Path(template_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let template = repo
        .get(&auth_user.user_id, &template_id)
        .await
        .map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type();
    render(TemplateDetailPageTemplate {
        id: template.id,
        name: template.name,
        description: template.description.clone(),
        event_type,
        nav_html,
    })
}

pub async fn template_edit_page(
    Path(template_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let template = repo
        .get(&auth_user.user_id, &template_id)
        .await
        .map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type().unwrap_or_default();
    render(TemplateEditPageTemplate {
        id: template.id,
        name: template.name,
        description: template.description.clone().unwrap_or_default(),
        event_type,
        nav_html,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTemplateForm {
    name: String,
    description: Option<String>,
    event_type: Option<String>,
}

pub async fn update_template_form(
    Path(template_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<UpdateTemplateForm>,
) -> Result<Html<String>, ItemError> {
    let event_type = form
        .event_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let description = form
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    template_service::update_template(
        &repo,
        UpdateTemplateParams {
            user_id: auth_user.user_id.clone(),
            template_id: template_id.clone(),
            name: form.name.trim().to_string(),
            description,
            event_type,
        },
    )
    .await?;
    let template = repo
        .get(&auth_user.user_id, &template_id)
        .await
        .map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type();
    render(TemplateDetailPageTemplate {
        id: template.id,
        name: template.name,
        description: template.description.clone(),
        event_type,
        nav_html,
    })
}

pub async fn template_children_fragment(
    Path(template_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    // Ownership gate: list_children itself isn't scoped by user, so confirm the caller owns
    // the template before listing its items.
    repo.get(&auth_user.user_id, &template_id)
        .await
        .map_err(ItemError::from)?;
    render_children_fragment(&repo, &template_id).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateChildForm {
    name: String,
    due_offset_days: Option<String>,
    /// Set on the child edit form's "Save and close" submission — see `tasks.rs::TaskForm`'s
    /// identical field for the full rationale. Never present on the create-child form (no
    /// `<input name="redirect">` there), so `update_template_child_form` is the only reader.
    redirect: Option<String>,
}

fn parse_offset(form_value: &Option<String>) -> Option<i32> {
    form_value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

pub async fn create_template_child_form(
    Path(template_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Form(form): Form<TemplateChildForm>,
) -> Result<Html<String>, ItemError> {
    let params = item_service::CreateItemParams {
        user_id: auth_user.user_id.clone(),
        name: form.name,
        parent_item_id: Some(template_id.clone()),
        due_offset_days: parse_offset(&form.due_offset_days),
        ..Default::default()
    };
    item_service::create_item(&repo, params).await?;
    render_children_fragment(&repo, &template_id).await
}

pub async fn template_child_detail_page(
    Path((template_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let view = TemplateChildDetailView::from_item(&item).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Templates,
    )
    .await?;
    render(TemplateChildDetailPageTemplate {
        template_id,
        id: item.id,
        name: item.name,
        view,
        nav_html,
    })
}

pub async fn template_child_edit_page(
    Path((template_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let fields = TemplateChildDetailFields::from_item(&template_id, &item, false).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Templates,
    )
    .await?;
    render(TemplateChildEditPageTemplate {
        template_id,
        id: item.id,
        name: item.name,
        fields,
        nav_html,
    })
}

pub async fn update_template_child_form(
    Path((template_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<TemplateChildForm>,
) -> Result<Response, ItemError> {
    let close = form.redirect.is_some();
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let name = if form.name.trim().is_empty() {
        current.name.clone()
    } else {
        form.name.trim().to_string()
    };
    let params = item_service::UpdateItemParams {
        user_id: auth_user.user_id.clone(),
        item_id: item_id.clone(),
        name,
        description: current.description.clone(),
        due_date: None,
        scheduled_date: None,
        scheduled_end_date: None,
        complete: false,
        recurrence: None,
        recurrence_basis: None,
        has_due_time: Some(false),
        has_scheduled_time: Some(false),
        has_end_time: Some(false),
        parent_item_id: Some(template_id.clone()),
        item_type: Some(ItemKind::Task),
        event_type: current.event_type(),
        due_offset_days: parse_offset(&form.due_offset_days),
        source_event_id: None,
        timezone_offset_minutes: None,
    };
    item_service::update_item(&repo, params).await?;
    let updated = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    if close {
        let view = TemplateChildDetailView::from_item(&updated).render()?;
        let nav_html = nav::build_nav_html(
            &team_repo,
            &auth_user.user_id,
            ActiveContext::Personal,
            SidebarSection::Templates,
        )
        .await?;
        return Ok(render(TemplateChildDetailPageTemplate {
            template_id,
            id: updated.id.clone(),
            name: updated.name.clone(),
            view,
            nav_html,
        })?
        .into_response());
    }
    // Same dual-purpose response as items::update_item_form: the row half serves a row-level
    // PUT (none exist for template children today, but keeps this consistent with the
    // items.rs pattern it mirrors), the fields half serves this handler's own edit form,
    // which targets/selects only `#template-item-{id}-fields` and ignores the rest.
    let row = TemplateChildRow::from_item(&template_id, &updated).render()?;
    let fields = TemplateChildDetailFields::from_item(&template_id, &updated, true).render()?;
    Ok(Html(format!("{row}{fields}")).into_response())
}

pub async fn delete_template_child_form(
    Path((_template_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    item_service::delete_item(&repo, &auth_user.user_id, &item_id).await?;
    Ok(Html(String::new()))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UseTemplateForm {
    name: Option<String>,
    due_date: Option<String>,
}

pub async fn use_template_form(
    Path(template_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<UseTemplateForm>,
) -> Result<Response, ItemError> {
    let template = repo
        .get(&auth_user.user_id, &template_id)
        .await
        .map_err(ItemError::from)?;

    let due_date = form
        .due_date
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| parse_use_due_date(s, tz));

    let name = form
        .name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| template.name.clone());

    let params = item_service::CreateItemParams {
        user_id: auth_user.user_id.clone(),
        name,
        due_date,
        recurrence: template.recurrence_pattern(),
        recurrence_basis: template.recurrence_basis(),
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    };
    let new_item_id = item_service::create_item(&repo, params).await?;

    // Re-fetch: if the template carries its own recurrence and no due date was submitted
    // above, create_item auto-computes an initial deadline server-side — the offset root for
    // copied children must reflect that actual deadline, not the blank form input.
    let new_item = repo
        .get(&auth_user.user_id, &new_item_id)
        .await
        .map_err(ItemError::from)?;

    item_service::copy_template_children(&repo, &template_id, &new_item_id, new_item.due_date(), tz)
        .await
        .map_err(ItemError::from)?;

    Ok((
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            format!("/web/tasks/{new_item_id}"),
        )],
        Html(String::new()),
    )
        .into_response())
}
