use crate::domain::item::Item;
use crate::web_ui::components::row::Row;
use crate::web_ui::to_local;
use askama::Template;
use chrono::Utc;

// ---- templates --------------------------------------------------------------

/// Builds a generic `components::row::Row` rather than an Event-specific template of its
/// own — the same reuse `ProjectTaskRow` (stage B5a) established. An Event is always
/// top-level (see `project_events::require_event`'s doc comment on why it can never have
/// structural children), so unlike `ProjectTaskRow` this never sets `offset_label`,
/// `assignee_name`, or `siblings` — an Event never carries any of those concepts. `complete`
/// is hardcoded `false`/`complete_url: None` (mirroring `ProjectSimpleItemRow`'s identical
/// precedent) — `Item::validate` rejects `complete: true` for `ItemType::Event` outright, so
/// there's no toggle to render.
pub struct ProjectEventRow;

impl ProjectEventRow {
    pub fn from_item(item: &Item, project_id: &str, tz: i32) -> Row {
        Row {
            id: item.id.clone(),
            item_url: format!("/web/projects/{project_id}/events/{}", item.id),
            name: item.name.clone(),
            complete: false,
            due_date: item
                .due_date()
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
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
            expanded_row: true,
            has_children: false,
            offset_label: None,
            assignee_name: None,
            complete_url: None,
            duplicate_url: None,
            reschedule_url: None,
            toggle_complete_json: String::new(),
            siblings: Vec::new(),
            is_source_event_linked: false,
        }
    }
}

#[derive(Template)]
#[template(path = "project_events/detail_fields.html")]
pub struct ProjectEventDetailFields {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub scheduled_date_input: String,
    pub scheduled_time_input: String,
    pub scheduled_end_date_input: String,
    pub scheduled_end_time_input: String,
    pub due_date_input: String,
    pub due_time_input: String,
    pub event_type_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    pub just_saved: bool,
}

impl ProjectEventDetailFields {
    pub fn from_item(item: &Item, project_id: &str, tz: i32, just_saved: bool) -> Self {
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
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            due_date_input,
            due_time_input,
            event_type_input: item.event_type().unwrap_or_default(),
            just_saved,
        }
    }
}

/// Read-only counterpart to `ProjectEventDetailFields` — see `items.rs`'s `DetailView` for
/// the row-editing convention this mirrors. No complete-toggle here (unlike Task's): an
/// Event can never be marked complete, see `Item::validate`.
#[derive(Template)]
#[template(path = "project_events/detail_view.html")]
pub struct ProjectEventDetailView {
    pub id: String,
    pub description: Option<String>,
    pub scheduled_date: Option<String>,
    pub scheduled_end_date: Option<String>,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub event_type: Option<String>,
    /// See `project_tasks::templates::ProjectTaskDetailView::series_link`'s identical
    /// rationale.
    pub series_link: Option<(String, String)>,
}

impl ProjectEventDetailView {
    pub fn from_item(item: &Item, tz: i32, series_link: Option<(String, String)>) -> Self {
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
        let due_date = item.due_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_due_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        Self {
            id: item.id.clone(),
            description: item.description.clone(),
            scheduled_date,
            scheduled_end_date,
            due_date,
            overdue: item.is_overdue(Utc::now()),
            event_type: item.event_type(),
            series_link,
        }
    }
}

/// Resolves the (series_name, edit-page URL) of the `ItemSeries` this item was materialized
/// from — the Events counterpart of
/// `project_tasks::templates::resolve_series_link`, identical rationale.
pub async fn resolve_series_link(
    event_series: &std::sync::Arc<dyn crate::storage::sqlite::ItemSeriesRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, crate::service::error::ItemError> {
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
#[template(path = "project_events/rows_fragment.html")]
pub struct ProjectEventRowsFragmentTemplate {
    pub rows: Vec<String>,
    pub empty_message: String,
}

#[derive(Template)]
#[template(path = "project_events/list_page.html")]
pub struct ProjectEventsListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_events/new_page.html")]
pub struct NewProjectEventPageTemplate {
    pub project_id: String,
    pub blank_event_type_input: String,
    pub blank_scheduled_date_input: String,
    pub blank_scheduled_time_input: String,
    pub blank_scheduled_end_date_input: String,
    pub blank_scheduled_end_time_input: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_events/detail_page.html")]
pub struct ProjectEventDetailPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub view: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_events/edit_page.html")]
pub struct ProjectEventEditPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}

pub struct CalendarEventEntry {
    /// Unique across the whole grid — see `project_dashboard::ProjectDashboardCalendarEntry`'s
    /// identical field for why it's unconditional rather than `Option`.
    pub entry_id: String,
    pub href: String,
    pub name: String,
    pub time_label: Option<String>,
    /// `Some(...)` only for a virtual (unmaterialized) series occurrence (Stage 5 of
    /// docs/recurring-events-virtual-occurrences-rough-plan.md) — the template POSTs here
    /// instead of following `href` (which is `"#"` in that case).
    pub materialize_url: Option<String>,
    /// `Some(...)` only for a virtual occurrence too (Stage 6) — see
    /// `project_dashboard::ProjectDashboardCalendarEntry::skip_url`'s identical rationale.
    pub skip_url: Option<String>,
    pub is_virtual: bool,
    /// See `project_dashboard::ProjectDashboardCalendarEntry::is_skipped`'s identical
    /// rationale.
    pub is_skipped: bool,
    pub unskip_url: Option<String>,
}

pub struct CalendarDay {
    pub date: String,
    pub day_number: u32,
    pub is_current_month: bool,
    pub is_today: bool,
    pub events: Vec<CalendarEventEntry>,
}

#[derive(Template)]
#[template(path = "project_events/calendar_page.html")]
pub struct ProjectEventsCalendarPageTemplate {
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
