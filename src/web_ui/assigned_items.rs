use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use super::nav::{self, ActiveContext, SidebarSection};
use super::to_local;
use crate::service::error::ItemError;
use crate::storage::sqlite::{ItemRepo, ProjectRepo};
use askama::Template;
use axum::extract::Extension;
use axum::response::Html;
use chrono::Utc;
use std::sync::Arc;

use super::TzOffset;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

#[derive(Template)]
#[template(path = "assigned_items/row.html")]
struct AssignedItemRow {
    id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    overdue: bool,
    detail_link: String,
    toggle_complete_json: String,
}

/// Stage B5e/B5f: "cross-project query" — every row on this page always has a `team_id`
/// (assignment is a team-item-only concept), and links to the project-scoped URL for the
/// item's own `project_id` (set at creation since stage B2c; see
/// docs/project-abstraction-plan.md). Stage B5f removed the legacy per-type/team-scoped
/// screens this used to fall back to for an item that predated B2c and never got a
/// `project_id` — such a row is now skipped in `from_item` below rather than linking
/// somewhere that no longer exists (same accepted, bounded gap B2c's own implementation
/// notes flagged, now resolved by omission instead of a dead link).
fn detail_url(item: &Item, project_id: &str) -> String {
    match item.kind() {
        ItemKind::Task => format!("/web/projects/{project_id}/tasks/{}", item.id),
        ItemKind::Event => format!("/web/projects/{project_id}/events/{}", item.id),
        ItemKind::Simple => format!("/web/projects/{project_id}/simple-lists/{}", item.id),
        ItemKind::Template => format!("/web/projects/{project_id}/templates/{}", item.id),
    }
}

impl AssignedItemRow {
    fn from_item(item: &Item, tz: i32) -> Option<Self> {
        item.team_id.as_ref()?;
        let project_id = item.project_id.clone()?;
        Some(Self {
            id: item.id.clone(),
            detail_link: detail_url(item, &project_id),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date()
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            overdue: item.is_overdue(Utc::now()),
            toggle_complete_json: (!item.complete).to_string(),
        })
    }
}

#[derive(Template)]
#[template(path = "assigned_items/page.html")]
struct AssignedItemsPageTemplate {
    rows: Vec<String>,
    nav_html: String,
}

pub async fn assigned_items_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let items = repo
        .list_assigned(&auth_user.user_id)
        .await
        .map_err(ItemError::from)?;
    let rows = items
        .iter()
        // Assignment is a team-item-only concept (see CLAUDE.md's Recurrence/domain notes) —
        // every row here has a team_id; the filter_map also skips a row lacking project_id
        // (see detail_url's doc comment above).
        .filter_map(|i| AssignedItemRow::from_item(i, tz))
        .map(|row| row.render())
        .collect::<Result<Vec<_>, _>>()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        ActiveContext::None,
        SidebarSection::None,
    )
    .await?;
    render(AssignedItemsPageTemplate { rows, nav_html })
}
