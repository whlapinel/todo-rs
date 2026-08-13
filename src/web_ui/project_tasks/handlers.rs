use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::service::templates::{self as template_service, CreateProjectTemplateParams};
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, ProjectRepo, RepoError, TeamRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_tasks::{
    active_member_options, build_calendar_days, create_params_from_form, list_project_tasks,
    names_for, next_month, non_empty, prev_month, render, render_scope_fragment, require_task,
    sibling_group, top_level_anchor_project, update_params_from_form, ProjectTaskForm,
};
use crate::web_ui::project_tasks::templates::*;
use crate::web_ui::TzOffset;
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn project_task_url(project_id: &str, item_id: &str) -> String {
    format!("/web/projects/{project_id}/tasks/{item_id}")
}

fn project_tasks_list_url(project_id: &str) -> String {
    format!("/web/projects/{project_id}/tasks")
}

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowCompleteQuery {
    show_complete: Option<String>,
}

pub async fn project_tasks_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let show_complete = q.show_complete.is_some();
    let items = list_project_tasks(&repo, &project_id).await?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let rows = super::render_rows(&items, &project_id, &names, show_complete, tz)?;
    let points_label = match &project.team_id {
        Some(team_id) => {
            let points = team_service::member_points(&teams, team_id, &auth_user.user_id).await?;
            Some(format!("{points} pts"))
        }
        None => None,
    };
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTasksListPageTemplate {
        project_id,
        rows,
        show_complete,
        points_label,
        nav_html,
    })
}

pub async fn new_project_task_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let is_team_project = project.team_id.is_some();
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(&teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(NewProjectTaskPageTemplate {
        project_id,
        show_complete: q.show_complete.is_some(),
        is_team_project,
        assignee_options,
        blank_recurrence: None,
        blank_recurrence_basis: Some("SCHEDULED_DATE".to_string()),
        blank_due_offset_days_input: String::new(),
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
        is_team_admin,
        blank_points_input: String::new(),
        nav_html,
    })
}

#[derive(serde::Deserialize)]
pub struct CalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
}

