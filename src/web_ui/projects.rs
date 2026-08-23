use crate::auth::AuthUser;
use crate::domain::team::TeamRole;
use crate::service::error::ItemError;
use crate::service::projects;
use crate::storage::sqlite::{ProjectRepo, TeamRepo, UserRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::http::HeaderName;
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub is_team_project: bool,
    /// Gates the row's "Delete" link — mirrors `service::projects::delete_project`'s own
    /// admin-and-not-your-personal-project rule, so the button never appears where the
    /// server would reject it anyway.
    pub can_delete: bool,
}

#[derive(Template)]
#[template(path = "projects/list_page.html")]
pub struct ProjectsListPageTemplate {
    pub rows: Vec<ProjectRow>,
    pub nav_html: String,
}

async fn render_projects_page(
    projects_repo: &Arc<dyn ProjectRepo>,
    users_repo: &Arc<dyn UserRepo>,
    user_id: &str,
) -> Result<Html<String>, ItemError> {
    let list = projects::list_projects(projects_repo, user_id).await?;
    let personal_project_id = users_repo.get(user_id).await?.personal_project_id;
    let mut rows = Vec::with_capacity(list.len());
    for p in list {
        let is_admin = projects_repo
            .member_role(&p.id, user_id)
            .await
            .map_err(|e| ItemError::Internal(format!("{e:?}")))?
            == Some(TeamRole::Admin);
        let is_personal = personal_project_id.as_deref() == Some(p.id.as_str());
        rows.push(ProjectRow {
            id: p.id,
            name: p.name,
            is_team_project: p.team_id.is_some(),
            can_delete: is_admin && !is_personal,
        });
    }
    let nav_html = nav::build_nav_html(
        projects_repo,
        user_id,
        ActiveContext::None,
        SidebarSection::None,
    )
    .await?;
    render(ProjectsListPageTemplate { rows, nav_html })
}

/// The full list of every project the user belongs to — nav's own project switcher (stage
/// B5f, see `nav.rs`) only ever shows a pill per project with no create/attach-team UI, so
/// this remains the one place to see every project (including any with no natural "current
/// section" link yet) and, eventually, manage project-level settings.
pub async fn projects_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects_repo): Extension<Arc<dyn ProjectRepo>>,
    Extension(users_repo): Extension<Arc<dyn UserRepo>>,
) -> Result<Html<String>, ItemError> {
    render_projects_page(&projects_repo, &users_repo, &auth_user.user_id).await
}

#[derive(serde::Deserialize)]
pub struct CreateProjectForm {
    name: String,
}

/// Mirrors `teams::create_team_form` — a new project only changes this page's own list, but
/// re-rendering the whole page (rather than a narrower row fragment) keeps this handler
/// consistent with that precedent instead of being the one exception.
pub async fn create_project_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects_repo): Extension<Arc<dyn ProjectRepo>>,
    Extension(users_repo): Extension<Arc<dyn UserRepo>>,
    Form(form): Form<CreateProjectForm>,
) -> Result<Html<String>, ItemError> {
    projects::create_project(&projects_repo, &form.name, &auth_user.user_id).await?;
    render_projects_page(&projects_repo, &users_repo, &auth_user.user_id).await
}

#[derive(Template)]
#[template(path = "projects/delete_dialog.html")]
pub struct DeleteProjectDialogTemplate {
    pub project_id: String,
    pub name: String,
}

/// Renders the double-confirmation delete dialog fragment (`projects/delete_dialog.html`)
/// into `#action-dialog` — see that template's own doc comment for why this one destructive
/// action gets a stronger barrier than this app's usual single `hx-confirm`. Re-checks
/// admin/not-personal via `get_project` + the same `member_role`/`personal_project_id` logic
/// `render_projects_page` uses for the row's `can_delete` gate, rather than trusting the
/// list page's own gate — a stale row (another tab, a role change since page load) must not
/// be able to reach a dialog whose submit would just 422 anyway.
pub async fn delete_project_dialog(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects_repo): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams_repo): Extension<Arc<dyn TeamRepo>>,
    Extension(users_repo): Extension<Arc<dyn UserRepo>>,
) -> Result<Html<String>, ItemError> {
    let project =
        projects::get_project(&projects_repo, &teams_repo, &project_id, &auth_user.user_id).await?;
    let is_admin = projects_repo
        .member_role(&project_id, &auth_user.user_id)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?
        == Some(TeamRole::Admin);
    if !is_admin {
        return Err(ItemError::Invalid(
            "only a project admin can do this".to_string(),
        ));
    }
    let personal_project_id = users_repo
        .get(&auth_user.user_id)
        .await?
        .personal_project_id;
    if personal_project_id.as_deref() == Some(project_id.as_str()) {
        return Err(ItemError::Invalid(
            "cannot delete your personal project".to_string(),
        ));
    }
    render(DeleteProjectDialogTemplate {
        project_id,
        name: project.name,
    })
}

/// Deletes the project (cascading to every item/series/subscription/activity-log row it
/// owns — see `SqliteProjectRepo::delete`) and redirects the whole page back to the project
/// list via `HX-Redirect`, since the deleted project's own row/detail no longer exists for
/// any narrower swap to target.
pub async fn delete_project_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects_repo): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams_repo): Extension<Arc<dyn TeamRepo>>,
    Extension(users_repo): Extension<Arc<dyn UserRepo>>,
) -> Result<Response, ItemError> {
    projects::delete_project(
        &projects_repo,
        &teams_repo,
        &users_repo,
        &project_id,
        &auth_user.user_id,
    )
    .await?;
    Ok((
        [(HeaderName::from_static("hx-redirect"), "/web/projects")],
        Html(String::new()),
    )
        .into_response())
}
