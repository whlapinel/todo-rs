use crate::domain::item_series::{ItemSeries, ItemSeriesChild};
use crate::web_ui::{format_display_date, to_local};
use askama::Template;

pub struct ProjectItemSeriesRow;

impl ProjectItemSeriesRow {
    pub fn from_series(
        s: &ItemSeries,
        tz: i32,
        assignee_name: Option<String>,
        child_count: usize,
    ) -> Row {
        Row {
            id: s.id.clone(),
            project_id: s.project_id.clone(),
            name: s.name.clone(),
            recurrence: s.recurrence.clone(),
            event_type: s.event_type.clone(),
            anchor_date_label: format_display_date(to_local(s.anchor_date, tz), true),
            item_type_label: s.item_type.label(),
            item_type_badge_color: s.item_type.badge_color(),
            assignee_name,
            points: s.points,
            // Task-series-only (`Item::validate`/`validate_series_priority` restrict `priority`
            // to a Task-typed series — see root CLAUDE.md's Priority section), but unlike
            // `points` it's never admin-gated, so it's always carried straight through.
            priority: s.priority,
            child_count,
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
    /// See `Row`'s (`components::row::Row`) identical `priority_label` doc comment.
    pub priority: Option<i32>,
    /// How many sub-item definitions this series carries — `0` renders nothing. Only the
    /// count, not the sub-items themselves: Stage 4 authors definitions, Stage 5 is what
    /// renders their occurrences. Without it a series' sub-items would be invisible until
    /// someone opened the Edit dialog.
    pub child_count: usize,
    pub duplicate_url: Option<String>,
}

/// One authored sub-item definition as the panel renders it. Every field is an editable
/// input in its own right — the panel has no separate read/edit modes, unlike the
/// item screens' detail-page-then-Edit-link convention (CLAUDE.md's row-editing convention),
/// because a definition has no detail page of its own to link to: it is a setting on a series,
/// not an item.
pub struct SeriesChildView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub days_before: i32,
    pub priority: Option<i32>,
}

impl SeriesChildView {
    pub fn from_child(c: &ItemSeriesChild) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            description: c.description.clone().unwrap_or_default(),
            days_before: c.days_before,
            priority: c.priority,
        }
    }
}

/// The Sub-items panel on the series edit dialog, and the fragment all three sub-item CRUD
/// routes return — each swaps the whole `#series-children` element, so a create/edit/delete
/// re-renders the list from storage rather than patching a single row client-side.
#[derive(Template)]
#[template(path = "project_item_series/children_panel.html")]
pub struct SeriesChildrenPanelTemplate {
    pub project_id: String,
    pub series_id: String,
    pub children: Vec<SeriesChildView>,
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
    /// (id, name) pairs for the project's Template items — Task-typed series only
    /// (see the create form's TASK/EVENT toggle script), populated regardless of
    /// which kind is initially selected since the toggle can flip client-side.
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
    /// Same shape as `NewProjectItemSeriesPageTemplate` — see its own field docs.
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
    /// Task-series-only, but — unlike `points` — not team-project-only and not
    /// admin-gated. See root CLAUDE.md's Priority section.
    pub priority: Option<i32>,
    /// A pre-rendered `SeriesChildrenPanelTemplate`, embedded with `|safe` the same way
    /// `nav_html` and the list page's rows are. Rendered to a string rather than nested as a
    /// sub-template because the three CRUD routes return that same fragment on its own.
    pub children_html: String,
}
