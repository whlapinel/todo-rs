use crate::domain::item_series::ItemSeries;
use crate::web_ui::{format_display_date, to_local};
use askama::Template;
use chrono::Utc;

pub struct ProjectItemSeriesRow;

impl ProjectItemSeriesRow {
    pub fn from_series(s: &ItemSeries, tz: i32, assignee_name: Option<String>) -> Row {
        Row {
            id: s.id.clone(),
            project_id: s.project_id.clone(),
            name: s.name.clone(),
            // docs/series-sub-items-plan.md: no web UI can create a child series yet
            // (Stage 3) — a genuine child reaching this row (e.g. created via the JSON
            // API/CLI) falls back to a blank recurrence / this render time's date rather
            // than panicking.
            recurrence: s.recurrence.clone().unwrap_or_default(),
            event_type: s.event_type.clone(),
            anchor_date_label: format_display_date(
                to_local(s.anchor_date.unwrap_or_else(Utc::now), tz),
                true,
            ),
            item_type_label: s.item_type.label(),
            item_type_badge_color: s.item_type.badge_color(),
            assignee_name,
            points: s.points,
            duplicate_url: Some(format!(
                "/web/projects/{}/series/{}/duplicate",
                s.project_id, s.id
            )),
        }
    }
}

#[derive(Template)]
#[template(path = "project_item_series/row.html")]
pub struct Row {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub recurrence: String,
    pub event_type: Option<String>,
    pub anchor_date_label: String,
    pub item_type_label: &'static str,
    pub item_type_badge_color: &'static str,
    pub assignee_name: Option<String>,
    pub points: Option<i32>,
    pub duplicate_url: Option<String>,
}

#[derive(Template)]
#[template(path = "project_item_series/list_page.html")]
pub struct ProjectItemSeriesListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_item_series/new_page.html")]
pub struct NewProjectItemSeriesPageTemplate {
    pub project_id: String,
    pub nav_html: String,
    /// Gates the Assign to/Points markup, same as `project_tasks`' own new-task form —
    /// both are Task-series-only (see the TASK/EVENT toggle script) and additionally
    /// only ever meaningful on a team-backed project.
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub is_team_admin: bool,
}

#[derive(Template)]
#[template(path = "project_item_series/edit_page.html")]
pub struct EditProjectItemSeriesPageTemplate {
    pub project_id: String,
    pub series_id: String,
    pub nav_html: String,
    pub name: String,
    pub description: String,
    pub is_task: bool,
    pub recurrence: String,
    /// "" (schedule) / "COMPLETION" / "DUE_DATE" — see `ItemSeries::basis`'s doc comment.
    pub basis: String,
    pub anchor_date: String,
    pub anchor_time: String,
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub assigned_to_user_id: Option<String>,
    /// Stage 4 of docs/assignment-rotation-plan.md — whether the Fixed/Rotate toggle
    /// starts on Rotate (non-empty rotation membership) or Fixed.
    pub is_rotating: bool,
    /// Which project members' checkboxes start checked when `is_rotating`.
    pub rotation_user_ids: Vec<String>,
    pub is_team_admin: bool,
    pub points: Option<i32>,
}
