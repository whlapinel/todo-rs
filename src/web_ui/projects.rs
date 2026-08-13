use crate::auth::AuthUser;
use crate::service::error::ItemError;
use crate::service::projects;
use crate::storage::sqlite::ProjectRepo;
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

/// The full list of every project the user belongs to — nav's own project switcher (stage
/// B5f, see `nav.rs`) only ever shows a pill per project with no create/attach-team UI, so
/// this remains the one place to see every project (including any with no natural "current
/// section" link yet) and, eventually, manage project-level settings.
pub async fn projects_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects_repo): Extension<Arc<dyn ProjectRepo>>,
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
        &projects_repo,
        &auth_user.user_id,
        ActiveContext::None,
        SidebarSection::None,
    )
    .await?;
    render(ProjectsListPageTemplate { rows, nav_html })
}
