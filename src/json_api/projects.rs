use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::domain::team::TeamRole;
use crate::service::items::ItemError;
use crate::service::projects as project_service;
use crate::storage::sqlite::{ProjectRepo, TeamRepo, UserRepo};
use std::str::FromStr;
use std::sync::Arc;
use todo_server_sdk::{error, input, model, output, server};

fn to_msg(e: ItemError) -> error::PeoplesRepublicOfListsError {
    match e {
        ItemError::NotFound => not_found(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg),
    }
}

/// The `{userId}` path segment is client-supplied and must never be trusted as the
/// acting user on its own — see `json_api::teams`'s identical helper/comment.
fn require_matching_user(auth: &AuthUser, path_user_id: &str) -> Result<(), ItemError> {
    if auth.user_id != path_user_id {
        return Err(ItemError::Invalid(
            "userId path segment does not match authenticated user".to_string(),
        ));
    }
    Ok(())
}

pub async fn create_project(
    input: input::CreateProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateProjectOutput, error::CreateProjectError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::CreateProjectError::from(to_msg(e)))?;
    let project_id = project_service::create_project(&projects, &input.name, &auth.user_id)
        .await
        .map_err(|e| error::CreateProjectError::from(to_msg(e)))?;
    Ok(output::CreateProjectOutput { project_id })
}

pub async fn get_project(
    input: input::GetProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::GetProjectOutput, error::GetProjectError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::GetProjectError::from(to_msg(e)))?;
    let project = project_service::get_project(&projects, &teams, &input.project_id, &auth.user_id)
        .await
        .map_err(|e| error::GetProjectError::from(to_msg(e)))?;
    Ok(output::GetProjectOutput {
        project_id: project.id,
        name: project.name,
        owner_user_id: project.owner_user_id,
        team_id: project.team_id,
    })
}

pub async fn update_project(
    input: input::UpdateProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateProjectOutput, error::UpdateProjectError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::UpdateProjectError::from(to_msg(e)))?;
    project_service::update_project(
        &projects,
        &teams,
        &input.project_id,
        &auth.user_id,
        &input.name,
    )
    .await
    .map_err(|e| error::UpdateProjectError::from(to_msg(e)))?;
    Ok(output::UpdateProjectOutput {})
}

pub async fn delete_project(
    input: input::DeleteProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(users): server::Extension<Arc<dyn UserRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DeleteProjectOutput, error::DeleteProjectError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::DeleteProjectError::from(to_msg(e)))?;
    project_service::delete_project(&projects, &teams, &users, &input.project_id, &auth.user_id)
        .await
        .map_err(|e| error::DeleteProjectError::from(to_msg(e)))?;
    Ok(output::DeleteProjectOutput {})
}

pub async fn list_projects(
    input: input::ListProjectsInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListProjectsOutput, error::ListProjectsError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::ListProjectsError::from(to_msg(e)))?;
    let projects = project_service::list_projects(&projects, &auth.user_id)
        .await
        .map_err(|e| error::ListProjectsError::from(to_msg(e)))?
        .into_iter()
        .map(|p| model::ProjectSummary {
            project_id: p.id,
            name: p.name,
            owner_user_id: p.owner_user_id,
            team_id: p.team_id,
        })
        .collect();
    Ok(output::ListProjectsOutput { projects })
}

pub async fn list_project_members(
    input: input::ListProjectMembersInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListProjectMembersOutput, error::ListProjectMembersError> {
    let members =
        project_service::list_project_members(&projects, &teams, &input.project_id, &auth.user_id)
            .await
            .map_err(|e| error::ListProjectMembersError::from(to_msg(e)))?
            .into_iter()
            .map(|m| model::ProjectMemberSummary {
                user_id: m.user.id,
                first_name: m.user.first_name,
                last_name: m.user.last_name,
                role: m.role.as_str().to_string(),
                points: m.points,
            })
            .collect();
    Ok(output::ListProjectMembersOutput { members })
}

pub async fn set_project_member_role(
    input: input::SetProjectMemberRoleInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::SetProjectMemberRoleOutput, error::SetProjectMemberRoleError> {
    let new_role = TeamRole::from_str(&input.role).map_err(|e| {
        error::SetProjectMemberRoleError::from(to_msg(ItemError::Invalid(format!(
            "invalid role: {e}"
        ))))
    })?;
    project_service::set_project_member_role(
        &projects,
        &teams,
        &input.project_id,
        &auth.user_id,
        &input.target_user_id,
        new_role,
    )
    .await
    .map_err(|e| error::SetProjectMemberRoleError::from(to_msg(e)))?;
    Ok(output::SetProjectMemberRoleOutput {})
}

pub async fn attach_team_to_project(
    input: input::AttachTeamToProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::AttachTeamToProjectOutput, error::AttachTeamToProjectError> {
    project_service::attach_team_to_project(
        &projects,
        &teams,
        &input.project_id,
        &auth.user_id,
        &input.team_id,
    )
    .await
    .map_err(|e| error::AttachTeamToProjectError::from(to_msg(e)))?;
    Ok(output::AttachTeamToProjectOutput {})
}

pub async fn detach_team_from_project(
    input: input::DetachTeamFromProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DetachTeamFromProjectOutput, error::DetachTeamFromProjectError> {
    project_service::detach_team_from_project(&projects, &teams, &input.project_id, &auth.user_id)
        .await
        .map_err(|e| error::DetachTeamFromProjectError::from(to_msg(e)))?;
    Ok(output::DetachTeamFromProjectOutput {})
}
