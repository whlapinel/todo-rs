use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::item::Item;
use crate::service::error::ItemError;
use crate::service::item_series::ProjectOccurrence;
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
    #[allow(clippy::too_many_arguments)]
    pub fn from_item(
        item: &Item,
        project_id: &str,
        names: &HashMap<String, String>,
        siblings: &[&Item],
        tz: i32,
        skip_url: Option<String>,
        is_team_project: bool,
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
            assign_url: is_team_project
                .then(|| format!("/web/projects/{project_id}/tasks/{}/assign", item.id)),
            skip_url,
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

/// A lightweight date/schedule-only editor, opened from a task row's calendar-icon button —
/// see `docs/archived/archived_issues_and_features.md`'s "quick reschedule" entry. Deliberately reuses the same PUT
/// `/web/projects/:project_id/tasks/:item_id` endpoint (`handlers::update_project_task_form`)
/// the full edit form already saves to, rather than introducing a second save path: the field
/// names below (`dueDate`/`dueTime`/`scheduledDate`/... via `macros::due_date_fields`/
/// `scheduled_fields`) match `ProjectTaskForm` exactly, so every field this dialog omits
/// (name, assignment, points, ...) round-trips from `current` unchanged via that handler's
/// existing overlay helpers — no separate validation/side-effect logic to duplicate.
#[derive(Template)]
#[template(path = "components/reschedule_dialog.html")]
pub struct RescheduleDialog {
    pub item_id: String,
    pub post_reschedule_url: String,
    pub scheduled_start_date: String,
    pub scheduled_start_time: String,
    pub scheduled_end_date: String,
    pub scheduled_end_time: String,
    pub due_date: String,
    pub due_time: String,
}

impl RescheduleDialog {
    pub fn from_task(task: &Item, project_id: &str, tz: i32) -> Self {
        let local_due_date = task.due_date().map(|d| to_local(d, tz));
        let local_scheduled_date = task.scheduled_date().map(|d| to_local(d, tz));
        let local_scheduled_end_date = task.scheduled_end_date().map(|d| to_local(d, tz));
        RescheduleDialog {
            item_id: task.id.clone(),
            post_reschedule_url: format!("/web/projects/{project_id}/tasks/{}", task.id),
            due_date: local_due_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            due_time: if task.has_due_time() {
                local_due_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            scheduled_start_date: local_scheduled_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_start_time: if task.has_scheduled_time() {
                local_scheduled_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            scheduled_end_date: local_scheduled_end_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_end_time: if task.has_end_time() {
                local_scheduled_end_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
        }
    }
}

/// A lightweight assignee-only editor, opened from a task row's person-icon button — mirrors
/// `RescheduleDialog` exactly (see its doc comment for the rationale): reuses the same PUT
/// `/web/projects/:project_id/tasks/:item_id` endpoint (`handlers::update_project_task_form`),
/// so every field this dialog omits round-trips from `current` unchanged via that handler's
/// existing overlay helpers. Only ever built for a team-backed project — `assign_url` (see
/// `components::row::Row`) is `None` on a personal project, so this dialog's route is never
/// reached without one.
#[derive(Template)]
#[template(path = "components/quick_assign_dialog.html")]
pub struct QuickAssignDialog {
    pub item_id: String,
    pub post_assign_url: String,
    pub assignee_options: Vec<(String, String)>,
    pub assigned_to_user_id: Option<String>,
}

impl QuickAssignDialog {
    pub fn from_task(
        task: &Item,
        project_id: &str,
        assignee_options: Vec<(String, String)>,
    ) -> Self {
        QuickAssignDialog {
            item_id: task.id.clone(),
            post_assign_url: format!("/web/projects/{project_id}/tasks/{}", task.id),
            assignee_options,
            assigned_to_user_id: task.assigned_to_user_id(),
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
    /// Mirrors `ProjectTaskRow`'s 💀-vs-📅 date icon choice — see
    /// `ProjectOccurrence::is_due_date_basis`'s doc comment.
    pub is_due_date_basis: bool,
    /// Same meaning as `ProjectTaskRow::overdue` — `false` for a scheduled-date-basis series
    /// (scheduled dates are never styled overdue, matching a materialized row).
    pub overdue: bool,
    pub materialize_url: String,
    pub skip_url: String,
    pub complete_url: String,
    pub is_current: bool,
    /// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — `true` when this
    /// occurrence has been explicitly skipped (`OccurrenceState::Skipped`), in which case the
    /// template shows a struck-through name + "Skipped" label + Unskip button instead of the
    /// materialize link/Skip button.
    pub is_skipped: bool,
    pub unskip_url: String,
    /// Populated the same way `ProjectTaskRow::assignee_name` is — previously dropped despite
    /// `ProjectOccurrence` already carrying it, so a virtual occurrence's row silently showed no
    /// assignee until materialized. See docs/issues_and_features.md's "occurrences don't show
    /// the assignee unless it's materialized" item.
    pub assignee_name: Option<String>,
}

impl ProjectTaskVirtualRow {
    pub fn from_occurrence(occ: &ProjectOccurrence, project_id: &str, tz: i32) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            name: occ.series_name.clone(),
            date_label: local.format("%Y-%m-%d %H:%M").to_string(),
            is_due_date_basis: occ.is_due_date_basis,
            overdue: occ.is_due_date_basis && occ.occurrence_date < Utc::now(),
            materialize_url: occ.materialize_url(project_id),
            skip_url: occ.skip_url(project_id),
            complete_url: occ.complete_url(project_id),
            is_current: occ.is_current,
            is_skipped: occ.is_skipped(),
            unskip_url: occ.unskip_url(project_id),
            assignee_name: occ.assigned_to_user_name.clone(),
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
    /// closing `docs/archived/archived_issues_and_features.md`'s ranked item 2(a) ("no link from a materialized item's detail
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

/// Stage C of `docs/unify-virtual-materialized-occurrences-plan.md` — the read-only view for
/// a still-virtual or skipped series occurrence, rendered by the new
/// `GET /projects/:project_id/series/:series_id/occurrences/:occurrence_ts` route (no side
/// effect — this never materializes). Visually mirrors `ProjectTaskDetailView`, but every
/// mutating affordance (checkbox, Skip/Unskip) points at a series/occurrence-scoped route
/// that materializes the occurrence as an internal step of the write, rather than at
/// `/tasks/{id}` (which doesn't exist yet). Once materialized, the detail *page* route
/// redirects to the item's real `/tasks/{id}` page instead of rendering this — so this
/// struct is only ever built for a `Virtual` or `Skipped` occurrence, never a materialized
/// one; there is no `is_top_level`/offset/linked-event handling here because a series
/// occurrence is always top-level and never offset-driven.
#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_view.html")]
pub struct ProjectTaskSeriesOccurrenceView {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub description: Option<String>,
    pub is_skipped: bool,
    pub is_current: bool,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub scheduled_date: Option<String>,
    pub is_team_project: bool,
    pub assignee_name: Option<String>,
    pub update_url: String,
    pub skip_url: String,
    pub unskip_url: String,
}

impl ProjectTaskSeriesOccurrenceView {
    #[allow(clippy::too_many_arguments)]
    pub fn from_series(
        series: &crate::domain::item_series::ItemSeries,
        occurrence_date: chrono::DateTime<Utc>,
        project_id: &str,
        is_team_project: bool,
        names: &HashMap<String, String>,
        // Stage 4 of docs/assignment-rotation-plan.md — this occurrence's own resolved
        // assignee (`item_series_service::resolve_occurrence_assignee`), not
        // `series.assigned_to_user_id` directly, which is always `None` for a rotating
        // series.
        resolved_assignee_id: Option<String>,
        is_skipped: bool,
        is_current: bool,
        tz: i32,
    ) -> Self {
        let occurrence_ts = occurrence_date.timestamp();
        let local = to_local(occurrence_date, tz);
        let is_due_date_basis = crate::service::item_series::is_due_date_basis(series);
        let due_date = is_due_date_basis.then(|| local.format("%Y-%m-%d %H:%M").to_string());
        let scheduled_date =
            (!is_due_date_basis).then(|| local.format("%Y-%m-%d %H:%M").to_string());
        Self {
            series_id: series.id.clone(),
            occurrence_ts,
            description: series.description.clone(),
            is_skipped,
            is_current,
            overdue: is_due_date_basis && occurrence_date < Utc::now(),
            due_date,
            scheduled_date,
            is_team_project,
            assignee_name: resolved_assignee_id
                .as_ref()
                .map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone())),
            update_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/task",
                series.id,
            ),
            skip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/skip",
                series.id,
            ),
            unskip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/unskip",
                series.id,
            ),
        }
    }
}

/// Stage C counterpart to `ProjectTaskSeriesOccurrenceView` — the edit form for a still-
/// virtual series occurrence, prefilled from `ItemSeries` + `occurrence_date` rather than a
/// real `Item` (none exists yet). Submitting it (`Save`) materializes the occurrence and
/// applies the edit in one step — see
/// `project_tasks::handlers::update_project_task_series_occurrence_form`.
#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_fields.html")]
pub struct ProjectTaskSeriesOccurrenceFields {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub name: String,
    pub description: String,
    pub due_date_input: String,
    pub due_time_input: String,
    pub scheduled_date_input: String,
    pub scheduled_time_input: String,
    pub scheduled_end_date_input: String,
    pub scheduled_end_time_input: String,
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub assigned_to_user_id: Option<String>,
    pub is_team_admin: bool,
    pub points_input: String,
    pub update_url: String,
}

impl ProjectTaskSeriesOccurrenceFields {
    #[allow(clippy::too_many_arguments)]
    pub fn from_series(
        series: &crate::domain::item_series::ItemSeries,
        occurrence_date: chrono::DateTime<Utc>,
        project_id: &str,
        is_team_project: bool,
        assignee_options: Vec<(String, String)>,
        // Stage 4 of docs/assignment-rotation-plan.md — same resolved-not-raw rationale
        // as `ProjectTaskSeriesOccurrenceView::from_series`.
        resolved_assignee_id: Option<String>,
        is_team_admin: bool,
        tz: i32,
    ) -> Self {
        let occurrence_ts = occurrence_date.timestamp();
        let local = to_local(occurrence_date, tz);
        let date_input = local.format("%Y-%m-%d").to_string();
        let time_input = local.format("%H:%M").to_string();
        let is_due_date_basis = crate::service::item_series::is_due_date_basis(series);
        Self {
            series_id: series.id.clone(),
            occurrence_ts,
            name: series.name.clone(),
            description: series.description.clone().unwrap_or_default(),
            due_date_input: if is_due_date_basis {
                date_input.clone()
            } else {
                String::new()
            },
            due_time_input: if is_due_date_basis {
                time_input.clone()
            } else {
                String::new()
            },
            scheduled_date_input: if is_due_date_basis {
                String::new()
            } else {
                date_input
            },
            scheduled_time_input: if is_due_date_basis {
                String::new()
            } else {
                time_input
            },
            scheduled_end_date_input: String::new(),
            scheduled_end_time_input: String::new(),
            is_team_project,
            assignee_options,
            assigned_to_user_id: resolved_assignee_id,
            is_team_admin,
            points_input: format_points_input(series.points),
            update_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/task",
                series.id,
            ),
        }
    }
}

#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_detail_page.html")]
pub struct ProjectTaskSeriesOccurrenceDetailPageTemplate {
    pub project_id: String,
    pub name: String,
    pub is_skipped: bool,
    pub view: String,
    pub child_create_url: String,
    pub edit_url: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_edit_page.html")]
pub struct ProjectTaskSeriesOccurrenceEditPageTemplate {
    pub name: String,
    pub fields: String,
    pub back_url: String,
    pub nav_html: String,
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
    series: &Arc<dyn crate::storage::sqlite::ItemSeriesRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(series_id) = &item.series_id else {
        return Ok(None);
    };
    // `delete_series` orphans rather than cascades (see its doc comment) — a materialized
    // item can outlive its parent series, so a missing series here means "no link to show",
    // not "this item doesn't exist."
    let series = match series.get_series(series_id).await {
        Ok(series) => series,
        Err(crate::storage::sqlite::RepoError::NotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
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

/// Redesign per docs/issues_and_features.md's calendar-view entry: a day cell only shows a
/// count hint now, not the items themselves (see `ProjectTasksCalendarPageTemplate`'s doc
/// comment for where the full list moved to) — so this only needs a tally, not the full
/// `CalendarTaskEntry`/`DateType` shapes the crammed-cell rendering used to require.
pub struct CalendarDay {
    pub date: String,
    pub day_number: u32,
    pub is_current_month: bool,
    pub is_today: bool,
    /// Whether this cell's `date` matches the calendar page's `?date=` query param — see
    /// `ProjectTasksCalendarPageTemplate::selected_date`'s doc comment.
    pub is_selected: bool,
    pub task_count: usize,
}

#[derive(Template)]
#[template(path = "project_tasks/calendar_page.html")]
pub struct ProjectTasksCalendarPageTemplate {
    pub project_id: String,
    pub month_label: String,
    pub month_iso: String,
    pub year: i32,
    pub month: u32,
    pub prev_year: i32,
    pub prev_month: u32,
    pub next_year: i32,
    pub next_month: u32,
    pub days: Vec<CalendarDay>,
    /// Redesign per docs/issues_and_features.md: day cells only show a count hint now: the
    /// full item list for a clicked day renders in a panel below the grid instead, populated
    /// either by this initial page render (`?date=` in the URL, e.g. from a bookmark or the
    /// `hx-push-url` a day click sets) or by the `.../calendar/day` htmx fragment route
    /// (`ProjectTasksCalendarDayPanelTemplate`, which shares this same partial template).
    pub selected_date_label: Option<String>,
    pub day_rows: Vec<String>,
    pub nav_html: String,
}

/// The day-list panel shown below the month grid — shares `calendar_day_panel.html` with
/// `ProjectTasksCalendarPageTemplate` above (which embeds it via `{% include %}` for the
/// initial page render) so the two never drift. Bound to its own struct here so the
/// `.../calendar/day` htmx fragment route can render just this panel's contents on a day
/// click, swapped into the page's `#calendar-day-list` via `hx-target`/`hx-swap="innerHTML"`.
#[derive(Template)]
#[template(path = "project_tasks/calendar_day_panel.html")]
pub struct ProjectTasksCalendarDayPanelTemplate {
    pub selected_date_label: Option<String>,
    pub day_rows: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{ItemSeriesRepo, MockItemSeriesRepo, RepoError};

    #[tokio::test]
    async fn resolve_series_link_returns_none_when_parent_series_was_deleted() {
        // Regression for the "delete a series, then its materialized occurrence 404s"
        // bug: `delete_series` orphans (see its doc comment) rather than cascading, so
        // `item.series_id` can point at a series row that no longer exists.
        let item = Item {
            series_id: Some("deleted-series".to_string()),
            ..Item::default()
        };
        let mut mock = MockItemSeriesRepo::new();
        mock.expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(mock);

        let result = resolve_series_link(&series, "p1", &item).await;

        assert_eq!(result.unwrap(), None);
    }
}
