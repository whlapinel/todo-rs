use crate::domain::team::{Team, TeamRole};
use crate::service::items::ItemError;
use crate::service::projects::ensure_team_project;
use crate::storage::sqlite::{
    ProjectRepo, RepoError, TeamMemberInfo, TeamRepo, TeamWithStatus, UserRepo,
};
use std::sync::Arc;

/// Checks that `user_id` is an `ACTIVE` member of `team_id` **and** holds `admin`
/// there — mirrors `team_items::require_active_member`'s shape, one step further.
/// Roles are per-team (see CLAUDE.md), so this never consults any global concept.
pub async fn require_team_admin(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<(), ItemError> {
    let status = teams
        .member_status(team_id, user_id)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
    if status.as_deref() != Some("ACTIVE") {
        return Err(ItemError::Invalid(
            "you are not an active member of this team".to_string(),
        ));
    }
    let role = teams
        .member_role(team_id, user_id)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
    if role != Some(TeamRole::Admin) {
        return Err(ItemError::Invalid(
            "only a team admin can do this".to_string(),
        ));
    }
    Ok(())
}

/// Promotes/demotes `target_user_id`'s role on `team_id`. Requires the requester to
/// already be an admin of that team, and refuses to demote a team's last remaining
/// admin (there would be no one left who could ever promote another admin again).
pub async fn set_team_member_role(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
    target_user_id: &str,
    new_role: TeamRole,
) -> Result<(), ItemError> {
    require_team_admin(teams, team_id, requester_user_id).await?;

    if new_role != TeamRole::Admin {
        let target_role = teams
            .member_role(team_id, target_user_id)
            .await
            .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
        if target_role == Some(TeamRole::Admin) {
            let admin_count = teams
                .count_active_admins(team_id)
                .await
                .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
            if admin_count <= 1 {
                return Err(ItemError::Invalid(
                    "cannot demote the last remaining admin of this team".to_string(),
                ));
            }
        }
    }

    teams
        .set_member_role(team_id, target_user_id, new_role)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => {
                ItemError::Invalid("user is not a member of this team".to_string())
            }
            _ => ItemError::Internal(format!("{e:?}")),
        })
}

/// Moved from `json_api::teams::create_team`. Also ensures the new team has a backing
/// project (stage B2's `ensure_team_project`) — see docs/project-abstraction-plan.md.
pub async fn create_team(
    teams: &Arc<dyn TeamRepo>,
    projects: &Arc<dyn ProjectRepo>,
    name: &str,
    user_id: &str,
) -> Result<String, ItemError> {
    let team_id = teams.create(name, user_id).await?;
    ensure_team_project(projects, &team_id, name, user_id).await?;
    Ok(team_id)
}

/// Moved from `json_api::teams::get_team`.
pub async fn get_team(teams: &Arc<dyn TeamRepo>, team_id: &str) -> Result<Team, ItemError> {
    Ok(teams.get(team_id).await?)
}

/// Renames a team. Requires the requester to already be an admin of that team,
/// mirroring `set_team_member_role`'s own gating.
pub async fn update_team(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
    name: &str,
) -> Result<(), ItemError> {
    require_team_admin(teams, team_id, requester_user_id).await?;
    Ok(teams.update_name(team_id, name).await?)
}

