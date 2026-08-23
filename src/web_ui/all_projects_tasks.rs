use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{
    self as item_series_service, OccurrenceState, ProjectOccurrence,
};
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects as project_service;
use crate::storage::sqlite::{
    ActivityLogRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo, UserRepo,
};
use crate::web_ui::TzOffset;
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_tasks::names_for;
use crate::web_ui::project_tasks::templates::ProjectTaskRow;
use crate::web_ui::to_local;
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::Html;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Cross-project Task-only counterpart to `main_calendar::is_included` — this screen never
/// carries Events/Simple/Template rows at all (the per-project gather loop below already
/// filters to `ItemKind::Task`), so unlike that function this only needs the one case: a Task
/// is unrestricted on a personal project (single member, "assigned to me" is moot) and
/// restricted to the requester's own assignment on a team-backed one — otherwise this screen
/// would show every team member's tasks, defeating its "what's mine, across every project"
/// purpose. See `docs/all-projects-landing-plan.md` Stage 2: no `assignedToAny` toggle yet
/// (deferred — this screen has no such control today, matching the flat calendar list's own
/// pre-Stage-4 behavior).
fn task_included(is_team_project: bool, assigned_to: Option<&str>, user_id: &str) -> bool {
    !is_team_project || assigned_to == Some(user_id)
}

/// Cross-project counterpart to `project_tasks::templates::ProjectTaskVirtualRow` — a Task
/// series' current occurrence, tagged with the project it belongs to (mirrors
/// `main_calendar::MainCalendarVirtualRow`, minus the type symbol/label since this list is
/// Task-only already).
#[derive(Template)]
#[template(path = "all_projects_tasks/virtual_row.html")]
struct AllProjectsTaskVirtualRow {
    series_id: String,
    occurrence_ts: i64,
    project_name: String,
    name: String,
    date_label: String,
    is_due_date_basis: bool,
    overdue: bool,
    materialize_url: String,
    skip_url: String,
    complete_url: Option<String>,
    is_current: bool,
    assignee_name: Option<String>,
    is_skipped: bool,
    unskip_url: String,
}

impl AllProjectsTaskVirtualRow {
    /// `show_complete` bakes a `?view=all-tasks[&showComplete=1]` suffix onto the
    /// checkbox/Skip/Unskip URLs, mirroring `ProjectTaskVirtualRow::from_occurrence`'s own
    /// `list_query` — this screen is always the flat list (no calendar-day-panel equivalent, so
    /// no `in_list_view` gate is needed), letting `project_tasks::handlers::
    /// complete_project_item_series_occurrence_form`'s `"all-tasks"` branch (and
    /// `project_item_series::handlers`' skip/unskip `"all-tasks"` branch,
    /// `docs/all-projects-landing-plan.md` Stage 4) rebuild `#items-list` in place.
    fn from_occurrence(
        occ: &ProjectOccurrence,
        project_id: &str,
        project_name: &str,
        tz: i32,
        show_complete: bool,
    ) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        let list_query = if show_complete {
            "?view=all-tasks&showComplete=1"
        } else {
            "?view=all-tasks"
        };
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            project_name: project_name.to_string(),
            name: occ.series_name.clone(),
            date_label: local.format("%Y-%m-%d %H:%M").to_string(),
            is_due_date_basis: occ.is_due_date_basis,
            overdue: occ.is_due_date_basis && occ.occurrence_date < Utc::now(),
            materialize_url: occ.materialize_url(project_id),
            skip_url: format!("{}{list_query}", occ.skip_url(project_id)),
            complete_url: occ
                .is_current
                .then(|| format!("{}{list_query}", occ.complete_url(project_id))),
            is_current: occ.is_current,
            assignee_name: occ.assigned_to_user_name.clone(),
            is_skipped: occ.is_skipped(),
            unskip_url: format!("{}{list_query}", occ.unskip_url(project_id)),
        }
    }
}

