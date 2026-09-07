use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_input::{EditItem, EditItemKind, EditTask};
use crate::service::item_series::{
    self as item_series_service, OccurrenceState, ProjectOccurrence, SeriesChildOccurrenceView,
};
use crate::service::project_items::{self as project_item_service};
use crate::service::projects as project_service;
use crate::storage::sqlite::{
    ActivityLogRepo, ItemDependencyRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, ReminderRepo,
    TeamRepo, UserRepo,
};
use crate::web_ui::TzOffset;
use crate::web_ui::list_filters::{ListFilterQuery, ListFilters};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_tasks::names_for;
use crate::web_ui::project_tasks::require_task;
use crate::web_ui::project_tasks::templates::{ProjectTaskRow, priority_label_for};
use crate::web_ui::{format_display_date, to_local};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
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
    /// Stage 5 of the series sub-items plan — this occurrence's still-virtual sub-items,
    /// already rendered and inlined hidden. See `ProjectTaskVirtualRow::children_html`; the
    /// only difference here is that this screen isn't a treegrid, so the toggle hangs off the
    /// name the way `components/row.html`'s own non-treegrid branch does.
    children_html: Option<String>,
    /// `base.html`'s `toggleChildren` addresses elements as `item-{id}-children`/`chevron-{id}`,
    /// and this screen's `<li>` carries an `all-tasks-virtual-…` id rather than an `item-…` one
    /// — so the toggle gets its own id rather than reusing the row's.
    toggle_id: String,
}

impl AllProjectsTaskVirtualRow {
    /// `filters`/`project_filter` bake a `?view=all-tasks&<filters>[&project=<id>]` suffix onto
    /// the checkbox/Skip/Unskip URLs, mirroring `ProjectTaskVirtualRow::from_occurrence`'s own
    /// `list_query` — this screen is always the flat list (no calendar-day-panel equivalent, so
    /// no `in_list_view` gate is needed), letting `project_tasks::handlers::
    /// complete_project_item_series_occurrence_form`'s `"all-tasks"` branch (and
    /// `project_item_series::handlers`' skip/unskip `"all-tasks"` branch,
    /// `docs/all-projects-landing-plan.md` Stage 4) rebuild `#items-list` in place with the same
    /// filters (including the cross-project-only `project` dimension) the surrounding page load
    /// applied.
    fn from_occurrence(
        occ: &ProjectOccurrence,
        project_id: &str,
        project_name: &str,
        tz: i32,
        filters: &ListFilters,
        project_filter: Option<&str>,
    ) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        let mut parts = vec!["view=all-tasks".to_string()];
        let filters_suffix = filters.query_string();
        if !filters_suffix.is_empty() {
            parts.push(filters_suffix);
        }
        if let Some(pid) = project_filter {
            parts.push(format!("project={pid}"));
        }
        let list_query = format!("?{}", parts.join("&"));
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            project_name: project_name.to_string(),
            name: occ.series_name.clone(),
            date_label: format_display_date(local, true),
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
            // Filled in by `list_all_projects_task_rows`, which holds the fan-out.
            children_html: None,
            toggle_id: format!(
                "all-tasks-{}-{}",
                occ.series_id,
                occ.occurrence_date.timestamp()
            ),
        }
    }
}

/// Cross-project counterpart to `project_tasks::templates::ProjectTaskVirtualChildRow` — one
/// still-virtual sub-item, nested under its occurrence's row. Duplicated rather than shared
/// with that type for the same reason `AllProjectsTaskVirtualRow` is: this screen's rows carry
/// a project tag and no treegrid/selection markup at all (see this module's own doc comment on
/// the duplicate-small-per-screen-helpers precedent).
#[derive(Template)]
#[template(path = "all_projects_tasks/virtual_child_row.html")]
struct AllProjectsTaskVirtualChildRow {
    name: String,
    date_label: String,
    overdue: bool,
    /// See `ProjectTaskVirtualChildRow::offset_label`.
    offset_label: Option<String>,
    priority_label: Option<String>,
    detail_url: String,
    complete_url: String,
}

