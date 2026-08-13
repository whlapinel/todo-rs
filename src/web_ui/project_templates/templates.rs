use crate::domain::item::Item;
use askama::Template;

// ---- shared label helpers ----------------------------------------------------

pub fn format_offset_label(due_offset_days: Option<i32>) -> String {
    match due_offset_days {
        Some(0) => "on due date".to_string(),
        Some(n) if n > 0 => format!("+{n}d"),
        Some(n) => format!("{n}d"),
        None => "no offset".to_string(),
    }
}

// ---- templates --------------------------------------------------------------

/// Merges `templates::TemplateRow`/`team_templates::TeamTemplateRow` — `is_team_project`
/// gates the "Use..." form's assignee `<select>`, empty/inert on a personal project (same
/// shape as `project_tasks::ProjectTaskDetailFields`'s identical gate).
pub struct ProjectTemplateRow;

impl ProjectTemplateRow {
    pub fn from_item(
        item: &Item,
        project_id: &str,
        is_team_project: bool,
        assignee_options: Vec<(String, String)>,
    ) -> Row {
        Row {
            project_id: project_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            recurrence: item.recurrence_pattern(),
            is_team_project,
            assignee_options,
        }
    }
}

#[derive(Template)]
#[template(path = "project_templates/row.html")]
pub struct Row {
    pub project_id: String,
    pub id: String,
    pub name: String,
    pub recurrence: Option<String>,
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "project_templates/list_page.html")]
pub struct ProjectTemplatesListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_templates/rows_fragment.html")]
pub struct ProjectTemplatesRowsFragmentTemplate {
    pub rows: Vec<String>,
}

pub struct ProjectTemplateChildRow;

impl ProjectTemplateChildRow {
    pub fn from_item(project_id: &str, template_id: &str, item: &Item) -> ChildRow {
        ChildRow {
            project_id: project_id.to_string(),
            template_id: template_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            offset_label: format_offset_label(item.due_offset_days()),
        }
    }
}

#[derive(Template)]
#[template(path = "project_templates/child_row.html")]
pub struct ChildRow {
    pub project_id: String,
    pub template_id: String,
    pub id: String,
    pub name: String,
    pub offset_label: String,
}

#[derive(Template)]
#[template(path = "project_templates/children_fragment.html")]
pub struct ProjectTemplateChildrenFragmentTemplate {
    pub rows: Vec<String>,
}

#[derive(Template)]
#[template(path = "project_templates/detail_page.html")]
pub struct ProjectTemplateDetailPageTemplate {
    pub project_id: String,
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_templates/edit_page.html")]
pub struct ProjectTemplateEditPageTemplate {
    pub project_id: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub event_type: String,
    pub nav_html: String,
}

pub struct ProjectTemplateChildDetailFields;

impl ProjectTemplateChildDetailFields {
    pub fn from_item(project_id: &str, template_id: &str, item: &Item, just_saved: bool) -> ChildDetailFields {
        ChildDetailFields {
            project_id: project_id.to_string(),
            template_id: template_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            due_offset_days_input: item
                .due_offset_days()
                .map(|d| d.to_string())
                .unwrap_or_default(),
            just_saved,
        }
    }
}

#[derive(Template)]
#[template(path = "project_templates/child_detail_fields.html")]
pub struct ChildDetailFields {
    pub project_id: String,
    pub template_id: String,
    pub id: String,
    pub name: String,
    pub due_offset_days_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    pub just_saved: bool,
}

/// Read-only counterpart to `ChildDetailFields` — just the offset label, mirroring
/// `templates::TemplateChildDetailView`. No complete-toggle: template children have no
/// `complete` concept in their form at all.
pub struct ProjectTemplateChildDetailView;

impl ProjectTemplateChildDetailView {
    pub fn from_item(item: &Item) -> ChildDetailView {
        ChildDetailView {
            id: item.id.clone(),
            offset_label: format_offset_label(item.due_offset_days()),
        }
    }
}

#[derive(Template)]
#[template(path = "project_templates/child_detail_view.html")]
pub struct ChildDetailView {
    pub id: String,
    pub offset_label: String,
}

#[derive(Template)]
#[template(path = "project_templates/child_detail_page.html")]
pub struct ProjectTemplateChildDetailPageTemplate {
    pub project_id: String,
    pub template_id: String,
    pub id: String,
    pub name: String,
    pub view: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_templates/child_edit_page.html")]
pub struct ProjectTemplateChildEditPageTemplate {
    pub project_id: String,
    pub template_id: String,
    pub id: String,
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}