pub async fn project_tasks_calendar_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let _project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let today = crate::web_ui::to_local(Utc::now(), tz).date_naive();
    let year = q.year.unwrap_or_else(|| today.year());
    let month = q
        .month
        .filter(|m| (1..=12).contains(m))
        .unwrap_or_else(|| today.month());

    let items = list_project_tasks(&repo, &project_id).await?;
    let days = build_calendar_days(year, month, &items, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;

    render(ProjectTasksCalendarPageTemplate {
        project_id,
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

pub async fn project_task_detail_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item = repo
        .get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_task(item)?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let linked_event = resolve_linked_event(&repo, &project_id, &item).await?;
    let view = ProjectTaskDetailView::from_item(
        &item,
        &project_id,
        project.team_id.is_some(),
        &names,
        tz,
        linked_event,
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskDetailPageTemplate {
        id: item.id,
        project_id,
        name: item.name,
        complete: item.complete,
        view,
        nav_html,
    })
}

pub async fn project_task_edit_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item = repo
        .get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_task(item)?;
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(&teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    let fields = ProjectTaskDetailFields::from_item(
        &item,
        &project_id,
        project.team_id.is_some(),
        assignee_options,
        is_team_admin,
        tz,
        false,
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskEditPageTemplate {
        id: item.id,
        project_id,
        name: item.name,
        fields,
        nav_html,
    })
}

/// Renders a parent item's children as `Row`s — see `tasks::render_children_fragment`'s
/// identical rationale, project-scoped. Callers are responsible for their own membership gate
/// before calling this (see `project_task_children_fragment`).
pub(crate) async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    team_id: Option<&str>,
    parent_item_id: &str,
    requester_user_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let children = repo
        .list_children(parent_item_id)
        .await
        .map_err(ItemError::from)?;
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let rows = super::render_rows(&children, project_id, &names, true, tz)?;
    render(ProjectTaskRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

/// Renders every task that references `event_id` via `sourceEventId` as `Row`s, scoped to
/// `project_id` — the project-scoped counterpart of `tasks::render_source_event_fragment`/
/// `team_tasks::render_source_event_fragment`, called by `project_events`'s "Linked tasks"
/// section (Events have no children of their own — see `project_events::require_event`'s
/// doc comment — so there's no `project_events`-owned Task-row renderer to put this in).
pub(crate) async fn render_source_event_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    team_id: Option<&str>,
    event_id: &str,
    requester_user_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let tasks = repo
        .list_by_source_event(event_id)
        .await
        .map_err(ItemError::from)?;
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let rows = super::render_rows(&tasks, project_id, &names, true, tz)?;
    render(ProjectTaskRowsFragmentTemplate {
        rows,
        empty_message: "No linked tasks yet.".to_string(),
    })
}

pub async fn project_task_children_fragment(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    // Ownership gate: list_children isn't scoped by project, so confirm the parent actually
    // belongs to this project before listing its children (mirrors tasks.rs's equivalent).
    repo.get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    render_children_fragment(
        &repo,
        &teams,
        &project_id,
        project.team_id.as_deref(),
        &item_id,
        &auth_user.user_id,
        tz,
    )
    .await
}

/// Redirect back to the project's tasks list (via the `hx-redirect` header) after a create
/// from the standalone `/projects/:project_id/tasks/new` page. Mirrors
/// `tasks::redirect_to_tasks`/`team_tasks::redirect_to_team_tasks`.
fn redirect_to_project_tasks(project_id: &str, show_complete: bool) -> Response {
    let location = if show_complete {
        format!("/web/projects/{project_id}/tasks?showComplete=1")
    } else {
        project_tasks_list_url(project_id)
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

pub async fn create_project_task_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskForm>,
) -> Result<Response, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let show_complete = form.show_complete.is_some();
    let redirect = form.redirect.is_some();
    let params = create_params_from_form(&project_id, &form, tz);
    let parent_item_id = params.parent_item_id.clone();
    project_item_service::create_project_item(&repo, &projects, &teams, &auth_user.user_id, params)
        .await?;
    if redirect {
        return Ok(redirect_to_project_tasks(&project_id, show_complete));
    }
    Ok(render_scope_fragment(
        &repo,
        &teams,
        &project_id,
        project.team_id.as_deref(),
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

pub async fn create_project_tasks_batch(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchForm>,
) -> Result<Response, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let parent_item_id = non_empty(&form.parent_item_id);
    for line in form.names.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let params = crate::service::project_items::CreateProjectItemParams {
            project_id: project_id.clone(),
            name: name.to_string(),
            parent_item_id: parent_item_id.clone(),
            item_type: Some(ItemKind::Task),
            timezone_offset_minutes: Some(tz),
            ..Default::default()
        };
        project_item_service::create_project_item(&repo, &projects, &teams, &auth_user.user_id, params)
            .await?;
    }
    if form.redirect.is_some() {
        return Ok(redirect_to_project_tasks(
            &project_id,
            form.show_complete.is_some(),
        ));
    }
    Ok(render_scope_fragment(
        &repo,
        &teams,
        &project_id,
        project.team_id.as_deref(),
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
        tz,
    )
    .await?
    .into_response())
}

pub async fn update_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskForm>,
) -> Result<Response, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let current = repo
        .get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&project_id, &item_id, &current, &form, tz);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &auth_user.user_id,
        params,
    )
    .await?;

    match repo.get_by_project(&project_id, &item_id).await {
        Ok(updated) if close => {
            let names = match &project.team_id {
                Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
                None => HashMap::new(),
            };
            let linked_event = resolve_linked_event(&repo, &project_id, &updated).await?;
            let view = ProjectTaskDetailView::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                &names,
                tz,
                linked_event,
            )
            .render()?;
            let nav_html = nav::build_nav_html(
                &projects,
                &auth_user.user_id,
                active_context(&project_id),
                SidebarSection::Tasks,
            )
            .await?;
            Ok(render(ProjectTaskDetailPageTemplate {
                id: updated.id.clone(),
                project_id,
                name: updated.name.clone(),
                complete: updated.complete,
                view,
                nav_html,
            })?
            .into_response())
        }
        Ok(updated) => {
            let names = match &project.team_id {
                Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
                None => HashMap::new(),
            };
            let siblings =
                sibling_group(&repo, &project_id, updated.parent_item_id.as_deref()).await?;
            let siblings_ref: Vec<&Item> = siblings.iter().collect();
            let row =
                ProjectTaskRow::from_item(&updated, &project_id, &names, &siblings_ref, tz)
                    .render()?;
            let (assignee_options, is_team_admin) = match &project.team_id {
                Some(team_id) => (
                    active_member_options(&teams, team_id, &auth_user.user_id).await?,
                    project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
                ),
                None => (Vec::new(), false),
            };
            let fields = ProjectTaskDetailFields::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                assignee_options,
                is_team_admin,
                tz,
                true,
            )
            .render()?;
            let linked_event = resolve_linked_event(&repo, &project_id, &updated).await?;
            let view = ProjectTaskDetailView::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                &names,
                tz,
                linked_event,
            )
            .render()?;
            Ok(Html(format!("{row}{fields}{view}")).into_response())
        }
        // The task was recurring, just got marked complete, and the service layer replaced it
        // with a fresh successor under a new id — same situation `tasks.rs`'s
        // `update_task_form` handles.
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