impl AllProjectsTaskVirtualChildRow {
    fn from_view(
        view: &SeriesChildOccurrenceView,
        project_id: &str,
        tz: i32,
        filters: &ListFilters,
        project_filter: Option<&str>,
    ) -> Self {
        // Same `?view=all-tasks&…` round-trip `AllProjectsTaskVirtualRow::from_occurrence`
        // builds — see its doc comment.
        let mut parts = vec!["view=all-tasks".to_string()];
        let filters_suffix = filters.query_string();
        if !filters_suffix.is_empty() {
            parts.push(filters_suffix);
        }
        if let Some(pid) = project_filter {
            parts.push(format!("project={pid}"));
        }
        let list_query = format!("?{}", parts.join("&"));
        Self {
            name: view.child_name.clone(),
            date_label: format_display_date(to_local(view.date, tz), true),
            overdue: view.date < Utc::now(),
            offset_label: crate::web_ui::project_tasks::templates::offset_label_for_days(
                -view.days_before,
            ),
            priority_label: priority_label_for(view.priority),
            detail_url: view.detail_url(project_id),
            complete_url: format!("{}{list_query}", view.complete_url(project_id)),
        }
    }
}

/// The still-virtual sub-items of one parent cycle, in authored order — this screen's own copy
/// of `project_tasks::render_virtual_child_rows` (see that function for why materialized ones
/// are filtered out here rather than at the call site).
fn render_virtual_child_rows(
    child_occurrences: &[SeriesChildOccurrenceView],
    series_id: &str,
    occurrence_ts: i64,
    project_id: &str,
    tz: i32,
    filters: &ListFilters,
    project_filter: Option<&str>,
) -> Result<Option<String>, ItemError> {
    let mut views: Vec<&SeriesChildOccurrenceView> = child_occurrences
        .iter()
        .filter(|v| v.item_id.is_none())
        .filter(|v| {
            v.series_id == series_id && v.parent_occurrence_date.timestamp() == occurrence_ts
        })
        .collect();
    if views.is_empty() {
        return Ok(None);
    }
    views.sort_by_key(|v| v.sort_order);
    let mut html = String::new();
    for view in views {
        html.push_str(
            &AllProjectsTaskVirtualChildRow::from_view(
                view,
                project_id,
                tz,
                filters,
                project_filter,
            )
            .render()?,
        );
    }
    Ok(Some(html))
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
/// calls this same function to re-render a Reschedule/Assign save from this screen.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn all_projects_task_row(
    item: &Item,
    project_id: &str,
    project_name: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    tz: i32,
    skip_url: Option<String>,
    // See `project_calendar::calendar_row`'s identical parameter (`Row::series_sub_item`).
    series_sub_item: bool,
    show_complete: bool,
    children_html: Option<String>,
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
        // This cross-project screen only ever shows a series' current occurrence (mirroring
        // `project_tasks`'s own flat-list restriction — see `Row::series_current`'s doc
        // comment), but doesn't yet run the batched query needed to confirm that per row.
        None,
    );
    row.expanded_row = true;
    // See `project_calendar::calendar_row`'s identical override.
    row.series_sub_item = series_sub_item;
    row.materialized_occurrence = row.materialized_occurrence || series_sub_item;
    row.project_name = Some(project_name.to_string());
    // #6 of docs/issues_and_features.md — a row with children was falling into the
    // `detail_via_dialog` name-click branch instead of expanding in place, since this function
    // never set `children_html` (unlike `project_tasks`'s own flat-list row assembly). Reuses
    // `project_tasks::render_expandable_children` unchanged — see this module's own callers of
    // this function for where `children_html` is actually built.
    row.children_html = children_html;
    // Stage 3 of docs/dialog-item-forms-plan.md: opted in — `ProjectTaskRow::from_item`
    // already sets `detail_via_dialog: true`, so no override is needed here (unlike
    // `main_calendar`/`project_calendar`'s own rows, which explicitly opt back out).
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
    /// `AssignedToFilter::as_value()`/`DueDateFilter::as_value()`/`ScheduleFilter::as_value()` —
    /// see `templates/project_tasks/list_page.html`'s identical filter-dialog fields, which this
    /// mirrors exactly.
    assigned_to: String,
    /// Merged, deduped (by user id) `active_member_options` across every team-backed project the
    /// requester belongs to — populates the "Assigned to" select's specific-member options, since
    /// a cross-project screen has no single team to ask.
    assignee_options: Vec<(String, String)>,
    /// Whether the requester belongs to at least one team-backed project — gates whether the
    /// "Assigned to" select renders at all, the cross-project counterpart of `project_tasks`'s
    /// per-project `is_team_project` gate.
    has_team_project: bool,
    due_date: String,
    schedule: String,
    recurring: bool,
    /// `PriorityFilter::as_value`: `""` (any priority) | `"1"`-`"4"`.
    priority: String,
    /// Pre-encoded `ListFilters::query_string()` — see `templates/project_tasks/list_page.html`'s
    /// identical "Filters" button dot-indicator use.
    filters_query: String,
    /// `(project id, project name, is_selected)` — `project_tasks/new_page.html`'s project
    /// `<select>` shape, reused here for the filter dialog's own project dimension.
    project_options: Vec<(String, String, bool)>,
    /// `"all"` or a specific project id — the filter dialog's project `<select>` value.
    project_filter: String,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AllProjectsTasksQuery {
    show_complete: Option<String>,
    assigned_to: Option<String>,
    due_date: Option<String>,
    schedule: Option<String>,
    recurring: Option<String>,
    priority: Option<String>,
    /// Cross-project-only filter dimension (`docs/list-filtering-plan.md`'s Out of scope note) —
    /// a specific project id, or absent/`"all"` for every project the requester belongs to.
    project: Option<String>,
}

