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
}

pub(crate) fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
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
        parent_item_id: current.parent_item_id.clone(),
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
