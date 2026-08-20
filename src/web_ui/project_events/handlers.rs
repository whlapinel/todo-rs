use crate::auth::AuthUser;
use crate::domain::item::ItemKind;
use crate::service::error::ItemError;
use crate::service::item_series::{self as event_series_service};
use crate::service::project_items::{self as project_item_service};
use crate::service::projects::{self as project_service};
use crate::service::templates::{self as template_service, CreateProjectTemplateParams};
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo, UserRepo};
use crate::web_ui::TzOffset;
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_events::templates::*;
use crate::web_ui::project_events::{
    ProjectEventForm, build_calendar_days, create_params_from_form, grid_start_for,
    list_project_events, local_date_to_utc, next_month, prev_month, render, require_event,
    update_params_from_form,
};
use crate::web_ui::project_tasks::handlers::render_source_event_fragment;
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

fn project_event_url(project_id: &str, item_id: &str) -> String {
    format!("/web/projects/{project_id}/events/{item_id}")
}

/// See `project_tasks::handlers::hx_redirect`'s identical rationale — duplicated per that
/// module's own precedent rather than shared.
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

pub async fn project_events_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let _project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let items = list_project_events(&repo, &project_id).await?;
    // Stage D of docs/unify-virtual-materialized-occurrences-plan.md: the flat list view
    // previously had zero virtual-occurrence support (only the calendar did) — bounded to
    // `virtual_occurrence_window` since, unlike the calendar's own month grid, this view has
    // no natural range of its own.
    let (range_start, range_end) = super::virtual_occurrence_window(Utc::now());
    let virtual_occurrences: Vec<_> = event_series_service::list_occurrence_states_for_project(
        &event_series,
        &users,
        &project_id,
        range_start,
        range_end,
        tz,
    )
    .await?
    .into_iter()
    .filter(|occ| occ.item_type == ItemKind::Event)
    .filter(|occ| {
        !matches!(
            occ.state,
            event_series_service::OccurrenceState::Materialized { .. }
        )
    })
    .collect();
    let mut skip_urls: HashMap<String, String> = HashMap::new();
    for item in &items {
        if let Some(url) =
            event_series_service::skip_url_for_item(&event_series, item, &project_id).await?
        {
            skip_urls.insert(item.id.clone(), url);
        }
    }
    let rows = super::render_rows_with_virtual(
        &items,
        &virtual_occurrences,
        &project_id,
        tz,
        &skip_urls,
    )?;
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
    let _project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let _project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
    //
    // Stage B of docs/unify-virtual-materialized-occurrences-plan.md: switched from
    // `list_virtual_occurrences_for_project_unchecked` to `list_occurrence_states_for_project`
    // so a skipped occurrence stays visible (struck through, with an Unskip button) instead
    // of disappearing outright — `Materialized` entries are excluded since those already
    // render via `items` above.
    let virtual_occurrences: Vec<_> =
        event_series_service::list_occurrence_states_for_project(
            &event_series,
            &users,
            &project_id,
            range_start,
            range_end,
            tz,
        )
        .await?
        .into_iter()
        .filter(|occ| occ.item_type == ItemKind::Event)
        .filter(|occ| {
            !matches!(
                occ.state,
                event_series_service::OccurrenceState::Materialized { .. }
            )
        })
        .collect();
    let days = build_calendar_days(
        year,
        month,
        &project_id,
        &items,
        &virtual_occurrences,
        tz,
        today,
    );
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
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
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
    let series_link = crate::web_ui::project_events::templates::resolve_series_link(
        &event_series,
        &project_id,
        &item,
    )
    .await?;
    let view = ProjectEventDetailView::from_item(&item, tz, series_link).render()?;
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

