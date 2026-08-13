use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use super::nav::{self, ActiveContext, SidebarSection};
use super::to_local;
use crate::service::error::ItemError;
use crate::storage::sqlite::{ItemRepo, TeamRepo};
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

/// Stage B5e: "cross-project query" — every row on this page always has a `team_id`
/// (assignment is a team-item-only concept), but now prefers the project-scoped URL
/// once the item actually carries a `project_id` (set at creation since stage B2c;
/// see docs/project-abstraction-plan.md). Falls back to the legacy team-owned URL for
/// an item that predates B2 and has never been touched since — same accepted,
/// bounded gap B2c's own implementation notes flagged, not new here.
fn detail_url(item: &Item, team_id: &str) -> String {
    match &item.project_id {
        Some(project_id) => match item.kind() {
            ItemKind::Task => format!("/web/projects/{project_id}/tasks/{}", item.id),
            ItemKind::Event => format!("/web/projects/{project_id}/events/{}", item.id),
            ItemKind::Simple => format!("/web/projects/{project_id}/simple-lists/{}", item.id),
            ItemKind::Template => format!("/web/projects/{project_id}/templates/{}", item.id),
        },
        None => match item.kind() {
            ItemKind::Task => format!("/web/team-tasks/{team_id}/{}", item.id),
            ItemKind::Event => format!("/web/team-events/{team_id}/{}", item.id),
            ItemKind::Simple => format!("/web/team-simple-lists/{team_id}/{}", item.id),
            ItemKind::Template => format!("/web/team-templates/{team_id}/{}", item.id),
        },
    }
}

impl AssignedItemRow {
    fn from_item(item: &Item, tz: i32) -> Option<Self> {
        let team_id = item.team_id.clone()?;
        Some(Self {
            id: item.id.clone(),
            detail_link: detail_url(item, &team_id),
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
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let items = repo
        .list_assigned(&auth_user.user_id)
        .await
        .map_err(ItemError::from)?;
    let rows = items
        .iter()
        // Assignment is a team-item-only concept (see CLAUDE.md's Recurrence/domain notes) —
        // every row here has a team_id; the filter_map is just a defensive skip, not an
        // expected case.
        .filter_map(|i| AssignedItemRow::from_item(i, tz))
        .map(|row| row.render())
        .collect::<Result<Vec<_>, _>>()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::None,
    )
    .await?;
    render(AssignedItemsPageTemplate { rows, nav_html })
}
