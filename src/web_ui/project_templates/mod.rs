pub mod handlers;
pub mod templates;

use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use askama::Template;
use axum::response::Html;

pub(crate) fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every template-detail route below to the item actually being a Template —
/// mirrors `project_simple_lists::require_simple`/`project_tasks::require_task`. Unlike
/// those screens, the *children* of a template are not gated the same way: a personal
/// template's children are auto-upgraded to `Template` kind by `items::create_item`
/// (parent-is-Template auto-detection), but a *team* template's children stay `Task`-kind
/// (`team_items::create_team_item` has no such auto-detection — see
/// docs/project-abstraction-plan.md's stage B5d notes, which reproduces this existing
/// asymmetry from `templates.rs`/`team_templates.rs` verbatim rather than "fixing" it here).
/// So only the template item itself is checked; a child route trusts `parent_item_id`
/// instead.
pub(crate) fn require_project_template(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Template {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

pub(crate) fn parse_offset(form_value: &Option<String>) -> Option<i32> {
    form_value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

pub(crate) fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
