use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::domain::team::TeamRole;
use crate::service::items::ItemError;
use crate::service::teams as team_service;
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
/// acting user on its own — it exists purely for URL shape. Every handler below
/// checks it against the authenticated `AuthUser` and rejects a mismatch rather than
/// silently substituting the authenticated id and ignoring the path value.
fn require_matching_user(auth: &AuthUser, path_user_id: &str) -> Result<(), ItemError> {
    if auth.user_id != path_user_id {
        return Err(ItemError::Invalid(
            "userId path segment does not match authenticated user".to_string(),
        ));
    }
    Ok(())
}

pub async fn create_team(
    input: input::CreateTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateTeamOutput, error::CreateTeamError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::CreateTeamError::from(to_msg(e)))?;
    let team_id = team_service::create_team(&teams, &projects, &input.name, &auth.user_id)
        .await
        .map_err(|e| error::CreateTeamError::from(to_msg(e)))?;
    Ok(output::CreateTeamOutput { team_id })
}

pub async fn get_team(
    input: input::GetTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::GetTeamOutput, error::GetTeamError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::GetTeamError::from(to_msg(e)))?;
    let team = team_service::get_team(&teams, &input.team_id)
        .await
        .map_err(|e| error::GetTeamError::from(to_msg(e)))?;
    Ok(output::GetTeamOutput {
        team_id: team.id,
        name: team.name,
    })
}

pub async fn update_team(
    input: input::UpdateTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateTeamOutput, error::UpdateTeamError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::UpdateTeamError::from(to_msg(e)))?;
    team_service::update_team(&teams, &input.team_id, &auth.user_id, &input.name)
        .await
        .map_err(|e| error::UpdateTeamError::from(to_msg(e)))?;
    Ok(output::UpdateTeamOutput {})
}

pub async fn list_teams(
    input: input::ListTeamsInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListTeamsOutput, error::ListTeamsError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::ListTeamsError::from(to_msg(e)))?;
    let memberships = team_service::list_teams(&teams, &auth.user_id)
        .await
        .map_err(|e| error::ListTeamsError::from(to_msg(e)))?;
    let teams = memberships
        .into_iter()
        .map(|m| model::TeamSummary {
            team_id: m.team.id,
            name: m.team.name,
            status: m.status,
            invited_by_name: m.invited_by_name,
        })
        .collect();
    Ok(output::ListTeamsOutput { teams })
}

pub async fn list_team_members(
    input: input::ListTeamMembersInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListTeamMembersOutput, error::ListTeamMembersError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::ListTeamMembersError::from(to_msg(e)))?;
    let members = team_service::list_team_members(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(|e| error::ListTeamMembersError::from(to_msg(e)))?
        .into_iter()
        .map(|m| model::TeamMemberSummary {
            user_id: m.user.id,
            first_name: m.user.first_name,
            last_name: m.user.last_name,
            status: m.status,
            role: m.role.as_str().to_string(),
            points: m.points,
        })
        .collect();
    Ok(output::ListTeamMembersOutput { members })
}

pub async fn set_team_member_role(
    input: input::SetTeamMemberRoleInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::SetTeamMemberRoleOutput, error::SetTeamMemberRoleError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::SetTeamMemberRoleError::from(to_msg(e)))?;
    let new_role = TeamRole::from_str(&input.role).map_err(|e| {
        error::SetTeamMemberRoleError::from(to_msg(ItemError::Invalid(format!(
            "invalid role: {e}"
        ))))
    })?;
    team_service::set_team_member_role(
        &teams,
        &input.team_id,
        &auth.user_id,
        &input.target_user_id,
        new_role,
    )
    .await
    .map_err(|e| error::SetTeamMemberRoleError::from(to_msg(e)))?;
    Ok(output::SetTeamMemberRoleOutput {})
}

pub async fn invite_team_member(
    input: input::InviteTeamMemberInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(users): server::Extension<Arc<dyn UserRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::InviteTeamMemberOutput, error::InviteTeamMemberError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::InviteTeamMemberError::from(to_msg(e)))?;
    team_service::invite_team_member(
        &teams,
        &users,
        &input.team_id,
        &auth.user_id,
        &input.invitee_user_id,
    )
    .await
    .map_err(|e| error::InviteTeamMemberError::from(to_msg(e)))?;
    Ok(output::InviteTeamMemberOutput {})
}

pub async fn accept_team_invite(
    input: input::AcceptTeamInviteInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::AcceptTeamInviteOutput, error::AcceptTeamInviteError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::AcceptTeamInviteError::from(to_msg(e)))?;
    team_service::accept_team_invite(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(|e| error::AcceptTeamInviteError::from(to_msg(e)))?;
    Ok(output::AcceptTeamInviteOutput {})
}

pub async fn leave_team(
    input: input::LeaveTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::LeaveTeamOutput, error::LeaveTeamError> {
    require_matching_user(&auth, &input.user_id)
        .map_err(|e| error::LeaveTeamError::from(to_msg(e)))?;
    team_service::leave_team(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(|e| error::LeaveTeamError::from(to_msg(e)))?;
    Ok(output::LeaveTeamOutput {})
}
