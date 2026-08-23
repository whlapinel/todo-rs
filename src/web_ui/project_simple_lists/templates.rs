use crate::domain::item::Item;
use crate::web_ui::components::row::Row;
use askama::Template;

// ---- templates --------------------------------------------------------------

/// `Row`'s third real caller (after `ProjectTaskRow`/`ProjectEventRow` — see
/// `docs/project-abstraction-plan.md` stage B5c). Simple items carry none of
/// `Row`'s optional fields (no dates, no recurrence, no assignment/points, no
/// complete concept, no `sourceEventId`), so `expanded_row` is always `false` — the
/// metadata line never has anything to show, matching legacy `simple_lists/row.html`'s
/// single-line-only markup.
pub struct ProjectSimpleItemRow;

impl ProjectSimpleItemRow {
    pub fn from_item(item: &Item, project_id: &str, siblings: &[&Item]) -> Row {
        Row {
            id: item.id.clone(),
            item_url: format!("/web/projects/{project_id}/simple-lists/{}", item.id),
            name: item.name.clone(),
            complete: false,
            due_date: None,
            overdue: false,
            scheduled_date: None,
            scheduled_end_date: None,
            event_type: None,
            expanded_row: false,
            has_children: item.has_children,
            offset_label: None,
            assignee_name: None,
            complete_url: None,
            edit_url: Some(format!(
                "/web/projects/{project_id}/simple-lists/{}/edit",
                item.id
            )),
            duplicate_url: None,
            reschedule_url: None,
            assign_url: None,
            skip_url: None,
            toggle_complete_json: String::new(),
            siblings: siblings
                .iter()
                .filter(|s| s.id != item.id)
                .map(|s| (s.id.clone(), s.name.clone()))
                .collect(),
            is_source_event_linked: false,
            show_complete: false,
            confirmation: None,
            dismiss_after_ms: None,
            // Simple items are never Google-Calendar-imported (only Events are).
            is_imported: false,
            // Calendar-only fields — see `Row`'s doc comments; Simple items never appear on a
            // calendar screen at all (see `ItemKind::Simple`'s exclusion there).
            type_badge: None,
            parent_name: None,
            project_name: None,
            // Not yet converted to dialog fragments — see Stage 2 of
            // docs/dialog-item-forms-plan.md.
            detail_via_dialog: false,
        }
    }
}

#[derive(Template)]
#[template(path = "project_simple_lists/detail_fields.html")]
pub struct ProjectSimpleItemDetailFields {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    pub just_saved: bool,
}

impl ProjectSimpleItemDetailFields {
    pub fn from_item(item: &Item, project_id: &str, just_saved: bool) -> Self {
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            just_saved,
        }
    }
}

#[derive(Template)]
#[template(path = "project_simple_lists/rows_fragment.html")]
pub struct ProjectSimpleItemRowsFragmentTemplate {
    pub rows: Vec<String>,
    pub empty_message: String,
}

#[derive(Template)]
#[template(path = "project_simple_lists/list_page.html")]
pub struct ProjectSimpleListsListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    /// `Some("{n} pts")` on a team-backed project (the viewer's own balance — see
    /// `service::teams::member_points`), `None` on a personal project. Simple items
    /// themselves never carry points (see this module's own doc comment), but every
    /// other project-scoped list screen shows the viewer's running total here too
    /// (mirrors `team_simple_lists::team_simple_lists_page`'s existing behavior).
    pub points_label: Option<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_simple_lists/new_page.html")]
pub struct NewProjectSimpleItemPageTemplate {
    pub project_id: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_simple_lists/detail_page.html")]
pub struct ProjectSimpleItemDetailPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_top_level: bool,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_simple_lists/edit_page.html")]
pub struct ProjectSimpleItemEditPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}
