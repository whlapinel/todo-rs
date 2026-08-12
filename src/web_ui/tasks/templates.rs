use std::sync::Arc;

use askama::Template;
use crate::{domain::item::Item}; 
use super::super::{dashboard::detail_url, super::service::error::ItemError, super::storage::sqlite::ItemRepo};
use super::super::to_local;
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

#[derive(Template)]
#[template(path = "tasks/row.html")]
pub struct TaskRow {
    expanded_row: bool,
    id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    overdue: bool,
    scheduled_date: Option<String>,
    has_children: bool,
    offset_label: Option<String>,
    recurrence: Option<String>,
    toggle_complete_json: String,
    /// (id, name) of every other item rendered alongside this one in the same list —
    /// i.e. this item's actual siblings, since `render_rows` is only ever called with a
    /// single sibling group (a full top-level list or one parent's children) at a time.
    /// Populates the row's "subordinate under…" picker (see `subordinate_task_form`);
    /// empty for an only child / sole top-level item.
    siblings: Vec<(String, String)>,
    /// True if this task references an Event via `sourceEventId` — its row hides the
    /// "subordinate under…" picker even when siblings exist, since giving it a
    /// `parentItemId` too would conflict with the reference (see `Item::validate`).
    is_source_event_linked: bool,
}

impl TaskRow {
    pub fn from_item(item: &Item, siblings: &[&Item], tz: i32) -> Self {
        let offset_label = offset_label_for(item);
        Self {
            id: item.id.clone(),
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
                || offset_label.is_some(),
            has_children: item.has_children,
            offset_label,
            recurrence: item.recurrence_pattern(),
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

pub fn format_offset_input(due_offset_days: Option<i32>) -> String {
    due_offset_days.map(|d| d.to_string()).unwrap_or_default()
}

#[derive(Template)]
#[template(path = "tasks/detail_fields.html")]
pub struct TaskDetailFields {
    pub id: String,
    pub name: String,
    pub description: String,
    pub complete: bool,
    pub is_top_level: bool,
    /// True for a structural child or an event-linked task — its due date is always
    /// computed from `due_offset_days`, never manually typed (issues #21/#22), so the
    /// template swaps the free-form due-date/scheduled-window inputs for a read-only
    /// computed display and shows the offset field instead of recurrence controls.
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
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    pub just_saved: bool,
}

impl TaskDetailFields {
    pub fn from_item(item: &Item, tz: i32, just_saved: bool) -> Self {
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
            just_saved,
        }
    }
}

/// Read-only counterpart to `TaskDetailFields` — see `items.rs`'s `DetailView` for the
/// row-editing convention this mirrors (complete-toggle lives here too).
#[derive(Template)]
#[template(path = "tasks/detail_view.html")]
pub struct TaskDetailView {
    pub id: String,
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
    /// (name, detail-page URL) of the Event this task references via `sourceEventId`,
    /// resolved by the caller (a plain lookup, since this struct's own `from_item` stays
    /// pure/repo-free like every other `*_view`/`*_fields` struct in this module).
    pub linked_event: Option<(String, String)>,
}

impl TaskDetailView {
    pub fn from_item(item: &Item, tz: i32, linked_event: Option<(String, String)>) -> Self {
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
            linked_event,
        }
    }
}

/// Resolves the (name, detail-page URL) of the Event a task references via `sourceEventId`,
/// for `TaskDetailView`'s read-only "Linked to event" line. `None` if the task doesn't
/// reference one at all.
pub async fn resolve_linked_event(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(event_id) = item.source_event_id() else {
        return Ok(None);
    };
    let event = repo
        .get(user_id, &event_id)
        .await
        .map_err(ItemError::from)?;
    Ok(Some((event.name.clone(), detail_url(&event))))
}

#[derive(Template)]
#[template(path = "tasks/rows_fragment.html")]
pub struct TaskRowsFragmentTemplate {
    pub rows: Vec<String>,
    pub empty_message: String,
}

#[derive(Template)]
#[template(path = "tasks/list_page.html")]
pub struct TasksListPageTemplate {
    pub rows: Vec<String>,
    pub show_complete: bool,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "tasks/new_page.html")]
pub struct NewTaskPageTemplate {
    pub show_complete: bool,
    pub blank_recurrence: Option<String>,
    pub blank_recurrence_basis: Option<String>,
    pub blank_due_offset_days_input: String,
    pub blank_scheduled_date_input: String,
    pub blank_scheduled_time_input: String,
    pub blank_scheduled_end_date_input: String,
    pub blank_scheduled_end_time_input: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "tasks/detail_page.html")]
pub struct TaskDetailPageTemplate {
    pub id: String,
    pub name: String,
    pub complete: bool,
    pub view: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "tasks/edit_page.html")]
pub struct TaskEditPageTemplate {
    pub id: String,
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
#[template(path = "tasks/calendar_page.html")]
pub struct TasksCalendarPageTemplate {
    pub month_label: String,
    pub month_iso: String,
    pub prev_year: i32,
    pub prev_month: u32,
    pub next_year: i32,
    pub next_month: u32,
    pub days: Vec<CalendarDay>,
    pub nav_html: String,
}