/// Builds a real Task item's row for this screen — `ProjectTaskRow::from_item` plus the same
/// `project_name`/URL overrides `main_calendar::calendar_row` applies, minus the type badge
/// (single-kind screen) and parent name (this screen only ever lists top-level items — see
/// `all_projects_tasks_page` — children stay reachable via the row's own existing "expand"
/// affordance, unaffected by any of this). `complete_url` points at this module's own
/// `toggle_all_projects_task_complete` route rather than the per-project one, so its response
/// can re-render via this same function; `reschedule_url`/`assign_url` carry a `?view=all-tasks`
/// suffix, now recognized by `project_tasks::normalize_row_view`
/// (`docs/all-projects-landing-plan.md` Stage 4) — `update_project_task_form`'s `"all-tasks"` arm
/// calls this same function to re-render a Reschedule/Assign save from this screen. `confirmation`/
/// `dismiss_after_ms` mirror `project_calendar::calendar_row`/`main_calendar::calendar_row`'s own
/// trailing params, threaded through by `list_all_projects_task_rows`'s `just_completed_item_id`
/// and by `project_tasks::handlers::complete_project_item_series_occurrence_form`'s `"all-tasks"`
/// rebuild branch.
#[allow(clippy::too_many_arguments)]
pub(crate) fn all_projects_task_row(
    item: &Item,
    project_id: &str,
    project_name: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    tz: i32,
    skip_url: Option<String>,
    show_complete: bool,
    confirmation: Option<String>,
    dismiss_after_ms: Option<u32>,
) -> Result<String, ItemError> {
    let mut row = ProjectTaskRow::from_item(
        item,
        project_id,
        names,
        &[],
        tz,
        skip_url,
        is_team_project,
        show_complete,
        confirmation,
        dismiss_after_ms,
    );
    row.expanded_row = true;
    row.project_name = Some(project_name.to_string());
    row.complete_url = row
        .complete_url
        .as_ref()
        .map(|_| format!("/web/tasks/projects/{project_id}/items/{}", item.id));
    row.reschedule_url = row
        .reschedule_url
        .map(|url| format!("{url}?view=all-tasks"));
    row.assign_url = row.assign_url.map(|url| format!("{url}?view=all-tasks"));
    Ok(row.render()?)
}

#[derive(Template)]
#[template(path = "all_projects_tasks/list_page.html")]
struct AllProjectsTasksListPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    nav_html: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllProjectsTasksQuery {
    show_complete: Option<String>,
}

