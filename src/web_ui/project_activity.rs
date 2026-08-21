use super::nav::{self, ActiveContext, SidebarSection};
use super::{TzOffset, to_local};
use crate::auth::AuthUser;
use crate::domain::activity_log::ActivityLogEntry;
use crate::service::activity_log as activity_log_service;
use crate::service::error::ItemError;
use crate::service::projects::get_project;
use crate::service::teams as team_service;
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Path};
use axum::response::Html;
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Same server-side cap as `team_activity.rs`'s own — no pagination concept exists
/// anywhere in this app (see CLAUDE.md).
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
#[template(path = "project_activity/row.html")]
struct ProjectActivityRow {
    entry_id: String,
    project_id: String,
    user_name: String,
    item_name: String,
    points_delta: i32,
    reversed: bool,
    created_at: String,
    can_undo: bool,
}

impl ProjectActivityRow {
    fn from_entry(
        entry: &ActivityLogEntry,
        project_id: &str,
        names: &HashMap<String, String>,
        viewer_user_id: &str,
        tz: i32,
    ) -> Self {
        Self {
            entry_id: entry.id.clone(),
            project_id: project_id.to_string(),
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
#[template(path = "project_activity/page.html")]
struct ProjectActivityPageTemplate {
    project_id: String,
    rows: Vec<String>,
    nav_html: String,
}

/// Project-scoped counterpart to `team_activity.rs`'s `render_activity_page` — the
/// underlying data query has been `project_id`-keyed since stage B2 already (see
/// `list_activity_for_project`'s own doc comment), so this stage was originally a new
/// URL/screen on top of already-correct data, not a data-layer change. A personal
/// project's completions are logged too now (see docs/archived/archived_issues_and_features.md's "unify
/// completion-undo" note and `service::items::update_item`), so this is no longer the
/// "always empty for personal projects" case it started as — `list_activity_for_project`
/// naturally returns nothing only for a project with no completions logged yet at
/// all, personal or team.
async fn render_activity_page(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    project_id: &str,
    requester_user_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let project = get_project(projects, teams, project_id, requester_user_id).await?;
    let entries = activity_log
        .list_activity_for_project(project_id, ACTIVITY_LOG_LIMIT)
        .await
        .map_err(ItemError::from)?;
    let names = match &project.team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let rows = entries
        .iter()
        .map(|e| {
            ProjectActivityRow::from_entry(e, project_id, &names, requester_user_id, tz).render()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let active = ActiveContext::Project(project_id.to_string());
    let nav_html =
        nav::build_nav_html(projects, requester_user_id, active, SidebarSection::None).await?;
    render(ProjectActivityPageTemplate {
        project_id: project_id.to_string(),
        rows,
        nav_html,
    })
}

pub async fn project_activity_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    render_activity_page(
        &projects,
        &teams,
        &activity_log,
        &project_id,
        &auth_user.user_id,
        tz,
    )
    .await
}

pub async fn undo_project_activity_log_entry_form(
    Path((project_id, entry_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    // Project-scoped (see docs/archived/archived_issues_and_features.md's "unify completion-undo" note) — works for
    // a personal project too now, not just a team-backed one, since it's gated by
    // project membership rather than resolving a backing team.
    activity_log_service::undo_project_activity_log_entry(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &project_id,
        &entry_id,
        &auth_user.user_id,
        tz,
    )
    .await?;
    render_activity_page(
        &projects,
        &teams,
        &activity_log,
        &project_id,
        &auth_user.user_id,
        tz,
    )
    .await
}
