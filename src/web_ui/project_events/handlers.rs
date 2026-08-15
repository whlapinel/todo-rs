use crate::auth::AuthUser;
use crate::domain::item::ItemKind;
use crate::service::error::ItemError;
use crate::service::item_series::{self as event_series_service};
use crate::service::project_items::{self as project_item_service};
use crate::service::projects::{self as project_service};
use crate::service::templates::{self as template_service, CreateProjectTemplateParams};
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_events::{
    build_calendar_days, create_params_from_form, grid_start_for, list_project_events,
    local_date_to_utc, next_month, prev_month, render, require_event, update_params_from_form,
    ProjectEventForm,
};
use crate::web_ui::project_events::templates::*;
use crate::web_ui::project_tasks::handlers::render_source_event_fragment;
use crate::web_ui::TzOffset;
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Utc};
use std::sync::Arc;

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

pub async fn project_events_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let _project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let items = list_project_events(&repo, &project_id).await?;
    let rows = super::render_rows(&items, &project_id, tz)?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Events,
    )
    .await?;
    render(ProjectEventsListPageTemplate {
        project_id,
        rows,
        nav_html,
    })
}

pub async fn new_project_event_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let _project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Events,
    )
    .await?;
    render(NewProjectEventPageTemplate {
        project_id,
        blank_event_type_input: String::new(),
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

pub async fn project_events_calendar_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
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

    let items = list_project_events(&repo, &project_id).await?;
    let grid_start = grid_start_for(year, month);
    let range_start = local_date_to_utc(grid_start, NaiveTime::from_hms_opt(0, 0, 0).unwrap(), tz);
    let range_end = local_date_to_utc(
        grid_start + Duration::days(41),
        NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
        tz,
    );
    // Filtered to Event-typed series only (Stage 8) — Task-typed series get their own
    // equivalent surface on the Tasks calendar instead of doubling up here.
    let virtual_occurrences: Vec<_> = event_series_service::list_virtual_occurrences_for_project_unchecked(
        &event_series,
        &project_id,
        range_start,
        range_end,
        tz,
    )
    .await?
    .into_iter()
    .filter(|occ| occ.item_type == ItemKind::Event)
    .collect();
    let days = build_calendar_days(year, month, &project_id, &items, &virtual_occurrences, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Events,
    )
    .await?;

    render(ProjectEventsCalendarPageTemplate {
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

pub async fn project_event_detail_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
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
    let item = require_event(item)?;
    let view = ProjectEventDetailView::from_item(&item, tz).render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Events,
    )
    .await?;
    render(ProjectEventDetailPageTemplate {
        id: item.id,
        project_id,
        name: item.name,
        view,
        nav_html,
    })
}

pub async fn project_event_edit_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
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
    let item = require_event(item)?;
    let fields = ProjectEventDetailFields::from_item(&item, &project_id, tz, false).render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Events,
    )
    .await?;
    render(ProjectEventEditPageTemplate {
        id: item.id,
        project_id,
        name: item.name,
        fields,
        nav_html,
    })
}

/// An Event can never have structural children (see `Item::validate`/`create_project_item`'s
/// parent-kind check, delegated through to `service::items::create_item`/
/// `service::team_items::create_team_item`) — its "Linked tasks" section instead shows every
/// top-level Task that references it via `sourceEventId`, via
/// `project_tasks::render_source_event_fragment`.
pub async fn project_event_children_fragment(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    // Ownership gate (event belongs to this project) is folded into
    // `list_project_event_children_unchecked`, called inside `render_source_event_fragment`.
    render_source_event_fragment(
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEventChildForm {
    name: String,
    due_offset_days: Option<String>,
}

/// Creates a top-level Task that references this event via `sourceEventId` — not a
/// structural child (Events can never have children, see
/// `project_event_children_fragment`'s doc comment). Its `dueDate` is server-computed from
/// `dueOffsetDays` against the event's own anchor, same as a structural child's would be —
/// mirrors `events::create_event_child_form`/`team_events::create_team_event_child_form`.
pub async fn create_project_event_child_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectEventChildForm>,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let params = crate::service::project_items::CreateProjectItemParams {
        project_id: project_id.clone(),
        name: form.name,
        source_event_id: Some(item_id.clone()),
        item_type: Some(ItemKind::Task),
        due_offset_days: form
            .due_offset_days
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    };
    project_item_service::create_project_item(&repo, &projects, &teams, &auth_user.user_id, params)
        .await?;
    render_source_event_fragment(
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

/// Redirect back to the project's events list (via the `hx-redirect` header) after a create
/// from the standalone `/projects/:project_id/events/new` page. Mirrors
/// `project_tasks::handlers::redirect_to_project_tasks`.
fn redirect_to_project_events(project_id: &str) -> Response {
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            format!("/web/projects/{project_id}/events"),
        )],
        Html(String::new()),
    )
        .into_response()
}

pub async fn create_project_event_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectEventForm>,
) -> Result<Response, ItemError> {
    project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let params = create_params_from_form(&project_id, &form, tz);
    project_item_service::create_project_item(&repo, &projects, &teams, &auth_user.user_id, params)
        .await?;
    if form.redirect.is_some() {
        return Ok(redirect_to_project_events(&project_id));
    }
    let items = list_project_events(&repo, &project_id).await?;
    let rows = super::render_rows(&items, &project_id, tz)?;
    Ok(render(ProjectEventRowsFragmentTemplate {
        rows,
        empty_message: "No events yet.".to_string(),
    })?
    .into_response())
}

pub async fn update_project_event_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn crate::storage::sqlite::ActivityLogRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectEventForm>,
) -> Result<Response, ItemError> {
    let current = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let current = require_event(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&project_id, &item_id, &current, &form, tz);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &event_series,
        &auth_user.user_id,
        params,
    )
    .await?;

    let updated = project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    if close {
        let view = ProjectEventDetailView::from_item(&updated, tz).render()?;
        let nav_html = nav::build_nav_html(
            &projects,
            &auth_user.user_id,
            active_context(&project_id),
            SidebarSection::Events,
        )
        .await?;
        return Ok(render(ProjectEventDetailPageTemplate {
            id: updated.id.clone(),
            project_id,
            name: updated.name.clone(),
            view,
            nav_html,
        })?
        .into_response());
    }
    let row = ProjectEventRow::from_item(&updated, &project_id, tz).render()?;
    let fields = ProjectEventDetailFields::from_item(&updated, &project_id, tz, true).render()?;
    let view = ProjectEventDetailView::from_item(&updated, tz).render()?;
    Ok(Html(format!("{row}{fields}{view}")).into_response())
}

pub async fn delete_project_event_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    require_event(current)?;
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &event_series,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    Ok(Html(String::new()))
}

pub async fn save_project_event_as_template(
    Path((project_id, item_id)): Path<(String, String)>,
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
