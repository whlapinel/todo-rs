use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{self as item_series_service};
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::service::templates::{self as template_service, CreateProjectTemplateParams};
use crate::storage::sqlite::{
    ActivityLogRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo, UserRepo,
};
use crate::web_ui::TzOffset;
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_tasks::templates::*;
use crate::web_ui::project_tasks::{
    ProjectTaskForm, active_member_options, build_calendar_days, create_params_from_form,
    grid_start_for, list_project_tasks, local_date_to_utc, names_for, next_month, non_empty,
    prev_month, render, render_scope_fragment, require_task, sibling_group,
    update_params_from_form,
};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc};
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
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let show_complete = q.show_complete.is_some();
    let items = list_project_tasks(&repo, &project_id).await?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    // Stage 10 gap 2: a Task series' current occurrence is range-independent (a pure
    // function of the cursor), so any degenerate range surfaces it — see
    // `list_virtual_occurrences_for_project_unchecked`'s backlog-exemption logic (which
    // `list_occurrence_states_for_project` mirrors — see its own doc comment).
    //
    // Stage B of docs/unify-virtual-materialized-occurrences-plan.md switched this from
    // `list_virtual_occurrences_for_project_unchecked` to `list_occurrence_states_for_project`
    // purely for the shared `ProjectOccurrence` type `ProjectTaskVirtualRow` now takes — the
    // `is_current` filter below means this view's actual visible behavior is unchanged: by
    // construction (`require_current_occurrence`'s self-heal, see `service::item_series`) the
    // series' current occurrence is never itself materialized or skipped, so the trailing
    // `!Materialized` filter never has anything to exclude here in practice. A just-skipped
    // occurrence stops being current the instant it's settled, so it (correctly) has nothing
    // to show in this current-only view — see the calendar/dashboard views for where a
    // skipped occurrence's struck-through Unskip row actually appears.
    let virtual_occurrences: Vec<_> = item_series_service::list_occurrence_states_for_project(
        &series,
        &users,
        &project_id,
        Utc::now(),
        Utc::now(),
        tz,
    )
    .await?
    .into_iter()
    .filter(|occ| occ.item_type == ItemKind::Task && occ.is_current)
    .filter(|occ| {
        !matches!(
            occ.state,
            item_series_service::OccurrenceState::Materialized { .. }
        )
    })
    .collect();
    let mut skip_urls: HashMap<String, String> = HashMap::new();
    for item in &items {
        if let Some(url) =
            item_series_service::skip_url_for_item(&series, item, &project_id).await?
        {
            skip_urls.insert(item.id.clone(), url);
        }
    }
    let rows = super::render_rows_with_virtual(
        &items,
        &virtual_occurrences,
        &project_id,
        &names,
        show_complete,
        tz,
        &skip_urls,
        project.team_id.as_deref(),
    )?;
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
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
        blank_due_date_input: String::new(),
        blank_due_time_input: String::new(),
        is_team_admin,
        blank_points_input: String::new(),
        nav_html,
    })
}

#[derive(serde::Deserialize)]
pub struct CalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
    /// `YYYY-MM-DD` — the day currently selected in the per-day panel below the grid. See
    /// `ProjectTasksCalendarPageTemplate::selected_date`'s doc comment.
    date: Option<String>,
}

/// "Friday, August 21, 2026" — shared by the full page and the `.../calendar/day` fragment
/// route so the panel heading is identical regardless of which one rendered it.
fn day_panel_label(date: NaiveDate) -> String {
    date.format("%A, %B %d, %Y").to_string()
}

