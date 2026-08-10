use crate::auth::AuthUser;
use crate::domain::activity_log::ActivityLogEntry;
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::activity_log as activity_log_service;
use crate::service::error::ItemError;
use crate::service::team_items::require_active_member;
use crate::service::teams as team_service;
use crate::storage::sqlite::{ActivityLogRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Path};
use axum::response::Html;
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Server-capped the same way `json_api::activity_log::list_team_activity_log` is — no
/// pagination concept exists anywhere in this app (see CLAUDE.md).
const ACTIVITY_LOG_LIMIT: i64 = 100;

async fn names_for(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<HashMap<String, String>, ItemError> {
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;
    Ok(members
        .into_iter()
        .map(|m| {
            (
                m.user.id.clone(),
                format!("{} {}", m.user.first_name, m.user.last_name),
            )
        })
        .collect())
}

#[derive(Template)]
#[template(path = "team_activity/row.html")]
struct ActivityLogRow {
    entry_id: String,
    team_id: String,
    user_name: String,
    item_name: String,
    points_delta: i32,
    reversed: bool,
    created_at: String,
    /// Only the entry's own `user_id` can undo it (no admin-override, matching
    /// `service::activity_log::undo_activity_log_entry`'s own restriction), and only
    /// while it's still unreversed.
    can_undo: bool,
}

impl ActivityLogRow {
    fn from_entry(
        entry: &ActivityLogEntry,
        names: &HashMap<String, String>,
        viewer_user_id: &str,
        tz: i32,
    ) -> Self {
        Self {
            entry_id: entry.id.clone(),
            team_id: entry.team_id.clone(),
            user_name: names
                .get(&entry.user_id)
                .cloned()
                .unwrap_or_else(|| entry.user_id.clone()),
            item_name: entry.item_name.clone(),
            points_delta: entry.points_delta,
            reversed: entry.reversed,
            created_at: to_local(entry.created_at, tz)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            can_undo: !entry.reversed && entry.user_id == viewer_user_id,
        }
    }
}

#[derive(Template)]
#[template(path = "team_activity/page.html")]
struct TeamActivityPageTemplate {
    team_id: String,
    rows: Vec<String>,
    nav_html: String,
}

async fn render_activity_page(
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    team_id: &str,
    requester_user_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    require_active_member(teams, team_id, requester_user_id).await?;
    let entries = activity_log
        .list_activity_for_team(team_id, ACTIVITY_LOG_LIMIT)
        .await
        .map_err(ItemError::from)?;
    let names = names_for(teams, team_id, requester_user_id).await?;
    let rows = entries
        .iter()
        .map(|e| ActivityLogRow::from_entry(e, &names, requester_user_id, tz).render())
        .collect::<Result<Vec<_>, _>>()?;
    let nav_html = nav::build_nav_html(
        teams,
        requester_user_id,
        ActiveContext::Team(team_id.to_string()),
        SidebarSection::None,
    )
    .await?;
    render(TeamActivityPageTemplate {
        team_id: team_id.to_string(),
        rows,
        nav_html,
    })
}

pub async fn team_activity_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    render_activity_page(&teams, &activity_log, &team_id, &auth_user.user_id, tz).await
}

pub async fn undo_activity_log_entry_form(
    Path((team_id, entry_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    activity_log_service::undo_activity_log_entry(
        &teams,
        &activity_log,
        &team_id,
        &entry_id,
        &auth_user.user_id,
    )
    .await?;
    render_activity_page(&teams, &activity_log, &team_id, &auth_user.user_id, tz).await
}