impl AllProjectsTasksQuery {
    fn filters(&self) -> ListFilters {
        ListFilters::from_query(ListFilterQuery {
            show_complete: self.show_complete.clone(),
            assigned_to: self.assigned_to.clone(),
            due_date: self.due_date.clone(),
            schedule: self.schedule.clone(),
            recurring: self.recurring.clone(),
            priority: self.priority.clone(),
        })
    }

    fn project_filter(&self) -> Option<&str> {
        self.project
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty() && *p != "all")
    }
}

/// Cross-project row assembly — one project at a time, mirroring
/// `project_tasks::list_task_rows_for_project`'s own gather shape (top-level Task items + each
/// Task series' current non-materialized occurrence) but across every project the requester
/// belongs to, each row tagged with its project's name. Duplicated rather than shared, per
/// this codebase's established "duplicate small per-screen helpers" precedent — this loop's
/// per-project bucketing/filtering/tagging has no real overlap with the single-project function
/// it otherwise resembles.
///
/// `filters` is the same screen-agnostic `ListFilters` `project_tasks` uses
/// (`docs/list-filtering-plan.md`) — every dimension (`showComplete`/`assignedTo`/`dueDate`/
/// `schedule`/`recurring`) applies per item/occurrence exactly as it does there, gated by each
/// item's own project's `team_id.is_some()`. `project_filter`, unlike every other dimension, has
/// no `ListFilters` counterpart (that type is deliberately screen-agnostic, and "which project"
/// is meaningless on a screen already scoped to one) — `Some(id)` restricts the whole gather loop
/// to that one project, `None` (the default) spans every project the requester belongs to.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn list_all_projects_task_rows(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    users: &Arc<dyn UserRepo>,
    teams: &Arc<dyn TeamRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    filters: &ListFilters,
    project_filter: Option<&str>,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    let user_projects = project_service::list_projects(projects, requester_user_id).await?;
    let now = Utc::now();

    let mut entries: Vec<(i64, String)> = Vec::new();
    for project in &user_projects {
        if project_filter.is_some_and(|pid| pid != project.id) {
            continue;
        }
        let is_team_project = project.team_id.is_some();
        let names = match &project.team_id {
            Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
            None => HashMap::new(),
        };

        let mut items =
            project_item_service::list_project_items_unchecked(repo, &project.id, None).await?;
        items.retain(|i| i.kind() == ItemKind::Task);

        // Fetched before the item loop (not alongside the virtual rows below) because a
        // *materialized* occurrence's own row needs its cycle's still-virtual sub-items nested
        // under it — see `project_tasks::render_rows_with_virtual`'s identical handling.
        #[allow(clippy::type_complexity)]
        let (all_occurrences, child_occurrences, materialized_cycles): (
            Vec<ProjectOccurrence>,
            Vec<SeriesChildOccurrenceView>,
            HashMap<String, (String, i64)>,
        ) = if filters.recurring {
            let all_occurrences = item_series_service::list_occurrence_states_for_project(
                series,
                users,
                &project.id,
                now,
                now,
                tz,
            )
            .await?;
            let materialized_cycles = all_occurrences
                .iter()
                .filter_map(|occ| match &occ.state {
                    OccurrenceState::Materialized { item_id } => Some((
                        item_id.clone(),
                        (occ.series_id.clone(), occ.occurrence_date.timestamp()),
                    )),
                    _ => None,
                })
                .collect();
            let child_occurrences =
                item_series_service::fan_out_child_occurrences(series, &all_occurrences, tz)
                    .await?;
            (all_occurrences, child_occurrences, materialized_cycles)
        } else {
            (Vec::new(), Vec::new(), HashMap::new())
        };

        for item in &items {
            if !filters.matches(item, requester_user_id, is_team_project, now) {
                continue;
            }
            let ts = item.due_date().map(|d| d.timestamp()).unwrap_or(i64::MAX);
            let skip_url =
                item_series_service::skip_url_for_item(series, item, &project.id).await?;
            // Always false here — a materialized sub-item is nested, so it renders through
            // `render_expandable_children` instead. The top-level half comes off `skip_url`
            // inside `all_projects_task_row`. See `Row::series_sub_item`.
            let series_sub_item = false;
            let children_html = if item.has_children {
                let descendants = crate::web_ui::project_tasks::render_expandable_children(
                    repo,
                    &item.id,
                    &project.id,
                    &names,
                    filters.show_complete,
                    tz,
                    &HashMap::new(),
                    is_team_project,
                    1,
                    None,
                    Some(series),
                    // Treegrid keyboard-nav pilot is Tasks-list-only for now — see
                    // `Row::treegrid`'s doc comment.
                    false,
                    0,
                )
                .await?;
                // has_children counts filtered-out children too — see
                // render_expandable_children's own call sites for the same guard.
                (!descendants.is_empty()).then_some(descendants)
            } else {
                None
            };
            // Sub-items nobody has materialized yet have no item for the walk above to find.
            let children_html = match materialized_cycles.get(&item.id) {
                Some((series_id, occurrence_ts)) => match render_virtual_child_rows(
                    &child_occurrences,
                    series_id,
                    *occurrence_ts,
                    &project.id,
                    tz,
                    filters,
                    project_filter,
                )? {
                    Some(virtual_children) => Some(match children_html {
                        Some(existing) => existing + &virtual_children,
                        None => virtual_children,
                    }),
                    None => children_html,
                },
                None => children_html,
            };
            let html = all_projects_task_row(
                item,
                &project.id,
                &project.name,
                &names,
                is_team_project,
                tz,
                skip_url,
                series_sub_item,
                filters.show_complete,
                children_html,
            )?;
            entries.push((ts, html));
        }

        let occurrences = all_occurrences
            .iter()
            .filter(|occ| occ.item_type == ItemKind::Task && occ.is_current)
            .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
            .filter(|occ| filters.matches_occurrence(occ, requester_user_id, is_team_project, now));
        for occ in occurrences {
            let mut row = AllProjectsTaskVirtualRow::from_occurrence(
                occ,
                &project.id,
                &project.name,
                tz,
                filters,
                project_filter,
            );
            row.children_html = render_virtual_child_rows(
                &child_occurrences,
                &occ.series_id,
                occ.occurrence_date.timestamp(),
                &project.id,
                tz,
                filters,
                project_filter,
            )?;
            entries.push((occ.occurrence_date.timestamp(), row.render()?));
        }
    }

    entries.sort_by_key(|(key, _)| *key);
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
    let filters = q.filters();
    let project_filter = q.project_filter();
    let rows = list_all_projects_task_rows(
        &repo,
        &projects,
        &users,
        &teams,
        &series,
        &auth_user.user_id,
        &filters,
        project_filter,
        tz,
    )
    .await?;
    let user_projects = project_service::list_projects(&projects, &auth_user.user_id).await?;
    let mut assignee_map: HashMap<String, String> = HashMap::new();
    let mut has_team_project = false;
    for project in &user_projects {
        if let Some(team_id) = &project.team_id {
            has_team_project = true;
            for (id, name) in crate::web_ui::project_tasks::active_member_options(
                &teams,
                team_id,
                &auth_user.user_id,
            )
            .await?
            {
                assignee_map.entry(id).or_insert(name);
            }
        }
    }
    let mut assignee_options: Vec<(String, String)> = assignee_map.into_iter().collect();
    assignee_options.sort_by(|a, b| a.1.cmp(&b.1));
    let project_filter_value = project_filter.unwrap_or("all").to_string();
    let project_options = user_projects
        .iter()
        .map(|p| {
            (
                p.id.clone(),
                p.name.clone(),
                project_filter == Some(p.id.as_str()),
            )
        })
        .collect();
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        ActiveContext::AllProjects,
        SidebarSection::Tasks,
    )
    .await?;
    render(AllProjectsTasksListPageTemplate {
        rows,
        show_complete: filters.show_complete,
        assigned_to: filters.assigned_to.as_value(),
        assignee_options,
        has_team_project,
        due_date: filters.due_date.as_value().to_string(),
        schedule: filters.schedule.as_value().to_string(),
        recurring: filters.recurring,
        priority: filters.priority.as_value(),
        filters_query: filters.query_string(),
        project_options,
        project_filter: project_filter_value,
        nav_html,
    })
}