pub async fn project_tasks_calendar_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let today = crate::web_ui::to_local(Utc::now(), tz).date_naive();
    let year = q.year.unwrap_or_else(|| today.year());
    let month = q
        .month
        .filter(|m| (1..=12).contains(m))
        .unwrap_or_else(|| today.month());
    let selected_date = q
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());

    let items = list_project_tasks(&repo, &project_id).await?;
    let grid_start = grid_start_for(year, month);
    let range_start = local_date_to_utc(grid_start, NaiveTime::from_hms_opt(0, 0, 0).unwrap(), tz);
    let range_end = local_date_to_utc(
        grid_start + Duration::days(41),
        NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
        tz,
    );
    // Filtered to Task-typed series only (Event-typed have their own home on the Events
    // calendar) — the past-date clamp (Stage 8) and `is_current` computation (Stage 9) now
    // live inside `list_occurrence_states_for_project` itself (mirroring
    // `list_virtual_occurrences_for_project_unchecked`'s own logic — see its doc comment).
    //
    // Stage B of docs/unify-virtual-materialized-occurrences-plan.md: unlike the flat list
    // above, the calendar shows a real date range, so a skipped occurrence within it stays
    // visible (struck through, with an Unskip button) rather than disappearing — hence the
    // switch from `list_virtual_occurrences_for_project_unchecked` (which drops any already-
    // settled date outright). Materialized dates are excluded — those already render via
    // `items` above.
    let virtual_occurrences: Vec<_> = item_series_service::list_occurrence_states_for_project(
        &series,
        &users,
        &project_id,
        range_start,
        range_end,
        tz,
    )
    .await?
    .into_iter()
    .filter(|occ| occ.item_type == ItemKind::Task)
    .filter(|occ| {
        !matches!(
            occ.state,
            item_series_service::OccurrenceState::Materialized { .. }
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
        selected_date,
    );
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    let day_rows = match selected_date {
        Some(date) => {
            super::day_list_rows(
                &repo,
                &series,
                &users,
                &teams,
                &project_id,
                project.team_id.as_deref(),
                &auth_user.user_id,
                date,
                tz,
            )
            .await?
        }
        None => Vec::new(),
    };

    render(ProjectTasksCalendarPageTemplate {
        project_id,
        month_label: NaiveDate::from_ymd_opt(year, month, 1)
            .unwrap()
            .format("%B %Y")
            .to_string(),
        month_iso: format!("{year:04}-{month:02}"),
        year,
        month,
        prev_year,
        prev_month,
        next_year,
        next_month,
        days,
        selected_date_label: selected_date.map(day_panel_label),
        day_rows,
        nav_html,
    })
}

pub async fn project_tasks_calendar_day_fragment(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let date = q
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or(ItemError::Invalid("date is required".to_string()))?;
    let day_rows = super::day_list_rows(
        &repo,
        &series,
        &users,
        &teams,
        &project_id,
        project.team_id.as_deref(),
        &auth_user.user_id,
        date,
        tz,
    )
    .await?;
    render(ProjectTasksCalendarDayPanelTemplate {
        selected_date_label: Some(day_panel_label(date)),
        day_rows,
    })
}

pub async fn project_task_detail_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let item = require_task(item)?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let linked_event = resolve_linked_event(&repo, &project_id, &item).await?;
    let series_link = resolve_series_link(&series, &project_id, &item).await?;
    let view = ProjectTaskDetailView::from_item(
        &item,
        &project_id,
        project.team_id.is_some(),
        &names,
        tz,
        linked_event,
        series_link,
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
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
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

/// Stage C of `docs/unify-virtual-materialized-occurrences-plan.md` — the Task-flavored half
/// of the deferred-materialization detail page, called by
/// `project_item_series::handlers::occurrence_detail_page` once it's dispatched on
/// `series.item_type == Task` and confirmed the occurrence isn't already materialized (that
/// case redirects to the real `/tasks/{id}` page instead of calling this — see that
/// function's doc comment). Renders read-only, with no side effect: never calls
/// `get_or_materialize_occurrence`.
pub(crate) async fn render_series_occurrence_detail_page(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    item_series: &Arc<dyn ItemSeriesRepo>,
    auth_user: &AuthUser,
    project_id: &str,
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    is_skipped: bool,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(projects, teams, project_id, &auth_user.user_id).await?;
    let names = match &project.team_id {
        Some(team_id) => names_for(teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let is_current = current_occurrence_is(series, occurrence_date, tz)?;
    // Stage 4 of docs/assignment-rotation-plan.md: this occurrence's own resolved
    // assignee (fixed, or this calendar position's rotation member) — not the series'
    // raw `assigned_to_user_id`, which is `None` for a rotating series.
    let resolved_assignee_id =
        item_series_service::resolve_occurrence_assignee(item_series, series, occurrence_date, tz)
            .await?;
    let view = ProjectTaskSeriesOccurrenceView::from_series(
        series,
        occurrence_date,
        project_id,
        project.team_id.is_some(),
        &names,
        resolved_assignee_id,
        is_skipped,
        is_current,
        tz,
    )
    .render()?;
    let occurrence_ts = occurrence_date.timestamp();
    let nav_html = nav::build_nav_html(
        projects,
        &auth_user.user_id,
        active_context(project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskSeriesOccurrenceDetailPageTemplate {
        project_id: project_id.to_string(),
        name: series.name.clone(),
        is_skipped,
        view,
        child_create_url: format!(
            "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/task-children",
            series.id
        ),
        edit_url: format!(
            "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/edit",
            series.id
        ),
        nav_html,
    })
}

/// The Task-flavored half of the deferred-materialization edit page — see
/// `render_series_occurrence_detail_page`'s doc comment for the dispatch this is called from.
/// Also no side effect: prefilled from `series`/`occurrence_date` directly, not a real `Item`.
pub(crate) async fn render_series_occurrence_edit_page(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    item_series: &Arc<dyn ItemSeriesRepo>,
    auth_user: &AuthUser,
    project_id: &str,
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(projects, teams, project_id, &auth_user.user_id).await?;
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(projects, teams, project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    // Stage 4 of docs/assignment-rotation-plan.md — prefill the select with this
    // occurrence's actually-resolved assignee (fixed, or this calendar position's
    // rotation member), not the series' raw `assigned_to_user_id`. Without this, an
    // unmodified Save on a rotating occurrence's edit form would silently overwrite the
    // just-materialized correct assignee with "Unassigned" (`overlay_str` always applies
    // whatever the select submits — see `update_params_from_form`).
    let resolved_assignee_id =
        item_series_service::resolve_occurrence_assignee(item_series, series, occurrence_date, tz)
            .await?;
    let fields = ProjectTaskSeriesOccurrenceFields::from_series(
        series,
        occurrence_date,
        project_id,
        project.team_id.is_some(),
        assignee_options,
        resolved_assignee_id,
        is_team_admin,
        tz,
    )
    .render()?;
    let occurrence_ts = occurrence_date.timestamp();
    let nav_html = nav::build_nav_html(
        projects,
        &auth_user.user_id,
        active_context(project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskSeriesOccurrenceEditPageTemplate {
        name: series.name.clone(),
        fields,
        back_url: format!(
            "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}",
            series.id
        ),
        nav_html,
    })
}

/// Whether `occurrence_date` is `series`'s current occurrence — same check
/// `service::item_series::require_current_occurrence` makes internally, duplicated here
/// (rather than exposed from that module) since it's purely a display concern on this page,
/// not a mutation gate. `Event`-typed series have no cursor/current concept, so always
/// `false` there — this is only ever called from the Task-flavored render path above anyway.
fn current_occurrence_is(
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<bool, ItemError> {
    let rule = crate::domain::recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
    Ok(
        item_series_service::current_occurrence_date(series, &rule, tz_offset_minutes)
            == occurrence_date,
    )
}

/// Stage C — materializes the occurrence (if not already) and applies the edit in one step,
/// the shared PUT target for both the checkbox on
/// `render_series_occurrence_detail_page`'s view (`hx-vals='{"complete": "true"}'`) and the
/// full edit form on `render_series_occurrence_edit_page` (same `ProjectTaskForm` shape either
/// way — a checkbox toggle is just a form submission with only `complete` set). Always
/// redirects to the now-real item's canonical `/tasks/{id}` page — there's no meaningful
/// in-place fragment to return to once the surrounding page's whole premise (a still-virtual
/// occurrence) has changed.
pub async fn update_project_task_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskForm>,
) -> Result<Response, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Task {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = item_series_service::get_or_materialize_occurrence(
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
    Ok(hx_redirect(project_task_url(&project_id, &item.id)))
}

/// See `project_item_series::handlers::redirect_to_current_page`'s identical rationale —
/// duplicated per that module's own precedent rather than shared.
fn redirect_to_current_page(headers: &HeaderMap, project_id: &str) -> Response {
    let location = headers
        .get("hx-current-url")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| project_tasks_list_url(project_id));
    hx_redirect(location)
}

/// The row-checkbox counterpart to Skip/Unskip (`project_item_series::handlers`) — completes a
/// Task-series occurrence directly from a list/dashboard row's checkbox, whether it's still
/// virtual or already materialized doesn't matter to the caller: materializes it first if
/// needed (`get_or_materialize_occurrence`, a no-op if already materialized), then completes it
/// via the exact same `update_project_item` path a real item's own checkbox already uses — so
/// cursor validation (`item_series::require_current_occurrence`), points, and activity logging
/// all apply identically. Task-typed series only — `Item::validate` rejects `complete: true`
/// for Events, so this route is never wired onto an Event occurrence's row. Redirects back to
/// the calling screen like Skip/Unskip, rather than to the item's own detail page — a row
/// checkbox click should behave like any other row checkbox, not navigate away.
pub async fn complete_project_item_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
    headers: HeaderMap,
) -> Result<Response, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Task {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = item_series_service::get_or_materialize_occurrence(
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
    let form = ProjectTaskForm {
        complete: Some("true".to_string()),
        ..Default::default()
    };
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
    Ok(redirect_to_current_page(&headers, &project_id))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskSeriesOccurrenceChildForm {
    name: String,
    due_offset_days: Option<String>,
}

/// Stage C — "adding a sub-item" to a still-virtual occurrence: materializes it first, then
/// creates the child underneath the resulting real item, then redirects to that item's
/// canonical `/tasks/{id}` page (mirrors `update_project_task_series_occurrence_form`'s own
/// redirect rationale — there's nothing left to render in place of the virtual page).
pub async fn create_project_task_series_occurrence_child_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskSeriesOccurrenceChildForm>,
) -> Result<Response, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Task {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = item_series_service::get_or_materialize_occurrence(
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
        parent_item_id: Some(item.id.clone()),
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
    Ok(hx_redirect(project_task_url(&project_id, &item.id)))
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
    let children = project_item_service::list_project_items_unchecked(
        repo,
        project_id,
        Some(parent_item_id.to_string()),
    )
    .await?;
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let rows = super::render_rows(
        &children,
        project_id,
        &names,
        true,
        tz,
        &HashMap::new(),
        team_id,
    )?;
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
    let tasks =
        project_item_service::list_project_event_children_unchecked(repo, project_id, event_id)
            .await?;
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let rows = super::render_rows(
        &tasks,
        project_id,
        &names,
        true,
        tz,
        &HashMap::new(),
        team_id,
    )?;
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
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    // Ownership gate: confirm the parent actually belongs to this project before listing its
    // children (mirrors tasks.rs's equivalent).
    project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
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
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
        project_item_service::create_project_item(
            &repo,
            &projects,
            &teams,
            &auth_user.user_id,
            params,
        )
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
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskForm>,
) -> Result<Response, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let current =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let current = require_task(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&project_id, &item_id, &current, &form, tz);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &auth_user.user_id,
        params,
    )
    .await?;

    match project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await {
        Ok(updated) if close => {
            let names = match &project.team_id {
                Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
                None => HashMap::new(),
            };
            let linked_event = resolve_linked_event(&repo, &project_id, &updated).await?;
            let series_link = resolve_series_link(&series, &project_id, &updated).await?;
            let view = ProjectTaskDetailView::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                &names,
                tz,
                linked_event,
                series_link,
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
            let skip_url =
                item_series_service::skip_url_for_item(&series, &updated, &project_id).await?;
            let row = ProjectTaskRow::from_item(
                &updated,
                &project_id,
                &names,
                &siblings_ref,
                tz,
                skip_url,
                project.team_id.is_some(),
            )
            .render()?;
            let (assignee_options, is_team_admin) = match &project.team_id {
                Some(team_id) => (
                    active_member_options(&teams, team_id, &auth_user.user_id).await?,
                    project_service::is_project_admin(
                        &projects,
                        &teams,
                        &project_id,
                        &auth_user.user_id,
                    )
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
            let series_link = resolve_series_link(&series, &project_id, &updated).await?;
            let view = ProjectTaskDetailView::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                &names,
                tz,
                linked_event,
                series_link,
            )
            .render()?;
            Ok(Html(format!("{row}{fields}{view}")).into_response())
        }
        // The task was recurring, just got marked complete, and the service layer replaced it
        // with a fresh successor under a new id — same situation `tasks.rs`'s
        // `update_task_form` handles.
        Err(ItemError::NotFound) => Ok((
            [(
                axum::http::header::HeaderName::from_static("hx-refresh"),
                "true",
            )],
            Html(String::new()),
        )
            .into_response()),
        Err(e) => Err(e),
    }
}

pub async fn delete_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
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
    require_task(current)?;
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &series,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    Ok(Html(String::new()))
}

pub async fn duplicate_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Response, ItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    require_task(item)?;
    project_item_service::duplicate_project_item(
        &repo,
        &projects,
        &teams,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    let location = project_tasks_list_url(&project_id);
    Ok(hx_redirect(location))
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
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Response, ItemError> {
    let target = project_item_service::resolve_promotion_target(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let current = require_task(target.current)?;
    let grandparent = target.grandparent;
    let params = reparent_params(
        &project_id,
        &item_id,
        &current,
        grandparent.as_ref().map(|gp| gp.id.clone()),
        target.offset_anchor,
        tz,
    );
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
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
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<SubordinateForm>,
) -> Result<Response, ItemError> {
    let target = project_item_service::resolve_subordination_target(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
        &form.new_parent_id,
    )
    .await?;
    let current = require_task(target.current)?;
    let new_parent = target.new_parent;
    let params = reparent_params(
        &project_id,
        &item_id,
        &current,
        Some(new_parent.id.clone()),
        target.offset_anchor,
        tz,
    );
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
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

pub async fn get_reschedule_task(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let task = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let task = require_task(task)?;
    render(RescheduleDialog::from_task(&task, &project_id, tz))
}

pub async fn get_quick_assign_task(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let task = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let task = require_task(task)?;
    let assignee_options = match &project.team_id {
        Some(team_id) => active_member_options(&teams, team_id, &auth_user.user_id).await?,
        None => Vec::new(),
    };
    render(QuickAssignDialog::from_task(
        &task,
        &project_id,
        assignee_options,
    ))
}
