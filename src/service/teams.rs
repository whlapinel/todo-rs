use crate::domain::team::Team;
use crate::service::items::ItemError;
use crate::storage::sqlite::{RepoError, TeamMemberInfo, TeamRepo, TeamWithStatus, UserRepo};
use std::sync::Arc;

/// Moved from `json_api::teams::create_team`.
pub async fn create_team(teams: &Arc<dyn TeamRepo>, name: &str, user_id: &str) -> Result<String, ItemError> {
    Ok(teams.create(name, user_id).await?)
}

/// Moved from `json_api::teams::get_team`.
pub async fn get_team(teams: &Arc<dyn TeamRepo>, team_id: &str) -> Result<Team, ItemError> {
    Ok(teams.get(team_id).await?)
}

/// Moved from `json_api::teams::list_teams`.
pub async fn list_teams(teams: &Arc<dyn TeamRepo>, user_id: &str) -> Result<Vec<TeamWithStatus>, ItemError> {
    Ok(teams.list_for_user(user_id).await?)
}

/// Moved from `json_api::teams::list_team_members`.
pub async fn list_team_members(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<Vec<TeamMemberInfo>, ItemError> {
    teams
        .member_status(team_id, requester_user_id)
        .await?
        .ok_or_else(|| ItemError::Invalid("not a member of this team".to_string()))?;
    Ok(teams.list_members(team_id).await?)
}

/// Moved from `json_api::teams::invite_team_member`.
pub async fn invite_team_member(
    teams: &Arc<dyn TeamRepo>,
    users: &Arc<dyn UserRepo>,
    team_id: &str,
    inviter_user_id: &str,
    invitee_user_id: &str,
) -> Result<(), ItemError> {
    let inviter_status = teams.member_status(team_id, inviter_user_id).await?;
    if inviter_status.as_deref() != Some("ACTIVE") {
        return Err(ItemError::Invalid(
            "you are not an active member of this team".to_string(),
        ));
    }
    users.get(invitee_user_id).await.map_err(|e| match e {
        RepoError::NotFound => ItemError::Invalid("invitee does not exist".to_string()),
        _ => ItemError::Internal(format!("{e:?}")),
    })?;
    let existing = teams.member_status(team_id, invitee_user_id).await?;
    if existing.is_some() {
        return Err(ItemError::Invalid(
            "user is already a member or has a pending invite".to_string(),
        ));
    }
    teams.invite(team_id, invitee_user_id, inviter_user_id).await?;
    Ok(())
}

/// Moved from `json_api::teams::accept_team_invite`.
pub async fn accept_team_invite(teams: &Arc<dyn TeamRepo>, team_id: &str, user_id: &str) -> Result<(), ItemError> {
    teams.accept(team_id, user_id).await.map_err(|e| match e {
        RepoError::NotFound => ItemError::Invalid("no pending invite found".to_string()),
        _ => ItemError::Internal(format!("{e:?}")),
    })
}

/// Moved from `json_api::teams::leave_team`.
pub async fn leave_team(teams: &Arc<dyn TeamRepo>, team_id: &str, user_id: &str) -> Result<(), ItemError> {
    teams.remove_member(team_id, user_id).await.map_err(|e| match e {
        RepoError::NotFound => ItemError::Invalid("not a member of this team".to_string()),
        _ => ItemError::Internal(format!("{e:?}")),
    })
}