/// Stage 3 of `docs/dialog-item-forms-plan.md` — the all-projects counterpart to
/// `project_tasks::templates::NewProjectTaskPageTemplate`, with a project `<select>` prepended.
/// `project_options` is `(id, name, is_selected)`; the form's own `hx-post` is baked to whatever
/// project is currently selected (`project_id`), so no client-side URL rewriting is needed —
/// only the select's own `hx-get` back to this same route (see `new_page.html`) re-renders the
/// whole fragment server-side when the selection changes, per the plan's "simplest correct
/// approach" note.
#[derive(Template)]
#[template(path = "all_projects_tasks/new_page.html")]
struct AllProjectsNewTaskPageTemplate {
    project_id: String,
    project_options: Vec<(String, String, bool)>,
    show_complete: bool,
    is_team_project: bool,
    assignee_options: Vec<(String, String)>,
    blank_scheduled_date_input: String,
    blank_scheduled_time_input: String,
    blank_scheduled_end_date_input: String,
    blank_scheduled_end_time_input: String,
    blank_due_date_input: String,
    blank_due_time_input: String,
    is_team_admin: bool,
    blank_points_input: String,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NewAllProjectsTaskQuery {
    project: Option<String>,
    show_complete: Option<String>,
}

/// `GET /web/tasks/new` — Stage 3. Resolves the selected project (query param, else
/// `personal_project_id`, else the first project) via `crate::web_ui::resolve_new_item_project`,
/// then renders the dialog fragment for that project. Both the initial "+ New Task" click and
/// the dialog's own project-select `onchange` hit this same route (see `new_page.html`).
pub async fn new_all_projects_task_dialog(
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<NewAllProjectsTaskQuery>,
) -> Result<Html<String>, ItemError> {
    let user_projects = project_service::list_projects(&projects, &auth_user.user_id).await?;
    let user = users.get(&auth_user.user_id).await?;
    let selected = crate::web_ui::resolve_new_item_project(
        &user_projects,
        q.project.as_deref(),
        user.personal_project_id.as_deref(),
    )
    .ok_or(ItemError::NotFound)?;
    let project_id = selected.id.clone();
    let is_team_project = selected.team_id.is_some();
    let (assignee_options, is_team_admin) = match &selected.team_id {
        Some(team_id) => (
            crate::web_ui::project_tasks::active_member_options(
                &teams,
                team_id,
                &auth_user.user_id,
            )
            .await?,
            project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    let project_options = user_projects
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.id == project_id))
        .collect();
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        ActiveContext::AllProjects,
        SidebarSection::Tasks,
    )
    .await?;
    render(AllProjectsNewTaskPageTemplate {
        project_id,
        project_options,
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

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToggleAllProjectsTaskForm {
    complete: Option<String>,
    /// Round-tripped from the checkbox's own `hx-vals` (`components/row.html`'s
    /// `show_complete`-gated `showComplete` field) — previously absent from this struct, so
    /// axum's `Form` extractor silently dropped it and `show_complete` below always defaulted
    /// to `false`, the root cause of #7 of docs/issues_and_features.md's calendar/list-view
    /// entries (completing a task here never showed the "Completed" confirmation
    /// `update_project_task_form` already gives the same checkbox on the per-project screen).
    /// Still load-bearing after that confirmation moved to a page-level banner: it's how this
    /// handler knows whether a just-completed row still belongs on the list at all — see
    /// `web_ui::hx_delete_target`.
    show_complete: Option<String>,
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
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    headers: HeaderMap,
    Form(form): Form<ToggleAllProjectsTaskForm>,
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
    // Only a Task can be completed, and only a Task row renders a checkbox pointing here
    // (`calendar_row` leaves an Event's `complete_url` as `None`) — but the old flat params
    // passed `item_type: Some(current.kind())`, so a crafted POST naming an Event fell through
    // to `Item::validate()`. `EditTask` cannot express a non-Task, and defaulting to Task would
    // silently *rewrite* the row's kind, so the check moves up front.
    let current = require_task(current)?;
    let mut task = EditTask::from_item(&current);
    task.complete = form.complete.as_deref() == Some("true");
    let edit = EditItem {
        project_id: project_id.clone(),
        item_id: item_id.clone(),
        name: current.name.clone(),
        description: current.description.clone(),
        timezone_offset_minutes: Some(tz),
        depends_on_item_ids: None,
        kind: EditItemKind::Task(task),
    };
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &reminders,
        &item_dependencies,
        &auth_user.user_id,
        edit,
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
            // This single-row swap can re-render a nested sub-item too — see
            // `project_tasks::handlers::update_project_task_form`'s identical computation.
            let series_sub_item =
                item_series_service::is_materialized_sub_item(&series, &updated).await?;
            // #7 of docs/issues_and_features.md — mirrors `update_project_task_form`'s
            // identical completion handling, previously missing here entirely (this handler
            // never signalled a completion at all, so completing a task on this screen was
            // silent where the per-project Tasks list already confirmed the same checkbox).
            let show_complete = form.show_complete.is_some();
            let just_completed = !current.complete() && updated.complete();
            let children_html = if updated.has_children {
                let descendants = crate::web_ui::project_tasks::render_expandable_children(
                    &repo,
                    &updated.id,
                    &project_id,
                    &names,
                    show_complete,
                    tz,
                    &HashMap::new(),
                    project.team_id.is_some(),
                    1,
                    None,
                    Some(&series),
                    // Treegrid keyboard-nav pilot is Tasks-list-only for now — see
                    // `Row::treegrid`'s doc comment.
                    false,
                    0,
                )
                .await?;
                // has_children counts filtered-out children too — see
                // render_expandable_children's own call sites for the same guard.
                (!descendants.is_empty()).then_some(descendants)
            } else {
                None
            };
            let mut response = Html(all_projects_task_row(
                &updated,
                &project_id,
                &project.name,
                &names,
                project.team_id.is_some(),
                tz,
                skip_url,
                series_sub_item,
                show_complete,
                children_html,
            )?)
            .into_response();
            // See `project_tasks::handlers::update_project_task_form`'s identical block.
            if just_completed {
                if !show_complete && crate::web_ui::targets_row(&headers, &updated.id) {
                    response = crate::web_ui::hx_delete_target(response);
                }
                response = crate::web_ui::hx_toast(response, "Completed");
            }
            Ok(response)
        }
        // Same rationale as `main_calendar::toggle_main_calendar_item_complete`'s identical
        // branch — the item was a recurring legacy item that got replaced by a fresh
        // successor under a new id (item-level recurrence is retired, but a pre-retirement
        // completed item's history can still hit this).
        Err(ItemError::NotFound) => Ok(Html(String::new()).into_response()),
        Err(e) => Err(e),
    }
}
