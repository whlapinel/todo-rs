use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::item::Item;
use crate::service::error::ItemError;
use crate::service::item_series::{OccurrenceState, ProjectOccurrence};
use crate::storage::sqlite::ItemRepo;
use crate::web_ui::components::row::Row;
use crate::web_ui::to_local;
use askama::Template;
use chrono::Utc;

// ---- templates --------------------------------------------------------------

pub fn offset_label_for(item: &Item) -> Option<String> {
    if !item.is_offset_driven() {
        return None;
    }
    match item.due_offset_days() {
        Some(0) => Some("on due date".to_string()),
        Some(n) if n > 0 => Some(format!("+{n}d")),
        Some(n) => Some(format!("{n}d")),
        None => None,
    }
}

pub fn format_offset_input(due_offset_days: Option<i32>) -> String {
    due_offset_days.map(|d| d.to_string()).unwrap_or_default()
}

pub fn format_points_input(points: Option<i32>) -> String {
    points.map(|p| p.to_string()).unwrap_or_default()
}

/// `Row`'s first real caller — see `docs/project-abstraction-plan.md` stage B5a. Builds a
/// generic `components::row::Row` rather than a Task-specific template of its own.
pub struct ProjectTaskRow;

impl ProjectTaskRow {
    pub fn from_item(
        item: &Item,
        project_id: &str,
        names: &HashMap<String, String>,
        siblings: &[&Item],
        tz: i32,
    ) -> Row {
        let offset_label = offset_label_for(item);
        let assignee_name = item
            .assigned_to_user_id()
            .map(|id| names.get(&id).cloned().unwrap_or(id));
        Row {
            id: item.id.clone(),
            item_url: format!("/web/projects/{project_id}/tasks/{}", item.id),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item.due_date().map(|d| {
                if item.has_due_time() {
                    to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()
                } else {
                    to_local(d, tz).format("%Y-%m-%d").to_string()
                }
            }),
            overdue: item.is_overdue(Utc::now()),
            scheduled_date: item.scheduled_date().map(|d| {
                let local = to_local(d, tz);
                if item.has_scheduled_time() {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            scheduled_end_date: item.scheduled_end_date().map(|d| {
                let local = to_local(d, tz);
                if item.has_end_time() {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            event_type: item.event_type(),
            expanded_row: item.due_date().is_some()
                || item.scheduled_date().is_some()
                || item.due_offset_days().is_some()
                || offset_label.is_some()
                || assignee_name.is_some(),
            has_children: item.has_children,
            offset_label,
            assignee_name,
            complete_url: Some(format!("/web/projects/{project_id}/tasks/{}", item.id)),
            duplicate_url: Some(format!(
                "/web/projects/{project_id}/tasks/{}/duplicate",
                item.id
            )),
            reschedule_url: Some(format!(
                "/web/projects/{project_id}/tasks/{}/reschedule",
                item.id
            )),
            toggle_complete_json: (!item.complete).to_string(),
            siblings: siblings
                .iter()
                .filter(|s| s.id != item.id)
                .map(|s| (s.id.clone(), s.name.clone()))
                .collect(),
            is_source_event_linked: item.source_event_id().is_some(),
        }
    }
}

#[derive(Template)]
#[template(path = "components/reschedule_dialog.html")]
pub struct RescheduleDialog {
    pub post_reschedule_url: String,
    pub scheduled_start_date: Option<String>,
    pub scheduled_start_time: Option<String>,
    pub scheduled_end_date: Option<String>,
    pub scheduled_end_time: Option<String>,
    pub due_date: Option<String>,
    pub due_time: Option<String>,
}

impl RescheduleDialog {
    pub fn from_task(task: Item, project_id: &str) -> Self {
        RescheduleDialog {
            post_reschedule_url: format!("/web/projects/{}/tasks/{}/", project_id, task.id),
            scheduled_start_date: task
                .scheduled_date()
                .map(|date| date.format(&"%Y-%m-%d").to_string()),
            scheduled_start_time: task
                .has_scheduled_time()
                .then(|| {
                    task.scheduled_date()
                        .map(|date| date.format(&"%Y-%m-%d").to_string())
                })
                .flatten(),
            scheduled_end_date: task
                .scheduled_end_date()
                .map(|date| date.format("%Y-%m-%d").to_string()),
            scheduled_end_time: task
                .has_end_time()
                .then(|| {
                    task.scheduled_end_date()
                        .map(|date| date.format(&"%Y-%m-%d").to_string())
                })
                .flatten(),
            due_date: task
                .due_date()
                .map(|date| date.format("%Y-%m-%d").to_string()),
            due_time: task
                .has_due_time()
                .then(|| {
                    task.scheduled_end_date()
                        .map(|date| date.format(&"%Y-%m-%d").to_string())
                })
                .flatten(),
        }
    }
}

/// Stage 10 gap 2: a Task series' `current_occurrence_date`, rendered as a distinct virtual
/// row in the flat `/tasks` list — mirrors `project_dashboard::ProjectDashboardVirtualRow` and
/// the Tasks calendar's `CalendarVirtualTaskEntry`, minus the type badge (this list is
/// Task-only already). Every row built from this struct is current by construction (callers
/// filter for `is_current` before constructing one), so the template has no "Planned" branch.
#[derive(Template)]
#[template(path = "project_tasks/virtual_row.html")]
pub struct ProjectTaskVirtualRow {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub name: String,
    pub date_label: String,
    pub materialize_url: String,
    pub skip_url: String,
    pub is_current: bool,
    /// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — `true` when this
    /// occurrence has been explicitly skipped (`OccurrenceState::Skipped`), in which case the
    /// template shows a struck-through name + "Skipped" label + Unskip button instead of the
    /// materialize link/Skip button.
    pub is_skipped: bool,
    pub unskip_url: String,
}

impl ProjectTaskVirtualRow {
    pub fn from_occurrence(occ: &ProjectOccurrence, project_id: &str, tz: i32) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            name: occ.series_name.clone(),
            date_label: local.format("%Y-%m-%d %H:%M").to_string(),
            materialize_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{}",
                occ.series_id,
                occ.occurrence_date.timestamp(),
            ),
            skip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{}/skip",
                occ.series_id,
                occ.occurrence_date.timestamp(),
            ),
            is_current: occ.is_current,
            is_skipped: matches!(occ.state, OccurrenceState::Skipped),
            unskip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{}/unskip",
                occ.series_id,
                occ.occurrence_date.timestamp(),
            ),
        }
    }
}

#[derive(Template)]
#[template(path = "project_tasks/detail_fields.html")]
pub struct ProjectTaskDetailFields {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub complete: bool,
    pub is_top_level: bool,
    /// See `tasks::TaskDetailFields`'s identical field.
    pub is_offset_driven: bool,
    pub due_date_input: String,
    pub due_time_input: String,
    pub scheduled_date_input: String,
    pub scheduled_time_input: String,
    pub scheduled_end_date_input: String,
    pub scheduled_end_time_input: String,
    pub due_offset_days_input: String,
    /// True for a team-backed project — gates the "Assign to"/points markup, which never
    /// renders at all on a personal project (not just hidden — see
    /// `docs/project-abstraction-plan.md` stage B5a's note on why points stays
    /// team-membership-gated rather than a new `ProjectRepo`-native concept).
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub assigned_to_user_id: Option<String>,
    /// Gates the admin-only `points` input — see `macros::points_field` and
    /// `service::teams::require_team_admin`, whose result populates this at render time.
    pub is_team_admin: bool,
    pub points_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    pub just_saved: bool,
}

#[allow(clippy::too_many_arguments)]
impl ProjectTaskDetailFields {
    pub fn from_item(
        item: &Item,
        project_id: &str,
        is_team_project: bool,
        assignee_options: Vec<(String, String)>,
        is_team_admin: bool,
        tz: i32,
        just_saved: bool,
    ) -> Self {
        let local_due_date = item.due_date().map(|d| to_local(d, tz));
        let due_date_input = local_due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time() {
            local_due_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_date = item.scheduled_date().map(|d| to_local(d, tz));
        let scheduled_date_input = local_scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_time_input = if item.has_scheduled_time() {
            local_scheduled_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_end_date = item.scheduled_end_date().map(|d| to_local(d, tz));
        let scheduled_end_date_input = local_scheduled_end_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_end_time_input = if item.has_end_time() {
            local_scheduled_end_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            complete: item.complete,
            is_top_level: item.parent_item_id.is_none(),
            is_offset_driven: item.is_offset_driven(),
            due_date_input,
            due_time_input,
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            due_offset_days_input: format_offset_input(item.due_offset_days()),
            is_team_project,
            assignee_options,
            assigned_to_user_id: item.assigned_to_user_id(),
            is_team_admin,
            points_input: format_points_input(item.points()),
            just_saved,
        }
    }
}

/// Read-only counterpart to `ProjectTaskDetailFields` — see `items.rs`'s `DetailView` for the
/// row-editing convention this mirrors (complete-toggle lives here too).
#[derive(Template)]
#[template(path = "project_tasks/detail_view.html")]
pub struct ProjectTaskDetailView {
    pub id: String,
    pub project_id: String,
    pub description: Option<String>,
    pub complete: bool,
    pub toggle_complete_json: String,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub scheduled_date: Option<String>,
    pub scheduled_end_date: Option<String>,
    pub is_top_level: bool,
    pub is_offset_driven: bool,
    pub offset_label: Option<String>,
    pub is_team_project: bool,
    pub assignee_name: Option<String>,
    /// See `tasks::TaskDetailView`'s identical field.
    pub linked_event: Option<(String, String)>,
    /// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — `Some((series_name,
    /// edit_url))` when this item was materialized from a series (`item.series_id.is_some()`),
    /// closing `docs/issues.md`'s ranked item 2(a) ("no link from a materialized item's detail
    /// page back to its series"). `None` for every item never materialized from a series — the
    /// overwhelmingly common case.
    pub series_link: Option<(String, String)>,
}

impl ProjectTaskDetailView {
    #[allow(clippy::too_many_arguments)]
    pub fn from_item(
        item: &Item,
        project_id: &str,
        is_team_project: bool,
        names: &HashMap<String, String>,
        tz: i32,
        linked_event: Option<(String, String)>,
        series_link: Option<(String, String)>,
    ) -> Self {
        let due_date = item.due_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_due_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_date = item.scheduled_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_scheduled_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_end_date = item.scheduled_end_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_end_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            description: item.description.clone(),
            complete: item.complete,
            toggle_complete_json: (!item.complete).to_string(),
            due_date,
            overdue: item.is_overdue(Utc::now()),
            scheduled_date,
            scheduled_end_date,
            is_top_level: item.parent_item_id.is_none(),
            is_offset_driven: item.is_offset_driven(),
            offset_label: offset_label_for(item),
            is_team_project,
            assignee_name: item
                .assigned_to_user_id()
                .map(|id| names.get(&id).cloned().unwrap_or(id)),
            linked_event,
            series_link,
        }
    }
}

/// Resolves the (name, detail-page URL) of the Event a task references via `sourceEventId`,
/// scoped to `project_id` — the project-scoped counterpart of `tasks::resolve_linked_event`/
/// `team_tasks::resolve_linked_event`. Links to the project-scoped Events screen directly
/// (`/web/projects/{project_id}/events/{id}`) now that stage B5b has built one — until this
/// stage it fell back to the event's *legacy* detail URL (`dashboard::detail_url`); the event
/// is guaranteed to already belong to `project_id` (fetched via `get_by_project` below), so
/// building the URL locally needs no extra lookup.
pub async fn resolve_linked_event(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(event_id) = item.source_event_id() else {
        return Ok(None);
    };
    let event = repo
        .get_by_project(project_id, &event_id)
        .await
        .map_err(ItemError::from)?;
    Ok(Some((
        event.name.clone(),
        format!("/web/projects/{project_id}/events/{}", event.id),
    )))
}

/// Resolves the (series_name, edit-page URL) of the `ItemSeries` this item was materialized
/// from, via `item.series_id` — see `ProjectTaskDetailView::series_link`'s doc comment.
/// Links to the series' edit page (`/web/projects/{project_id}/series/{series_id}/edit`) —
/// there's no dedicated series *detail* page (only the list and edit screens), and the edit
/// page already shows every field a "view" would.
pub async fn resolve_series_link(
    event_series: &Arc<dyn crate::storage::sqlite::ItemSeriesRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(series_id) = &item.series_id else {
        return Ok(None);
    };
    let series = event_series.get_series(series_id).await?;
    Ok(Some((
        series.name,
        format!("/web/projects/{project_id}/series/{series_id}/edit"),
    )))
}

#[derive(Template)]
#[template(path = "project_tasks/rows_fragment.html")]
pub struct ProjectTaskRowsFragmentTemplate {
    pub rows: Vec<String>,
    pub empty_message: String,
}

#[derive(Template)]
#[template(path = "project_tasks/list_page.html")]
pub struct ProjectTasksListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub show_complete: bool,
    /// `Some("{n} pts")` on a team-backed project (the viewer's own balance — see
    /// `service::teams::member_points`), `None` on a personal project.
    pub points_label: Option<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_tasks/new_page.html")]
pub struct NewProjectTaskPageTemplate {
    pub project_id: String,
    pub show_complete: bool,
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub blank_scheduled_date_input: String,
    pub blank_scheduled_time_input: String,
    pub blank_scheduled_end_date_input: String,
    pub blank_scheduled_end_time_input: String,
    pub blank_due_date_input: String,
    pub blank_due_time_input: String,
    pub is_team_admin: bool,
    pub blank_points_input: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_tasks/detail_page.html")]
pub struct ProjectTaskDetailPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub complete: bool,
    pub view: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_tasks/edit_page.html")]
pub struct ProjectTaskEditPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}

pub struct CalendarTaskEntry {
    pub id: String,
    pub name: String,
    pub time_label: Option<String>,
    pub date_type: DateType,
    pub has_end: bool,
    pub complete: bool,
}

pub enum DateType {
    Due,
    ScheduledStart,
    ScheduledEnd,
}

/// A Task-series (Stage 8) virtual occurrence rendered on the Tasks calendar. Kept separate
/// from `CalendarTaskEntry`/`DateType` — those mean "which real date field of this real task
/// is this," a distinction a virtual occurrence (one bare `occurrence_date`, no materialized
/// row yet) doesn't have, and they carry no materialize/skip affordance since a real task
/// never needs one.
pub struct CalendarVirtualTaskEntry {
    pub entry_id: String,
    pub name: String,
    pub time_label: Option<String>,
    pub materialize_url: String,
    pub skip_url: String,
    /// Stage 9: whether this is the series' `current_occurrence_date` — the one
    /// settleable occurrence a Task-typed series exposes, possibly backlogged into
    /// the past (see `service::item_series::current_occurrence_date`).
    pub is_current: bool,
    /// See `ProjectTaskVirtualRow::is_skipped`'s identical rationale.
    pub is_skipped: bool,
    pub unskip_url: String,
}

pub struct CalendarDay {
    pub date: String,
    pub day_number: u32,
    pub is_current_month: bool,
    pub is_today: bool,
    pub tasks: Vec<CalendarTaskEntry>,
    pub virtual_tasks: Vec<CalendarVirtualTaskEntry>,
}

#[derive(Template)]
#[template(path = "project_tasks/calendar_page.html")]
pub struct ProjectTasksCalendarPageTemplate {
    pub project_id: String,
    pub month_label: String,
    pub month_iso: String,
    pub prev_year: i32,
    pub prev_month: u32,
    pub next_year: i32,
    pub next_month: u32,
    pub days: Vec<CalendarDay>,
    pub nav_html: String,
}
