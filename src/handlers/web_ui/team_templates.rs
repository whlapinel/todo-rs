use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::domain::recurrence;
use crate::handlers::web_ui::TzOffset;
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::service::error::ItemError;
use crate::service::items::{self as item_service};
use crate::service::team_items::{self as team_item_service, require_active_member, CreateTeamItemParams, UpdateTeamItemContext, UpdateTeamItemParams};
use crate::service::teams as team_service;
use crate::service::templates::{self as template_service, CreateTeamTemplateParams, UpdateTeamTemplateParams};
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// A plain "YYYY-MM-DD" local date into an end-of-local-day UTC instant — same convention
/// as `templates::parse_use_due_date`.
fn parse_use_due_date(date: &str, tz_offset_minutes: i32) -> Option<DateTime<Utc>> {
    let naive_date = chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let naive = naive_date.and_hms_opt(0, 0, 0)?;
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Some(recurrence::apply_end_of_day(
        as_utc + chrono::Duration::minutes(tz_offset_minutes as i64),
        tz_offset_minutes,
    ))
}

async fn active_member_options(
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

// ---- templates --------------------------------------------------------------

#[derive(Template)]
#[template(path = "team_templates/row.html")]
struct TeamTemplateRow {
    team_id: String,
    id: String,
    name: String,
    recurrence: Option<String>,
    assignee_options: Vec<(String, String)>,
}

impl TeamTemplateRow {
    fn from_item(team_id: &str, item: &Item, assignee_options: Vec<(String, String)>) -> Self {
        Self {
            team_id: team_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            recurrence: item.recurrence_pattern(),
            assignee_options,
        }
    }
}

#[derive(Template)]
#[template(path = "team_templates/list_page.html")]
struct TeamTemplatesListPageTemplate {
    team_id: String,
    rows: Vec<String>,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_templates/rows_fragment.html")]
struct TeamTemplatesRowsFragmentTemplate {
    rows: Vec<String>,
}

#[derive(Template)]
#[template(path = "team_templates/child_row.html")]
struct TeamTemplateChildRow {
    team_id: String,
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

impl TeamTemplateChildRow {
    fn from_item(team_id: &str, template_id: &str, item: &Item) -> Self {
        Self {
            team_id: team_id.to_string(),
            template_id: template_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            offset_label: format_offset_label(item.due_offset_days()),
        }
    }
}

#[derive(Template)]
#[template(path = "team_templates/children_fragment.html")]
struct ChildrenFragmentTemplate {
    rows: Vec<String>,
}

#[derive(Template)]
#[template(path = "team_templates/detail_page.html")]
struct TeamTemplateDetailPageTemplate {
    team_id: String,
    id: String,
    name: String,
    description: Option<String>,
    event_type: Option<String>,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_templates/edit_page.html")]
struct TeamTemplateEditPageTemplate {
    team_id: String,
    id: String,
    name: String,
    description: String,
    event_type: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_templates/child_detail_fields.html")]
struct TeamTemplateChildDetailFields {
    team_id: String,
    template_id: String,
    id: String,
    name: String,
    due_offset_days_input: String,
    just_saved: bool,
}

impl TeamTemplateChildDetailFields {
    fn from_item(team_id: &str, template_id: &str, item: &Item, just_saved: bool) -> Self {
        Self {
            team_id: team_id.to_string(),
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

#[derive(Template)]
#[template(path = "team_templates/child_detail_view.html")]
struct TeamTemplateChildDetailView {
    id: String,
    offset_label: String,
}

impl TeamTemplateChildDetailView {
    fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            offset_label: format_offset_label(item.due_offset_days()),
        }
    }
}

#[derive(Template)]
#[template(path = "team_templates/child_detail_page.html")]
struct TeamTemplateChildDetailPageTemplate {
    team_id: String,
    template_id: String,
    id: String,
    name: String,
    view: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_templates/child_edit_page.html")]
struct TeamTemplateChildEditPageTemplate {
    team_id: String,
    template_id: String,
    id: String,
    name: String,
    fields: String,
    nav_html: String,
}

// ---- shared rendering helpers ------------------------------------------------

async fn render_team_template_rows(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
    templates: &[Item],
) -> Result<Vec<String>, ItemError> {
    let assignee_options = active_member_options(teams, team_id, requester_user_id).await?;
    templates
        .iter()
        .map(|i| TeamTemplateRow::from_item(team_id, i, assignee_options.clone()).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

async fn render_team_templates_rows_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<Html<String>, ItemError> {
    let templates = repo.list_team_templates(team_id).await.map_err(ItemError::from)?;
    let rows = render_team_template_rows(teams, team_id, requester_user_id, &templates).await?;
    render(TeamTemplatesRowsFragmentTemplate { rows })
}

fn render_children_rows(team_id: &str, template_id: &str, children: &[Item]) -> Result<Vec<String>, ItemError> {
    children
        .iter()
        .map(|i| TeamTemplateChildRow::from_item(team_id, template_id, i).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    team_id: &str,
    template_id: &str,
) -> Result<Html<String>, ItemError> {
    let children = repo.list_children(template_id).await.map_err(ItemError::from)?;
    let rows = render_children_rows(team_id, template_id, &children)?;
    render(ChildrenFragmentTemplate { rows })
}

// ---- handlers -----------------------------------------------------------------

pub async fn team_templates_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let templates = repo.list_team_templates(&team_id).await.map_err(ItemError::from)?;
    let rows = render_team_template_rows(&teams, &team_id, &auth_user.user_id, &templates).await?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Templates,
    )
    .await?;
    render(TeamTemplatesListPageTemplate { team_id, rows, nav_html })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamTemplateForm {
    name: String,
    description: Option<String>,
    event_type: Option<String>,
}

pub async fn create_team_template_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<CreateTeamTemplateForm>,
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
    template_service::create_team_template(
        &repo,
        &teams,
        CreateTeamTemplateParams {
            team_id: team_id.clone(),
            requester_user_id: auth_user.user_id.clone(),
            name: form.name,
            description,
            source_item_id: None,
            event_type,
        },
    )
    .await?;
    render_team_templates_rows_fragment(&repo, &teams, &team_id, &auth_user.user_id).await
}

pub async fn delete_team_template_form(
    Path((team_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    team_item_service::delete_team_item(&repo, &teams, &auth_user.user_id, &team_id, &template_id).await?;
    Ok(Html(String::new()))
}

pub async fn team_template_detail_page(
    Path((team_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let template = repo.get_team_item(&team_id, &template_id).await.map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type();
    render(TeamTemplateDetailPageTemplate {
        team_id,
        id: template.id,
        name: template.name,
        description: template.description.clone(),
        event_type,
        nav_html,
    })
}

pub async fn team_template_edit_page(
    Path((team_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let template = repo.get_team_item(&team_id, &template_id).await.map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type().unwrap_or_default();
    render(TeamTemplateEditPageTemplate {
        team_id,
        id: template.id,
        name: template.name,
        description: template.description.clone().unwrap_or_default(),
        event_type,
        nav_html,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamTemplateForm {
    name: String,
    description: Option<String>,
    event_type: Option<String>,
}

pub async fn update_team_template_form(
    Path((team_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<UpdateTeamTemplateForm>,
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
    template_service::update_team_template(
        &repo,
        &teams,
        UpdateTeamTemplateParams {
            team_id: team_id.clone(),
            requester_user_id: auth_user.user_id.clone(),
            template_id: template_id.clone(),
            name: form.name.trim().to_string(),
            description,
            event_type,
        },
    )
    .await?;
    let template = repo.get_team_item(&team_id, &template_id).await.map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type();
    render(TeamTemplateDetailPageTemplate {
        team_id,
        id: template.id,
        name: template.name,
        description: template.description.clone(),
        event_type,
        nav_html,
    })
}

pub async fn team_template_children_fragment(
    Path((team_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    repo.get_team_item(&team_id, &template_id).await.map_err(ItemError::from)?;
    render_children_fragment(&repo, &team_id, &template_id).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamTemplateChildForm {
    name: String,
    due_offset_days: Option<String>,
    /// Set on the child edit form's "Save and close" submission — see `tasks.rs::TaskForm`'s
    /// identical field for the full rationale. Never present on the create-child form.
    redirect: Option<String>,
}

fn parse_offset(form_value: &Option<String>) -> Option<i32> {
    form_value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

pub async fn create_team_template_child_form(
    Path((team_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<TeamTemplateChildForm>,
) -> Result<Html<String>, ItemError> {
    let params = CreateTeamItemParams {
        team_id: team_id.clone(),
        name: form.name,
        parent_item_id: Some(template_id.clone()),
        due_offset_days: parse_offset(&form.due_offset_days),
        ..Default::default()
    };
    team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;
    render_children_fragment(&repo, &team_id, &template_id).await
}

pub async fn team_template_child_detail_page(
    Path((team_id, template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let item = repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    let view = TeamTemplateChildDetailView::from_item(&item).render()?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Templates,
    )
    .await?;
    render(TeamTemplateChildDetailPageTemplate {
        team_id,
        template_id,
        id: item.id,
        name: item.name,
        view,
        nav_html,
    })
}

pub async fn team_template_child_edit_page(
    Path((team_id, template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let item = repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    let fields = TeamTemplateChildDetailFields::from_item(&team_id, &template_id, &item, false).render()?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Templates,
    )
    .await?;
    render(TeamTemplateChildEditPageTemplate {
        team_id,
        template_id,
        id: item.id,
        name: item.name,
        fields,
        nav_html,
    })
}

pub async fn update_team_template_child_form(
    Path((team_id, template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Form(form): Form<TeamTemplateChildForm>,
) -> Result<Response, ItemError> {
    let close = form.redirect.is_some();
    let current = repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    let name = if form.name.trim().is_empty() {
        current.name.clone()
    } else {
        form.name.trim().to_string()
    };
    let params = UpdateTeamItemParams {
        team_id: team_id.clone(),
        item_id: item_id.clone(),
        name,
        description: current.description.clone(),
        complete: false,
        parent_item_id: Some(template_id.clone()),
        item_type: Some(ItemKind::Task),
        event_type: current.event_type(),
        due_offset_days: parse_offset(&form.due_offset_days),
        ..Default::default()
    };
    team_item_service::update_team_item(
        &repo,
        &UpdateTeamItemContext { teams: teams.clone(), activity_log },
        &auth_user.user_id,
        params,
    )
    .await?;
    let updated = repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    if close {
        let view = TeamTemplateChildDetailView::from_item(&updated).render()?;
        let nav_html = nav::build_nav_html(
            &teams,
            &auth_user.user_id,
            ActiveContext::Team(team_id.clone()),
            SidebarSection::Templates,
        )
        .await?;
        return Ok(render(TeamTemplateChildDetailPageTemplate {
            team_id,
            template_id,
            id: updated.id.clone(),
            name: updated.name.clone(),
            view,
            nav_html,
        })?
        .into_response());
    }
    let row = TeamTemplateChildRow::from_item(&team_id, &template_id, &updated).render()?;
    let fields = TeamTemplateChildDetailFields::from_item(&team_id, &template_id, &updated, true).render()?;
    Ok(Html(format!("{row}{fields}")).into_response())
}

pub async fn delete_team_template_child_form(
    Path((team_id, _template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    team_item_service::delete_team_item(&repo, &teams, &auth_user.user_id, &team_id, &item_id).await?;
    Ok(Html(String::new()))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UseTeamTemplateForm {
    name: Option<String>,
    due_date: Option<String>,
    assigned_to_user_id: Option<String>,
}

pub async fn use_team_template_form(
    Path((team_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<UseTeamTemplateForm>,
) -> Result<Response, ItemError> {
    let template = repo.get_team_item(&team_id, &template_id).await.map_err(ItemError::from)?;

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

    let assigned_to_user_id = form
        .assigned_to_user_id
        .filter(|s| !s.is_empty());

    let params = CreateTeamItemParams {
        team_id: team_id.clone(),
        name,
        due_date,
        recurrence: template.recurrence_pattern(),
        recurrence_basis: template.recurrence_basis(),
        assigned_to_user_id,
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    };
    let new_item_id = team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;

    let new_item = repo.get_team_item(&team_id, &new_item_id).await.map_err(ItemError::from)?;

    item_service::copy_template_children(&repo, &template_id, &new_item_id, new_item.due_date(), tz)
        .await
        .map_err(ItemError::from)?;

    Ok((
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            format!("/web/team-tasks/{team_id}/{new_item_id}"),
        )],
        Html(String::new()),
    )
        .into_response())
}
