use super::{internal, not_found};
use crate::storage::{RepoError, TeamRepo, UserRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, model, output, server};

pub async fn create_team(
    input: input::CreateTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::CreateTeamOutput, error::CreateTeamError> {
    let team_id = teams
        .create(&input.name, &input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateTeamOutput { team_id })
}

pub async fn get_team(
    input: input::GetTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::GetTeamOutput, error::GetTeamError> {
    let team = teams.get(&input.team_id).await.map_err(|e| match e {
        RepoError::NotFound => error::GetTeamError::from(not_found()),
        _ => error::GetTeamError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::GetTeamOutput {
        team_id: team.id,
        name: team.name,
    })
}

pub async fn list_teams(
    input: input::ListTeamsInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::ListTeamsOutput, error::ListTeamsError> {
    let memberships = teams
        .list_for_user(&input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
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
    teams
        .member_status(&input.team_id, &input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?
        .ok_or_else(|| error::ListTeamMembersError::from(internal("not a member of this team")))?;

    let members = teams
        .list_members(&input.team_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?
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
    let inviter_status = teams
        .member_status(&input.team_id, &input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    if inviter_status.as_deref() != Some("ACTIVE") {
        return Err(error::InviteTeamMemberError::from(internal(
            "you are not an active member of this team",
        )));
    }

    users.get(&input.invitee_user_id).await.map_err(|e| match e {
        RepoError::NotFound => internal("invitee does not exist"),
        _ => internal(format!("{e:?}")),
    })?;

    let existing = teams
        .member_status(&input.team_id, &input.invitee_user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    if existing.is_some() {
        return Err(error::InviteTeamMemberError::from(internal(
            "user is already a member or has a pending invite",
        )));
    }

    teams
        .invite(&input.team_id, &input.invitee_user_id, &input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::InviteTeamMemberOutput {})
}

pub async fn accept_team_invite(
    input: input::AcceptTeamInviteInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::AcceptTeamInviteOutput, error::AcceptTeamInviteError> {
    teams
        .accept(&input.team_id, &input.user_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => internal("no pending invite found"),
            _ => internal(format!("{e:?}")),
        })?;
    Ok(output::AcceptTeamInviteOutput {})
}

pub async fn leave_team(
    input: input::LeaveTeamInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
) -> Result<output::LeaveTeamOutput, error::LeaveTeamError> {
    teams
        .remove_member(&input.team_id, &input.user_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => internal("not a member of this team"),
            _ => internal(format!("{e:?}")),
        })?;
    Ok(output::LeaveTeamOutput {})
}