pub async fn delete_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = repo
        .get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    require_task(current)?;
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    Ok(Html(String::new()))
}

/// Reparent-only update, every other field round-tripped from `current` — see
/// `tasks::reparent_params`/`team_tasks::reparent_params` for the full offset-recompute
/// rationale (identical here, just against `UpdateProjectItemParams`).
fn reparent_params(
    project_id: &str,
    item_id: &str,
    current: &Item,
    new_parent_item_id: Option<String>,
    offset_anchor: Option<DateTime<Utc>>,
    tz: i32,
) -> UpdateProjectItemParams {
    let (due_date, due_offset_days) = match (current.due_offset_days(), &new_parent_item_id) {
        (None, _) => (current.due_date(), None),
        (Some(_), Some(_)) => (
            offset_anchor.and_then(|anchor| current.deadline_from_offset(anchor, tz)),
            current.due_offset_days(),
        ),
        (Some(_), None) => (current.due_date(), None),
    };
    UpdateProjectItemParams {
        project_id: project_id.to_string(),
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
        assigned_to_user_id: current.assigned_to_user_id(),
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        points: current.points(),
    }
}

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

/// Promotes a child item to a sibling of its own parent — see `tasks::promote_task_form`.
/// Unlike the legacy screens, the redirect target is always within this same project's Tasks
/// screen (no per-kind dispatch table needed — see the plan doc's B5a note on why
/// `dashboard::detail_url`/`list_url_for` aren't reused here).
pub async fn promote_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Response, ItemError> {
    project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let current = repo
        .get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let Some(parent_id) = current.parent_item_id.clone() else {
        return Err(ItemError::Invalid(
            "item has no parent to promote from".to_string(),
        ));
    };
    let parent = repo
        .get_by_project(&project_id, &parent_id)
        .await
        .map_err(ItemError::from)?;
    let grandparent = match parent.parent_item_id {
        Some(gp_id) => Some(
            repo.get_by_project(&project_id, &gp_id)
                .await
                .map_err(ItemError::from)?,
        ),
        None => None,
    };
    let offset_anchor = match &grandparent {
        Some(gp) => top_level_anchor_project(&repo, &project_id, gp).await?,
        None => None,
    };
    let params = reparent_params(
        &project_id,
        &item_id,
        &current,
        grandparent.as_ref().map(|gp| gp.id.clone()),
        offset_anchor,
        tz,
    );
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &auth_user.user_id,
        params,
    )
    .await?;

    let location = match &grandparent {
        Some(gp) => project_task_url(&project_id, &gp.id),
        None => project_tasks_list_url(&project_id),
    };
    Ok(hx_redirect(location))
}

#[derive(serde::Deserialize, Debug)]
pub struct SubordinateForm {
    new_parent_id: String,
}

/// Subordinates a sibling to become a child of another sibling — see
/// `tasks::subordinate_task_form`.
pub async fn subordinate_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<SubordinateForm>,
) -> Result<Response, ItemError> {
    project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let current = repo
        .get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let new_parent = repo
        .get_by_project(&project_id, &form.new_parent_id)
        .await
        .map_err(ItemError::from)?;
    if new_parent.parent_item_id != current.parent_item_id {
        return Err(ItemError::Invalid(
            "target is not a sibling of this item".to_string(),
        ));
    }
    let offset_anchor = top_level_anchor_project(&repo, &project_id, &new_parent).await?;
    let params = reparent_params(
        &project_id,
        &item_id,
        &current,
        Some(new_parent.id.clone()),
        offset_anchor,
        tz,
    );
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &auth_user.user_id,
        params,
    )
    .await?;
    Ok(hx_redirect(project_task_url(&project_id, &new_parent.id)))
}

pub async fn save_project_task_as_template(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get_by_project(&project_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    template_service::create_project_template(
        &repo,
        &projects,
        &teams,
        &auth_user.user_id,
        CreateProjectTemplateParams {
            project_id,
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