/// Cross-project row assembly — one project at a time, mirroring
/// `project_tasks::list_task_rows_for_project`'s own gather shape (top-level Task items + each
/// Task series' current non-materialized occurrence) but across every project the requester
/// belongs to, each row tagged with its project's name. Duplicated rather than shared, per
/// this codebase's established "duplicate small per-screen helpers" precedent — this loop's
/// per-project bucketing/filtering/tagging has no real overlap with the single-project function
/// it otherwise resembles.
///
/// `just_completed_item_id` mirrors `project_tasks::render_rows_with_virtual`'s own parameter —
/// forces that one item's row to stay visible (with its "Completed" confirm-then-fade badge)
/// even when `!show_complete` would otherwise filter it out, used by `project_tasks::handlers::
/// complete_project_item_series_occurrence_form`'s `"all-tasks"` rebuild branch. `None` for the
/// plain page load (`all_projects_tasks_page`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn list_all_projects_task_rows(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    users: &Arc<dyn UserRepo>,
    teams: &Arc<dyn TeamRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    show_complete: bool,
    tz: i32,
    just_completed_item_id: Option<&str>,
) -> Result<Vec<String>, ItemError> {
    let user_projects = project_service::list_projects(projects, requester_user_id).await?;

    let mut entries: Vec<(i64, String)> = Vec::new();
    for project in &user_projects {
        let is_team_project = project.team_id.is_some();
        let names = match &project.team_id {
            Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
            None => HashMap::new(),
        };

        let mut items =
            project_item_service::list_project_items_unchecked(repo, &project.id, None).await?;
        items.retain(|i| i.kind() == ItemKind::Task);

        for item in &items {
            if !task_included(
                is_team_project,
                item.assigned_to_user_id().as_deref(),
                requester_user_id,
            ) {
                continue;
            }
            if !show_complete && item.complete && Some(item.id.as_str()) != just_completed_item_id {
                continue;
            }
            let ts = item.due_date().map(|d| d.timestamp()).unwrap_or(i64::MAX);
            let skip_url =
                item_series_service::skip_url_for_item(series, item, &project.id).await?;
            let just_completed = Some(item.id.as_str()) == just_completed_item_id;
            let confirmation = just_completed.then(|| "Completed".to_string());
            let dismiss_after_ms = (just_completed && !show_complete).then_some(1800u32);
            let html = all_projects_task_row(
                item,
                &project.id,
                &project.name,
                &names,
                is_team_project,
                tz,
                skip_url,
                show_complete,
                confirmation,
                dismiss_after_ms,
            )?;
            entries.push((ts, html));
        }

        let occurrences = item_series_service::list_occurrence_states_for_project(
            series,
            users,
            &project.id,
            Utc::now(),
            Utc::now(),
            tz,
        )
        .await?
        .into_iter()
        .filter(|occ| occ.item_type == ItemKind::Task && occ.is_current)
        .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
        .filter(|occ| {
            task_included(
                is_team_project,
                occ.assigned_to_user_id.as_deref(),
                requester_user_id,
            )
        });
        for occ in occurrences {
            entries.push((
                occ.occurrence_date.timestamp(),
                AllProjectsTaskVirtualRow::from_occurrence(
                    &occ,
                    &project.id,
                    &project.name,
                    tz,
                    show_complete,
                )
                .render()?,
            ));
        }
    }

    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

pub async fn all_projects_tasks_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<AllProjectsTasksQuery>,
) -> Result<Html<String>, ItemError> {
    let show_complete = q.show_complete.is_some();
    let rows = list_all_projects_task_rows(
        &repo,
        &projects,
        &users,
        &teams,
        &series,
        &auth_user.user_id,
        show_complete,
        tz,
        None,
    )
    .await?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        ActiveContext::AllProjects,
        SidebarSection::Tasks,
    )
    .await?;
    render(AllProjectsTasksListPageTemplate {
        rows,
        show_complete,
        nav_html,
    })
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToggleAllProjectsTaskForm {
    complete: Option<String>,
}

/// The row-checkbox target for a real (materialized) Task on this screen — mirrors
/// `main_calendar::toggle_main_calendar_item_complete` exactly (round-trips every other field
/// from `current`, only `complete` changes), re-rendering via this module's own
/// `all_projects_task_row` so the response keeps its `project_name` tag and cross-project URLs.
pub async fn toggle_all_projects_task_complete(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ToggleAllProjectsTaskForm>,
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
    let params = UpdateProjectItemParams {
        project_id: project_id.clone(),
        item_id: item_id.clone(),
        name: current.name.clone(),
        description: current.description.clone(),
        due_date: current.due_date(),
        scheduled_date: current.scheduled_date(),
        scheduled_end_date: current.scheduled_end_date(),
        complete: form.complete.as_deref() == Some("true"),
        has_due_time: Some(current.has_due_time()),
        has_scheduled_time: Some(current.has_scheduled_time()),
        has_end_time: Some(current.has_end_time()),
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(current.kind()),
        event_type: current.event_type(),
        due_offset_days: current.due_offset_days(),
        assigned_to_user_id: current.assigned_to_user_id(),
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        points: current.points(),
    };
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

    let project = project_item_service::get_project_unchecked(&projects, &project_id).await?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    match project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await {
        Ok(updated) => {
            let skip_url =
                item_series_service::skip_url_for_item(&series, &updated, &project_id).await?;
            Ok(Html(all_projects_task_row(
                &updated,
                &project_id,
                &project.name,
                &names,
                project.team_id.is_some(),
                tz,
                skip_url,
                false,
                None,
                None,
            )?))
        }
        // Same rationale as `main_calendar::toggle_main_calendar_item_complete`'s identical
        // branch — the item was a recurring legacy item that got replaced by a fresh
        // successor under a new id (item-level recurrence is retired, but a pre-retirement
        // completed item's history can still hit this).
        Err(ItemError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(e),
    }
}
