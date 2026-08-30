pub mod handlers;
pub mod templates;

use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::project_items::list_project_items_unchecked;
use crate::storage::sqlite::ItemRepo;
use crate::web_ui::project_simple_lists::templates::{
    ProjectSimpleItemRow, ProjectSimpleItemRowsFragmentTemplate,
};
use askama::Template;
use async_recursion::async_recursion;
use axum::response::Html;
use std::sync::Arc;

pub(crate) fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being Simple — mirrors
/// `simple_lists::require_simple`/`team_simple_lists::require_team_simple`. This screen's
/// forms hardcode `itemType: SIMPLE` on every create/update (no Kind selector), so a
/// Task/Event item's id reaching one of these handlers must 404 rather than render a form
/// that would silently reclassify it back to Simple on save.
pub(crate) fn require_simple(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Simple {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Deliberately no date/scheduling/recurrence/offset/complete/assignment/points fields
// anywhere in this module — `Item::validate` rejects all of those (plus `complete: true`)
// for `ItemType::Simple`, and (per `team_simple_lists.rs`'s own precedent) Simple items
// never carry assignment/points even on a team-backed project, so there is nothing for a
// form on this screen to ever legitimately set beyond name/description/parent.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSimpleItemForm {
    name: Option<String>,
    description: Option<String>,
    parent_item_id: Option<String>,
    /// Set on the standalone `/projects/:project_id/simple-lists/new` page's create forms
    /// and on every edit form's "Save and close" submission — see `tasks.rs`'s identical
    /// field for the full rationale.
    redirect: Option<String>,
    /// See `project_tasks::ProjectTaskForm::return_to`'s identical rationale.
    return_to: Option<String>,
}

pub(crate) fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// See `project_tasks::sanitize_return_to`'s identical rationale, project_simple_lists' own
/// copy per this codebase's "duplicate small per-screen helpers" precedent (matching
/// `redirect_to_project_simple_lists` already being its own copy of
/// `project_tasks::redirect_to_project_tasks`).
pub(crate) fn sanitize_return_to(url: Option<&str>, project_id: &str) -> Option<String> {
    let raw = url?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = raw
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
        .unwrap_or(raw);
    let prefix = format!("/web/projects/{project_id}/");
    path.starts_with(&prefix).then(|| path.to_string())
}

fn overlay_required_str(form_value: &Option<String>, current: &str) -> String {
    match form_value {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => current.to_string(),
    }
}

fn overlay_str(form_value: &Option<String>, current: Option<String>) -> Option<String> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
}

pub(crate) fn create_params_from_form(
    project_id: &str,
    form: &ProjectSimpleItemForm,
) -> crate::service::project_items::CreateProjectItemParams {
    crate::service::project_items::CreateProjectItemParams {
        project_id: project_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        description: non_empty(&form.description),
        parent_item_id: non_empty(&form.parent_item_id),
        item_type: Some(ItemKind::Simple),
        ..Default::default()
    }
}

pub(crate) fn update_params_from_form(
    project_id: &str,
    item_id: &str,
    current: &Item,
    form: &ProjectSimpleItemForm,
) -> crate::service::project_items::UpdateProjectItemParams {
    crate::service::project_items::UpdateProjectItemParams {
        project_id: project_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        description: overlay_str(&form.description, current.description.clone()),
        complete: false,
        parent_item_id: current.parent_item_id(),
        item_type: Some(ItemKind::Simple),
        ..Default::default()
    }
}

// ---- shared rendering helpers ------------------------------------------------

