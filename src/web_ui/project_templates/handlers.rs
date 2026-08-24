use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::domain::recurrence;
use crate::service::error::ItemError;
use crate::service::items as item_service;
use crate::service::project_items::{
    self as project_item_service, CreateProjectItemParams, UpdateProjectItemParams,
};
use crate::service::projects::{self as project_service};
use crate::service::templates::{
    self as template_service, CreateProjectTemplateParams, UpdateProjectTemplateParams,
};
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, ReminderRepo, TeamRepo};
use crate::web_ui::TzOffset;
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_tasks::active_member_options;
use crate::web_ui::project_templates::templates::*;
use crate::web_ui::project_templates::{non_empty, parse_offset, render, require_project_template};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::sync::Arc;

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

/// A plain "YYYY-MM-DD" local date (the "Use" form only ever collects a date, never a time —
/// templates carry no `hasDueTime` concept) into an end-of-local-day UTC instant — same
/// convention as `templates::parse_use_due_date`/`team_templates::parse_use_due_date`.
fn parse_use_due_date(date: &str, tz_offset_minutes: i32) -> Option<DateTime<Utc>> {
    let naive_date = chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let naive = naive_date.and_hms_opt(0, 0, 0)?;
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Some(recurrence::apply_end_of_day(
        as_utc + chrono::Duration::minutes(tz_offset_minutes as i64),
        tz_offset_minutes,
    ))
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(
    templates: &[Item],
    project_id: &str,
    is_team_project: bool,
    assignee_options: &[(String, String)],
) -> Result<Vec<String>, ItemError> {
    templates
        .iter()
        .map(|i| {
            ProjectTemplateRow::from_item(i, project_id, is_team_project, assignee_options.to_vec())
                .render()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

async fn render_templates_rows_fragment(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(projects, teams, project_id, requester_user_id).await?;
    let templates = template_service::list_project_templates(
        repo,
        projects,
        teams,
        project_id,
        requester_user_id,
    )
    .await?;
    let (is_team_project, assignee_options) = match &project.team_id {
        Some(team_id) => (
            true,
            active_member_options(teams, team_id, requester_user_id).await?,
        ),
        None => (false, Vec::new()),
    };
    let rows = render_rows(&templates, project_id, is_team_project, &assignee_options)?;
    render(ProjectTemplatesRowsFragmentTemplate { rows })
}

fn render_children_rows(
    project_id: &str,
    template_id: &str,
    children: &[Item],
) -> Result<Vec<String>, ItemError> {
    children
        .iter()
        .map(|i| ProjectTemplateChildRow::from_item(project_id, template_id, i).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    template_id: &str,
) -> Result<Html<String>, ItemError> {
    let children = project_item_service::list_project_items_unchecked(
        repo,
        project_id,
        Some(template_id.to_string()),
    )
    .await?;
    let rows = render_children_rows(project_id, template_id, &children)?;
    render(ProjectTemplateChildrenFragmentTemplate { rows })
}

// ---- handlers -----------------------------------------------------------------

pub async fn project_templates_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let templates = template_service::list_project_templates(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
    )
    .await?;
    let (is_team_project, assignee_options) = match &project.team_id {
        Some(team_id) => (
            true,
            active_member_options(&teams, team_id, &auth_user.user_id).await?,
        ),
        None => (false, Vec::new()),
    };
    let rows = render_rows(&templates, &project_id, is_team_project, &assignee_options)?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Templates,
    )
    .await?;
    render(ProjectTemplatesListPageTemplate {
        project_id,
        rows,
        nav_html,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectTemplateForm {
    name: String,
    description: Option<String>,
    event_type: Option<String>,
}

pub async fn create_project_template_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<CreateProjectTemplateForm>,
) -> Result<Html<String>, ItemError> {
    template_service::create_project_template(
        &repo,
        &projects,
        &teams,
        &auth_user.user_id,
        CreateProjectTemplateParams {
            project_id: project_id.clone(),
            name: form.name,
            description: non_empty(&form.description),
            source_item_id: None,
            event_type: non_empty(&form.event_type),
        },
    )
    .await?;
    render_templates_rows_fragment(&repo, &projects, &teams, &project_id, &auth_user.user_id).await
}

pub async fn delete_project_template_form(
    Path((project_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
) -> Result<Html<String>, ItemError> {
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &series,
        &reminders,
        &auth_user.user_id,
        &project_id,
        &template_id,
    )
    .await?;
    Ok(Html(String::new()))
}

pub async fn project_template_detail_page(
    Path((project_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let template = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &template_id,
    )
    .await?;
    let template = require_project_template(template)?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type();
    render(ProjectTemplateDetailPageTemplate {
        project_id,
        id: template.id,
        name: template.name,
        description: template.description.clone(),
        event_type,
        nav_html,
    })
}

pub async fn project_template_edit_page(
    Path((project_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let template = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &template_id,
    )
    .await?;
    let template = require_project_template(template)?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type().unwrap_or_default();
    render(ProjectTemplateEditPageTemplate {
        project_id,
        id: template.id,
        name: template.name,
        description: template.description.clone().unwrap_or_default(),
        event_type,
        nav_html,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectTemplateForm {
    name: String,
    description: Option<String>,
    event_type: Option<String>,
}

pub async fn update_project_template_form(
    Path((project_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<UpdateProjectTemplateForm>,
) -> Result<Html<String>, ItemError> {
    template_service::update_project_template(
        &repo,
        &projects,
        &teams,
        &auth_user.user_id,
        UpdateProjectTemplateParams {
            project_id: project_id.clone(),
            template_id: template_id.clone(),
            name: form.name.trim().to_string(),
            description: non_empty(&form.description),
            event_type: non_empty(&form.event_type),
        },
    )
    .await?;
    let template =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &template_id).await?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Templates,
    )
    .await?;
    let event_type = template.event_type();
    render(ProjectTemplateDetailPageTemplate {
        project_id,
        id: template.id,
        name: template.name,
        description: template.description.clone(),
        event_type,
        nav_html,
    })
}

pub async fn project_template_children_fragment(
    Path((project_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let template = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &template_id,
    )
    .await?;
    require_project_template(template)?;
    render_children_fragment(&repo, &project_id, &template_id).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTemplateChildForm {
    name: String,
    due_offset_days: Option<String>,
    /// Set on the child edit form's "Save and close" submission — see `tasks.rs::TaskForm`'s
    /// identical field for the full rationale. Never present on the create-child form.
    redirect: Option<String>,
}

pub async fn create_project_template_child_form(
    Path((project_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Form(form): Form<ProjectTemplateChildForm>,
) -> Result<Html<String>, ItemError> {
    let params = CreateProjectItemParams {
        project_id: project_id.clone(),
        name: form.name,
        parent_item_id: Some(template_id.clone()),
        due_offset_days: parse_offset(&form.due_offset_days),
        ..Default::default()
    };
    project_item_service::create_project_item(
        &repo,
        &projects,
        &teams,
        &reminders,
        &auth_user.user_id,
        params,
    )
    .await?;
    render_children_fragment(&repo, &project_id, &template_id).await
}

pub async fn project_template_child_detail_page(
    Path((project_id, template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let view = ProjectTemplateChildDetailView::from_item(&item).render()?;
    let dialog = ProjectTemplateChildDetailDialog::new(
        &project_id,
        &template_id,
        &item.id,
        &item.name,
        view,
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Templates,
    )
    .await?;
    render(ProjectTemplateChildDetailPageTemplate {
        name: item.name,
        dialog,
        nav_html,
    })
}

pub async fn project_template_child_edit_page(
    Path((project_id, template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let fields =
        ProjectTemplateChildDetailFields::from_item(&project_id, &template_id, &item, false)
            .render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Templates,
    )
    .await?;
    render(ProjectTemplateChildEditPageTemplate {
        name: item.name,
        fields,
        nav_html,
    })
}

pub async fn update_project_template_child_form(
    Path((project_id, template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn crate::storage::sqlite::ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Form(form): Form<ProjectTemplateChildForm>,
) -> Result<Response, ItemError> {
    let close = form.redirect.is_some();
    let current = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let name = if form.name.trim().is_empty() {
        current.name.clone()
    } else {
        form.name.trim().to_string()
    };
    let params = UpdateProjectItemParams {
        project_id: project_id.clone(),
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
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &reminders,
        &auth_user.user_id,
        params,
    )
    .await?;
    let updated =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    if close {
        let view = ProjectTemplateChildDetailView::from_item(&updated).render()?;
        let dialog = ProjectTemplateChildDetailDialog::new(
            &project_id,
            &template_id,
            &updated.id,
            &updated.name,
            view,
        )
        .render()?;
        let nav_html = nav::build_nav_html(
            &projects,
            &auth_user.user_id,
            active_context(&project_id),
            SidebarSection::Templates,
        )
        .await?;
        return Ok(render(ProjectTemplateChildDetailPageTemplate {
            name: updated.name.clone(),
            dialog,
            nav_html,
        })?
        .into_response());
    }
    let row = ProjectTemplateChildRow::from_item(&project_id, &template_id, &updated).render()?;
    let fields =
        ProjectTemplateChildDetailFields::from_item(&project_id, &template_id, &updated, true)
            .render()?;
    Ok(Html(format!("{row}{fields}")).into_response())
}

pub async fn delete_project_template_child_form(
    Path((project_id, _template_id, item_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
) -> Result<Html<String>, ItemError> {
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &series,
        &reminders,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    Ok(Html(String::new()))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UseProjectTemplateForm {
    name: Option<String>,
    due_date: Option<String>,
    assigned_to_user_id: Option<String>,
}

pub async fn use_project_template_form(
    Path((project_id, template_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<UseProjectTemplateForm>,
) -> Result<Response, ItemError> {
    let template = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &template_id,
    )
    .await?;

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

    // Dropped by `create_project_item` on the personal branch (`CreateItemParams` has no
    // slot for it) — harmless to always pass through, same as `project_tasks`'s own
    // create/update paths do for `assignedToUserId`/`points`.
    let assigned_to_user_id = form.assigned_to_user_id.filter(|s| !s.is_empty());

    let params = CreateProjectItemParams {
        project_id: project_id.clone(),
        name,
        due_date,
        assigned_to_user_id,
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    };
    let new_item_id = project_item_service::create_project_item(
        &repo,
        &projects,
        &teams,
        &reminders,
        &auth_user.user_id,
        params,
    )
    .await?;

    // Re-fetch the created item so the offset root for copied children reflects its actual
    // persisted due date, not just the raw form input (e.g. `has_due_time` normalization).
    let new_item =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &new_item_id).await?;

    item_service::copy_template_children(
        &repo,
        &template_id,
        &new_item_id,
        new_item.due_date(),
        tz,
    )
    .await
    .map_err(ItemError::from)?;

    Ok((
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            format!("/web/projects/{project_id}/tasks/{new_item_id}"),
        )],
        Html(String::new()),
    )
        .into_response())
}