/// Moved from `json_api::teams::list_teams`.
pub async fn list_teams(
    teams: &Arc<dyn TeamRepo>,
    user_id: &str,
) -> Result<Vec<TeamWithStatus>, ItemError> {
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

/// `user_id`'s current point balance on `team_id` — the Stage 7 points badge's data
/// source. Goes through `list_team_members` (so the requester's own active-membership
/// check still applies) rather than a dedicated repo round trip; defaults to 0 if
/// `user_id` is somehow absent from the resulting list, since none of this stage's
/// callers have a more specific way to react to that than "show zero."
pub async fn member_points(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<i32, ItemError> {
    let members = list_team_members(teams, team_id, user_id).await?;
    Ok(members
        .iter()
        .find(|m| m.user.id == user_id)
        .map(|m| m.points)
        .unwrap_or(0))
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
    teams
        .invite(team_id, invitee_user_id, inviter_user_id)
        .await?;
    Ok(())
}

/// Moved from `json_api::teams::accept_team_invite`.
pub async fn accept_team_invite(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<(), ItemError> {
    teams.accept(team_id, user_id).await.map_err(|e| match e {
        RepoError::NotFound => ItemError::Invalid("no pending invite found".to_string()),
        _ => ItemError::Internal(format!("{e:?}")),
    })
}

/// Moved from `json_api::teams::leave_team`.
pub async fn leave_team(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<(), ItemError> {
    teams
        .remove_member(team_id, user_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => ItemError::Invalid("not a member of this team".to_string()),
            _ => ItemError::Internal(format!("{e:?}")),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::MockTeamRepo;

    #[tokio::test]
    async fn require_team_admin_rejects_non_member() {
        let mut mock = MockTeamRepo::new();
        mock.expect_member_status().returning(|_, _| Ok(None));

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        let err = require_team_admin(&teams, "t1", "u1").await.unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn require_team_admin_rejects_active_non_admin() {
        let mut mock = MockTeamRepo::new();
        mock.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        let err = require_team_admin(&teams, "t1", "u1").await.unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn require_team_admin_allows_active_admin() {
        let mut mock = MockTeamRepo::new();
        mock.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        require_team_admin(&teams, "t1", "u1").await.unwrap();
    }

    #[tokio::test]
    async fn set_team_member_role_rejects_demoting_last_admin() {
        let mut mock = MockTeamRepo::new();
        // Requester's own require_team_admin check.
        mock.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        mock.expect_member_role().returning(|_, target_user_id| {
            // Both the requester's own admin check and the target's role lookup hit
            // this same mock method — both are "admin" here, since the requester is
            // demoting themself as the team's only admin.
            let _ = target_user_id;
            Ok(Some(TeamRole::Admin))
        });
        mock.expect_count_active_admins().returning(|_| Ok(1));

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        let err = set_team_member_role(&teams, "t1", "admin1", "admin1", TeamRole::Member)
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn set_team_member_role_allows_demoting_one_of_several_admins() {
        let mut mock = MockTeamRepo::new();
        mock.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        mock.expect_count_active_admins().returning(|_| Ok(2));
        mock.expect_set_member_role().returning(|_, _, _| Ok(()));

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        set_team_member_role(&teams, "t1", "admin1", "admin2", TeamRole::Member)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn member_points_finds_requesters_own_balance() {
        use crate::domain::user::User;
        use crate::storage::sqlite::TeamMemberInfo;

        let mut mock = MockTeamRepo::new();
        mock.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        mock.expect_list_members().returning(|_| {
            Ok(vec![TeamMemberInfo {
                user: User {
                    id: "u1".to_string(),
                    first_name: "A".to_string(),
                    last_name: "B".to_string(),
                    email: Some("a@b.com".to_string()),
                    google_id: None,
                    timezone: None,
                },
                status: "ACTIVE".to_string(),
                role: TeamRole::Member,
                points: 42,
            }])
        });

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        let points = member_points(&teams, "t1", "u1").await.unwrap();
        assert_eq!(points, 42);
    }

    #[tokio::test]
    async fn member_points_defaults_to_zero_when_absent() {
        let mut mock = MockTeamRepo::new();
        mock.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        mock.expect_list_members().returning(|_| Ok(vec![]));

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        let points = member_points(&teams, "t1", "u1").await.unwrap();
        assert_eq!(points, 0);
    }

    #[tokio::test]
    async fn set_team_member_role_rejects_requester_who_is_not_admin() {
        let mut mock = MockTeamRepo::new();
        mock.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let teams: Arc<dyn TeamRepo> = Arc::new(mock);
        let err = set_team_member_role(&teams, "t1", "u1", "u2", TeamRole::Admin)
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }
}