pub(crate) fn render_rows(items: &[Item], project_id: &str) -> Result<Vec<String>, ItemError> {
    let all: Vec<&Item> = items.iter().collect();
    all.iter()
        .map(|i| ProjectSimpleItemRow::from_item(i, project_id, &all).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// Fixed left-padding class for a nested row at `depth` levels below the flat list's own
/// top-level rows — mirrors `project_tasks::indent_class`'s identical rationale (Tailwind's
/// compiler needs literal, not computed, class names, so nesting caps at 3 indent steps).
fn indent_class(depth: u8) -> &'static str {
    match depth {
        0 => "",
        1 => "pl-8",
        2 => "pl-12",
        _ => "pl-16",
    }
}

/// Number of ancestors above `item` (0 for a top-level item) — mirrors
/// `project_tasks::depth_of_item`'s identical rationale: a single-row swap (e.g.
/// `update_project_simple_item_form`'s plain-row branch) has no depth on hand the way a
/// whole-subtree walk does, so without this it falls back to `from_item`'s default
/// `indent_class: ""` and a nested row visually jumps up to look shallower than it is.
pub(crate) async fn depth_of_item(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item: &Item,
) -> Result<u8, ItemError> {
    let mut depth = 0u8;
    let mut parent_id = item.parent_item_id();
    while let Some(id) = parent_id {
        let parent = match repo.get_by_project(project_id, &id).await {
            Ok(parent) => parent,
            Err(crate::storage::sqlite::RepoError::NotFound) => break,
            Err(e) => return Err(e.into()),
        };
        depth = depth.saturating_add(1);
        parent_id = parent.parent_item_id();
    }
    Ok(depth)
}

/// Recursively renders `parent_item_id`'s full descendant subtree as ready-to-insert `<li>`
/// markup, each descendant's own row in turn carrying its own nested `children_html` — the
/// flat Simple Lists screen's in-place "expand to view sub-items" feature, mirroring
/// `project_tasks::render_expandable_children`. Unlike that Tasks version, there's no
/// `show_complete` filter to apply (Simple items have no `complete` concept at all — see
/// `ProjectSimpleItemRow::from_item`'s doc comment).
#[async_recursion]
async fn render_expandable_children(
    repo: &Arc<dyn ItemRepo>,
    parent_item_id: &str,
    project_id: &str,
    depth: u8,
) -> Result<String, ItemError> {
    let children =
        list_project_items_unchecked(repo, project_id, Some(parent_item_id.to_string())).await?;
    let all: Vec<&Item> = children.iter().collect();
    let mut html = String::new();
    for i in &all {
        let mut row = ProjectSimpleItemRow::from_item(i, project_id, &all);
        row.indent_class = indent_class(depth);
        if i.has_children {
            row.children_html =
                Some(render_expandable_children(repo, &i.id, project_id, depth + 1).await?);
        }
        html.push_str(&row.render()?);
    }
    Ok(html)
}

/// The flat list page's own row-building — opts each top-level row with children into the
/// in-place expand feature (see `Row::children_html`'s doc comment) by eagerly inlining its
/// whole descendant subtree, so the browser's expand/collapse toggle never round-trips to the
/// server. Unlike plain `render_rows` above (used by the create-fragment route, which stays
/// non-expandable), this is async and needs `repo` to walk each branch. The children-fragment
/// route (the item detail page's Sub-items panel) also opts into this now, so a sub-item with
/// its own children toggles in place instead of falling through to the dialog/navigation
/// name-click branch (see `Row::children_html`'s doc comment).
pub(crate) async fn render_rows_expandable(
    repo: &Arc<dyn ItemRepo>,
    items: &[Item],
    project_id: &str,
) -> Result<Vec<String>, ItemError> {
    let all: Vec<&Item> = items.iter().collect();
    let mut rows = Vec::with_capacity(all.len());
    for i in &all {
        let mut row = ProjectSimpleItemRow::from_item(i, project_id, &all);
        if i.has_children {
            row.children_html = Some(render_expandable_children(repo, &i.id, project_id, 1).await?);
        }
        rows.push(row.render()?);
    }
    Ok(rows)
}

/// `list_project_items_unchecked` already scopes to top-level, non-Template items — this
/// narrows further to `Simple`. No sort key: unlike Tasks/Events there's no date field to
/// order by, mirroring `simple_lists::list_simple_items`/`team_simple_lists::list_team_simple_items`.
pub(crate) async fn list_project_simple_items(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
) -> Result<Vec<Item>, ItemError> {
    let mut items = list_project_items_unchecked(repo, project_id, None).await?;
    items.retain(|i| i.kind() == ItemKind::Simple);
    Ok(items)
}

/// The full sibling group (including the item itself) a given item belongs to — see
/// `tasks::sibling_group`'s identical rationale.
pub(crate) async fn sibling_group(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    parent_item_id: Option<&str>,
) -> Result<Vec<Item>, ItemError> {
    match parent_item_id {
        Some(pid) => list_project_items_unchecked(repo, project_id, Some(pid.to_string())).await,
        None => list_project_simple_items(repo, project_id).await,
    }
}

pub(crate) async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    parent_item_id: Option<&str>,
) -> Result<Html<String>, ItemError> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            list_project_items_unchecked(repo, project_id, Some(parent_id.to_string())).await?,
            "No sub-items yet.",
        )
    } else {
        (
            list_project_simple_items(repo, project_id).await?,
            "No items yet.",
        )
    };
    let rows = render_rows(&items, project_id)?;
    render(ProjectSimpleItemRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
}
