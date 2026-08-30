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
            // Simple items carry no `priority` concept (Task-only — see root CLAUDE.md's
            // Priority section).
            priority_label: None,
            // Simple items are never part of an `item_series` (Task-typed series only).
            series_current: None,
            assignee_name: None,
            blocked_by_names: Vec::new(),
            blocked_by_label: String::new(),
            blocked_by_links_html: String::new(),
            complete_url: None,
            edit_url: Some(format!(
                "/web/projects/{project_id}/simple-lists/{}/edit",
                item.id
            )),
            duplicate_url: None,
            add_child_url: Some(format!(
                "/web/projects/{project_id}/simple-lists/{}/add-child",
                item.id
            )),
            // Simple items have no "Save as template" route (see CLAUDE.md's own note on this).
            save_as_template_url: None,
            // Events-only (see `Row::add_linked_task_url`'s doc comment) — a Simple item uses
            // `add_child_url` instead.
            add_linked_task_url: None,
            move_url: (item.parent_item_id().is_some() || siblings.iter().any(|s| s.id != item.id))
                .then(|| format!("/web/projects/{project_id}/simple-lists/{}/move", item.id)),
            reschedule_url: None,
            assign_url: None,
            skip_url: None,
            toggle_complete_json: String::new(),
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
            // Stage 2 of docs/dialog-item-forms-plan.md — project_simple_lists' detail/edit/
            // new pages are now dialog fragments.
            detail_via_dialog: true,
            // The in-place expand feature is opt-in per render call, not per item — see
            // `project_simple_lists::render_rows_expandable`/`render_expandable_children`, used
            // by both the flat list page and the item detail page's Sub-items panel for any row
            // that itself `has_children`. `from_item` itself just leaves this at the default;
            // callers that haven't opted in (calendar screens, save-as-template, etc.) keep the
            // plain decorative `has_children` arrow and today's click-to-detail behavior.
            children_html: None,
            indent_class: "",
        }
    }
}

/// Opened from a row's "Add sub-item" row-action — see `project_tasks::templates::AddChildDialog`
/// for the full rationale (identical here, just posting to this screen's own create route).
#[derive(Template)]
#[template(path = "components/add_child_dialog.html")]
pub struct AddChildDialog {
    pub parent_item_id: String,
    pub parent_name: String,
    pub post_create_url: String,
    /// See `project_tasks::templates::AddChildDialog::post_batch_url` for the full rationale
    /// (identical here, just posting to this screen's own batch route).
    pub post_batch_url: String,
    /// See `project_tasks::templates::AddChildDialog::return_to` for the full rationale.
    pub return_to: String,
}

impl AddChildDialog {
    pub fn new(parent: &Item, project_id: &str, current_url: Option<&str>) -> Self {
        let return_to = super::sanitize_return_to(current_url, project_id)
            .unwrap_or_else(|| format!("/web/projects/{project_id}/simple-lists"));
        AddChildDialog {
            parent_item_id: parent.id.clone(),
            parent_name: parent.name.clone(),
            post_create_url: format!("/web/projects/{project_id}/simple-lists"),
            post_batch_url: format!("/web/projects/{project_id}/simple-lists/batch"),
            return_to,
        }
    }
}

/// Sentinel `target` value meaning "promote" — see
/// `project_tasks::templates::MOVE_TARGET_PARENT`'s identical rationale.
pub const MOVE_TARGET_PARENT: &str = "up";

/// See `project_tasks::templates::MoveDialog` for the full rationale (identical here, just
/// posting to this screen's own move route).
#[derive(Template)]
#[template(path = "components/move_dialog.html")]
pub struct MoveDialog {
    pub item_name: String,
    pub post_move_url: String,
    pub options: Vec<(String, String, bool)>,
}

impl MoveDialog {
    pub fn new(item: &Item, parent: Option<&Item>, siblings: &[Item], project_id: &str) -> Self {
        let mut options: Vec<(String, String, bool)> = Vec::new();
        if let Some(parent) = parent {
            options.push((MOVE_TARGET_PARENT.to_string(), parent.name.clone(), true));
        }
        options.extend(
            siblings
                .iter()
                .filter(|s| s.id != item.id)
                .map(|s| (s.id.clone(), s.name.clone(), false)),
        );
        MoveDialog {
            item_name: item.name.clone(),
            post_move_url: format!("/web/projects/{project_id}/simple-lists/{}/move", item.id),
            options,
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
    /// True only when this fragment was reached via the item's own full detail page's Edit
    /// link (`?redirect=1` on `GET .../edit`, see `project_simple_item_edit_page`) rather than
    /// a list row's "⋮ Edit"/detail-dialog Edit button. There's no `#item-{id}` row to
    /// target/select on a full detail page and the whole page needs its header/description
    /// refreshed on save, not just a row — see `detail_fields.html`'s own comment and
    /// `project_tasks::templates::ProjectTaskDetailFields.via_full_page`'s identical field.
    /// Always `false` for the post-save row+fields fragment (only reached when `redirect` was
    /// absent, i.e. the list-row case).
    pub via_full_page: bool,
}

impl ProjectSimpleItemDetailFields {
    pub fn from_item(item: &Item, project_id: &str, just_saved: bool, via_full_page: bool) -> Self {
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            just_saved,
            via_full_page,
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

/// Stage 2 of docs/dialog-item-forms-plan.md — the read-only detail dialog. Simple items have
/// no pre-rendered `view` partial to wrap (unlike `ProjectTaskDetailDialog`/
/// `ProjectEventDetailDialog`), so this holds `description` directly and
/// `detail_dialog.html` inlines it — see that template's own doc comment.
#[derive(Template)]
#[template(path = "project_simple_lists/detail_dialog.html")]
pub struct ProjectSimpleItemDetailDialog {
    pub name: String,
    pub description: Option<String>,
    pub edit_url: String,
}

impl ProjectSimpleItemDetailDialog {
    pub fn new(id: &str, project_id: &str, name: &str, description: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            description,
            edit_url: format!("/web/projects/{project_id}/simple-lists/{id}/edit"),
        }
    }
}

/// See docs/item-detail-full-page-retirement.md's revert note — this full page is back
/// (header with Edit/Back-or-parent-link/Delete, Move button, Sub-items management). `dialog`
/// is still rendered and embedded (`hidden`, in detail_page.html) purely so the row's own
/// hx-get (target=#action-dialog, select=#dialog-fragment — see components/row.html) can pluck
/// it out of this same route's response; as of 2026-08-25 nothing auto-opens it when this page
/// is loaded directly (see detail_page.html's own comment) — full-page access is popover-only
/// now (components/row_actions_menu.html's "View full page" entry), never from inside the
/// dialog itself, so the two navigation modes never race each other.
#[derive(Template)]
#[template(path = "project_simple_lists/detail_page.html")]
pub struct ProjectSimpleItemDetailPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    /// `Some((parent_name, parent_url))` when this item has a `parent_item_id` — reuses
    /// `project_tasks::templates::resolve_parent_link` (see that function's own doc comment;
    /// `project_events` already reuses it the same way). Drives the "⬆️ to {parent}" link that
    /// replaces the plain "Back to simple lists" one for a sub-item — see detail_page.html.
    /// `None` for a top-level item, or (gracefully) if the parent has since been deleted.
    pub parent_link: Option<(String, String)>,
    pub dialog: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_simple_lists/edit_page.html")]
pub struct ProjectSimpleItemEditPageTemplate {
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}
