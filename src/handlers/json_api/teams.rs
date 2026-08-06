use super::{internal, not_found};
use crate::service::items::ItemError;
use crate::service::teams as team_service;
use crate::storage::sqlite::{TeamRepo, UserRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, model, output, server};

fn to_msg(e: ItemError) -> error::PeoplesRepublicOfListsError {
    match e {
        ItemError::NotFound => not_found(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg),
    }
}

pub async fn create_team(
    input: input::CreateTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::CreateTeamOutput, error::CreateTeamError> {
    let team_id = team_service::create_team(&teams, &input.name, &input.user_id)
        .await
        .map_err(|e| error::CreateTeamError::from(to_msg(e)))?;
    Ok(output::CreateTeamOutput { team_id })
}

pub async fn get_team(
    input: input::GetTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::GetTeamOutput, error::GetTeamError> {
    let team = team_service::get_team(&teams, &input.team_id)
        .await
        .map_err(|e| error::GetTeamError::from(to_msg(e)))?;
    Ok(output::GetTeamOutput {
        team_id: team.id,
        name: team.name,
    })
}

pub async fn list_teams(
    input: input::ListTeamsInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::ListTeamsOutput, error::ListTeamsError> {
    let memberships = team_service::list_teams(&teams, &input.user_id)
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
) -> Result<output::ListTeamMembersOutput, error::ListTeamMembersError> {
    let members = team_service::list_team_members(&teams, &input.team_id, &input.user_id)
        .await
        .map_err(|e| error::ListTeamMembersError::from(to_msg(e)))?
        .into_iter()
        .map(|m| model::TeamMemberSummary {
            user_id: m.user.id,
            first_name: m.user.first_name,
            last_name: m.user.last_name,
            status: m.status,
        })
        .collect();
    Ok(output::ListTeamMembersOutput { members })
}

pub async fn invite_team_member(
    input: input::InviteTeamMemberInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(users): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::InviteTeamMemberOutput, error::InviteTeamMemberError> {
    team_service::invite_team_member(
        &teams,
        &users,
        &input.team_id,
        &input.user_id,
        &input.invitee_user_id,
    )
    .await
    .map_err(|e| error::InviteTeamMemberError::from(to_msg(e)))?;
    Ok(output::InviteTeamMemberOutput {})
}

pub async fn accept_team_invite(
    input: input::AcceptTeamInviteInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::AcceptTeamInviteOutput, error::AcceptTeamInviteError> {
    team_service::accept_team_invite(&teams, &input.team_id, &input.user_id)
        .await
        .map_err(|e| error::AcceptTeamInviteError::from(to_msg(e)))?;
    Ok(output::AcceptTeamInviteOutput {})
}

pub async fn leave_team(
    input: input::LeaveTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::LeaveTeamOutput, error::LeaveTeamError> {
    team_service::leave_team(&teams, &input.team_id, &input.user_id)
        .await
        .map_err(|e| error::LeaveTeamError::from(to_msg(e)))?;
    Ok(output::LeaveTeamOutput {})
}