/// Stage C of `docs/unify-virtual-materialized-occurrences-plan.md` — the Events counterpart
/// of `project_tasks::handlers::render_series_occurrence_detail_page`, called by
/// `project_item_series::handlers::occurrence_detail_page` once dispatched on
/// `series.item_type == Event`. No side effect.
pub(crate) async fn render_series_occurrence_detail_page(
    projects: &Arc<dyn ProjectRepo>,
    auth_user: &AuthUser,
    project_id: &str,
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    is_skipped: bool,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let view =
        ProjectEventSeriesOccurrenceView::from_series(series, occurrence_date, project_id, is_skipped, tz)
            .render()?;
    let occurrence_ts = occurrence_date.timestamp();
    let nav_html = nav::build_nav_html(
        projects,
        &auth_user.user_id,
        active_context(project_id),
        SidebarSection::Events,
    )
    .await?;
    render(ProjectEventSeriesOccurrenceDetailPageTemplate {
        project_id: project_id.to_string(),
        name: series.name.clone(),
        is_skipped,
        view,
        child_create_url: format!(
            "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/event-children",
            series.id
        ),
        edit_url: format!(
            "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/edit",
            series.id
        ),
        nav_html,
    })
}

/// The Events counterpart of `project_tasks::handlers::render_series_occurrence_edit_page`.
pub(crate) async fn render_series_occurrence_edit_page(
    projects: &Arc<dyn ProjectRepo>,
    auth_user: &AuthUser,
    project_id: &str,
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let fields =
        ProjectEventSeriesOccurrenceFields::from_series(series, occurrence_date, project_id, tz)
            .render()?;
    let occurrence_ts = occurrence_date.timestamp();
    let nav_html = nav::build_nav_html(
        projects,
        &auth_user.user_id,
        active_context(project_id),
        SidebarSection::Events,
    )
    .await?;
    render(ProjectEventSeriesOccurrenceEditPageTemplate {
        name: series.name.clone(),
        fields,
        back_url: format!(
            "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}",
            series.id
        ),
        nav_html,
    })
}

/// Stage C — materializes the occurrence (if not already) and applies the edit in one step,
/// the PUT target for `render_series_occurrence_edit_page`'s form. Always redirects to the
/// now-real item's canonical `/events/{id}` page — see
/// `project_tasks::handlers::update_project_task_series_occurrence_form`'s identical
/// rationale.
pub async fn update_project_event_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(activity_log): Extension<Arc<dyn crate::storage::sqlite::ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectEventForm>,
) -> Result<Response, ItemError> {
    let series = event_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Event {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = event_series_service::get_or_materialize_occurrence(
        &repo,
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
        tz,
    )
    .await?;
    let params = update_params_from_form(&project_id, &item.id, &item, &form, tz);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &item_series,
        &auth_user.user_id,
        params,
    )
    .await?;
    Ok(hx_redirect(project_event_url(&project_id, &item.id)))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEventSeriesOccurrenceChildForm {
    name: String,
    due_offset_days: Option<String>,
}

/// Stage C — "adding a linked task" to a still-virtual Event occurrence: materializes it
/// first, then creates the `sourceEventId`-linked task (mirroring
/// `create_project_event_child_form`'s own shape), then redirects to the now-real event's
/// canonical `/events/{id}` page.
pub async fn create_project_event_series_occurrence_child_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectEventSeriesOccurrenceChildForm>,
) -> Result<Response, ItemError> {
    let series = event_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Event {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = event_series_service::get_or_materialize_occurrence(
        &repo,
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
        tz,
    )
    .await?;
    let params = crate::service::project_items::CreateProjectItemParams {
        project_id: project_id.clone(),
        name: form.name,
        source_event_id: Some(item.id.clone()),
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
    Ok(hx_redirect(project_event_url(&project_id, &item.id)))
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
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
    let rows = super::render_rows(&items, &project_id, tz, &HashMap::new())?;
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

    let updated =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let series_link = crate::web_ui::project_events::templates::resolve_series_link(
        &event_series,
        &project_id,
        &updated,
    )
    .await?;
    if close {
        let view = ProjectEventDetailView::from_item(&updated, tz, series_link.clone()).render()?;
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
    let skip_url =
        event_series_service::skip_url_for_item(&event_series, &updated, &project_id).await?;
    let row = ProjectEventRow::from_item(&updated, &project_id, tz, skip_url).render()?;
    let fields = ProjectEventDetailFields::from_item(&updated, &project_id, tz, true).render()?;
    let view = ProjectEventDetailView::from_item(&updated, tz, series_link).render()?;
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

pub async fn get_reschedule_event(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let event = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let event = require_event(event)?;
    render(RescheduleDialog::from_event(&event, &project_id, tz))
}
