use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use crate::domain::item::Item;
use crate::service::error::ItemError;
use crate::storage::sqlite::ItemRepo;
use crate::web_ui::components::row::Row;
use crate::web_ui::dashboard::detail_url;
use crate::web_ui::to_local;
use chrono::Utc;

// ---- templates --------------------------------------------------------------

pub fn recurrence_basis_label(recurrence_basis: &Option<String>) -> String {
    match recurrence_basis.as_deref() {
        Some("COMPLETION_DATE") => "completion date".to_string(),
        Some("SCHEDULED_DATE") => "scheduled date".to_string(),
        Some(other) if other != "DUE_DATE" => other.to_string(),
        _ => "due date".to_string(),
    }
}

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
            expanded_row: item.due_date().is_some()
                || item.scheduled_date().is_some()
                || item.due_offset_days().is_some()
                || offset_label.is_some()
                || assignee_name.is_some(),
            has_children: item.has_children,
            offset_label,
            recurrence: item.recurrence_pattern(),
            assignee_name,
            complete_url: Some(format!("/web/projects/{project_id}/tasks/{}", item.id)),
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
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
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
            recurrence: item.recurrence_pattern(),
            recurrence_basis: item.recurrence_basis(),
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
    pub recurrence: Option<String>,
    pub recurrence_basis_label: String,
    pub offset_label: Option<String>,
    pub is_team_project: bool,
    pub assignee_name: Option<String>,
    /// See `tasks::TaskDetailView`'s identical field.
    pub linked_event: Option<(String, String)>,
}

impl ProjectTaskDetailView {
    pub fn from_item(
        item: &Item,
        project_id: &str,
        is_team_project: bool,
        names: &HashMap<String, String>,
        tz: i32,
        linked_event: Option<(String, String)>,
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
            recurrence: item.recurrence_pattern(),
            recurrence_basis_label: recurrence_basis_label(&item.recurrence_basis()),
            offset_label: offset_label_for(item),
            is_team_project,
            assignee_name: item
                .assigned_to_user_id()
                .map(|id| names.get(&id).cloned().unwrap_or(id)),
            linked_event,
        }
    }
}

/// Resolves the (name, detail-page URL) of the Event a task references via `sourceEventId`,
/// scoped to `project_id` — the project-scoped counterpart of `tasks::resolve_linked_event`/
/// `team_tasks::resolve_linked_event`. Still links to the event's *legacy* detail URL
/// (`dashboard::detail_url`), since there's no project-scoped Events screen yet (that's
/// stage B5b) — same bridge pattern the rest of stage B has used throughout.
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
    Ok(Some((event.name.clone(), detail_url(&event))))
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
    pub blank_recurrence: Option<String>,
    pub blank_recurrence_basis: Option<String>,
    pub blank_due_offset_days_input: String,
    pub blank_scheduled_date_input: String,
    pub blank_scheduled_time_input: String,
    pub blank_scheduled_end_date_input: String,
    pub blank_scheduled_end_time_input: String,
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
}

pub enum DateType {
    Due,
    ScheduledStart,
    ScheduledEnd,
}

pub struct CalendarDay {
    pub date: String,
    pub day_number: u32,
    pub is_current_month: bool,
    pub is_today: bool,
    pub tasks: Vec<CalendarTaskEntry>,
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
