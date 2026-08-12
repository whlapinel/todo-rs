use crate::domain::project::Project;
use crate::domain::team::TeamRole;
use crate::service::items::ItemError;
use crate::storage::sqlite::{ProjectMemberInfo, ProjectRepo, RepoError, TeamRepo};
use std::sync::Arc;

/// Checks that `user_id` can access `project_id` — a personal project (`team_id ==
/// None`) grants access to its owner only; a shared project (`team_id == Some`)
/// grants access to any `ACTIVE` member of that team. Mirrors
/// `service::teams::require_team_admin`'s shape, per the access-check formula in
/// docs/project-abstraction-plan.md. `team_id` is never actually `Some` yet as of
/// stage A3 (`create_project` doesn't set it, and there's no attach flow until A4),
/// but the check is written generally so A4 doesn't have to revisit it.
pub async fn require_project_member(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    user_id: &str,
) -> Result<(), ItemError> {
    let project = projects.get(project_id).await?;
    match &project.team_id {
        Some(team_id) => {
            let status = teams
                .member_status(team_id, user_id)
                .await
                .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
            if status.as_deref() != Some("ACTIVE") {
                return Err(ItemError::Invalid(
                    "you are not an active member of this project's team".to_string(),
                ));
            }
        }
        None => {
            if project.owner_user_id != user_id {
                return Err(ItemError::Invalid(
                    "you are not a member of this project".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Same shape one step further — requires `user_id` to hold `admin` on
/// `project_id`'s own `project_members` row (seeded for the owner at `create`;
/// synced in from an attached team's members starting stage A4). Checks membership
/// first so a non-member gets "not a member" rather than a confusing "not an admin".
pub async fn require_project_admin(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    user_id: &str,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, project_id, user_id).await?;
    let role = projects
        .member_role(project_id, user_id)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
    if role != Some(TeamRole::Admin) {
        return Err(ItemError::Invalid(
            "only a project admin can do this".to_string(),
        ));
    }
    Ok(())
}

/// Creates a personal project (no attached team — that's stage A4's
/// `attach_team_to_project`, not part of `create_project` itself).
pub async fn create_project(
    projects: &Arc<dyn ProjectRepo>,
    name: &str,
    owner_user_id: &str,
) -> Result<String, ItemError> {
    Ok(projects.create(name, owner_user_id, None).await?)
}

/// Every project `user_id` belongs to — owned or (once A4 lands) team-synced.
pub async fn list_projects(
    projects: &Arc<dyn ProjectRepo>,
    user_id: &str,
) -> Result<Vec<Project>, ItemError> {
    Ok(projects.list_for_user(user_id).await?)
}

pub async fn list_project_members(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
) -> Result<Vec<ProjectMemberInfo>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(projects.list_members(project_id).await?)
}

/// Promotes/demotes `target_user_id`'s role on `project_id`. Requires the requester
/// to already be a project admin. Unlike `service::teams::set_team_member_role`,
/// there's no last-remaining-admin guard here — `ProjectRepo` has no
/// `count_active_admins` equivalent (out of A2's scope), and adding one isn't part
/// of this stage; revisit if that gap turns out to matter once this is reachable
/// via HTTP (stage A5).
pub async fn set_project_member_role(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    target_user_id: &str,
    new_role: TeamRole,
) -> Result<(), ItemError> {
    require_project_admin(projects, teams, project_id, requester_user_id).await?;
    projects
        .set_member_role(project_id, target_user_id, new_role)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => {
                ItemError::Invalid("user is not a member of this project".to_string())
            }
            _ => ItemError::Internal(format!("{e:?}")),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{MockProjectRepo, MockTeamRepo};

    fn personal_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Personal".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: None,
        }
    }

    fn shared_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Shared".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: Some("team1".to_string()),
        }
    }

    #[tokio::test]
    async fn require_project_member_allows_owner_on_personal_project() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        require_project_member(&projects, &teams, "p1", "owner1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn require_project_member_rejects_non_owner_on_personal_project() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let err = require_project_member(&projects, &teams, "p1", "someone_else")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn require_project_member_allows_active_team_member_on_shared_project() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(shared_project()));

        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);
        require_project_member(&projects, &teams, "p1", "member1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn require_project_member_rejects_inactive_team_member_on_shared_project() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(shared_project()));

        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("PENDING".to_string())));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);
        let err = require_project_member(&projects, &teams, "p1", "member1")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn require_project_admin_rejects_non_admin() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let err = require_project_admin(&projects, &teams, "p1", "owner1")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn require_project_admin_allows_admin() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        require_project_admin(&projects, &teams, "p1", "owner1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn set_project_member_role_rejects_non_admin_requester() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let err = set_project_member_role(&projects, &teams, "p1", "owner1", "member1", TeamRole::Admin)
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn set_project_member_role_allows_admin_requester() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        mock.expect_set_member_role().returning(|_, _, _| Ok(()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        set_project_member_role(&projects, &teams, "p1", "owner1", "member1", TeamRole::Member)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_project_never_sets_a_team() {
        let mut mock = MockProjectRepo::new();
        mock.expect_create()
            .withf(|_, _, team_id: &Option<&str>| team_id.is_none())
            .returning(|_, _, _| Ok("p1".to_string()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let id = create_project(&projects, "My Project", "owner1").await.unwrap();
        assert_eq!(id, "p1");
    }

    #[tokio::test]
    async fn list_projects_delegates_to_repo() {
        let mut mock = MockProjectRepo::new();
        mock.expect_list_for_user()
            .returning(|_| Ok(vec![personal_project()]));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let result = list_projects(&projects, "owner1").await.unwrap();
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn list_project_members_rejects_non_member() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let result = list_project_members(&projects, &teams, "p1", "someone_else").await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }
}
