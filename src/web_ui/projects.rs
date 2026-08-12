use crate::auth::AuthUser;
use crate::service::error::ItemError;
use crate::service::projects;
use crate::storage::sqlite::{ProjectRepo, TeamRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use askama::Template;
use axum::extract::Extension;
use axum::response::Html;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub is_team_project: bool,
}

#[derive(Template)]
#[template(path = "projects/list_page.html")]
pub struct ProjectsListPageTemplate {
    pub rows: Vec<ProjectRow>,
    pub nav_html: String,
}

/// Minimal on-ramp to the new project-scoped screens (stage B5a) — not a full project
/// switcher (that's stage B5f). Just enough to reach `/web/projects/:project_id/tasks`
/// manually until nav grows real project awareness.
pub async fn projects_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects_repo): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let list = projects::list_projects(&projects_repo, &auth_user.user_id).await?;
    let rows = list
        .into_iter()
        .map(|p| ProjectRow {
            id: p.id,
            name: p.name,
            is_team_project: p.team_id.is_some(),
        })
        .collect();
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::None,
    )
    .await?;
    render(ProjectsListPageTemplate { rows, nav_html })
}
