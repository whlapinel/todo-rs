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

/// Non-erroring variant of `require_project_admin`, for gating optional UI (mirrors
/// `service::teams::is_team_admin`) — added in stage C1
/// (docs/project-abstraction-plan.md), since points/assignment authority moved off
/// `team_members.role` onto `project_members.role`, and the web UI's own
/// show-the-points-field gate needs to match whatever the server will actually
/// honor.
pub async fn is_project_admin(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    user_id: &str,
) -> bool {
    require_project_admin(projects, teams, project_id, user_id)
        .await
        .is_ok()
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

/// Idempotent: creates a default personal "Personal" project for `user_id` unless
/// they already belong to at least one. Called from every `UserRepo::get_or_create_*`
/// call site (see `auth.rs`) rather than gated on "was this user just created",
/// since `get_or_create_*` itself doesn't report that — checking "has zero projects"
/// instead means this also self-heals any user who signed up before Project existed,
/// without waiting on stage B's dedicated backfill migration.
pub async fn ensure_default_project(
    projects: &Arc<dyn ProjectRepo>,
    user_id: &str,
) -> Result<(), ItemError> {
    if projects.list_for_user(user_id).await?.is_empty() {
        create_project(projects, "Personal", user_id).await?;
    }
    Ok(())
}

/// Idempotent: creates and attaches a backing project for `team_id`, named after the
/// team and owned by `creator_user_id`, unless one already exists. Closes the gap left
/// by `ensure_default_project` only ever covering *personal* projects — without this,
/// every team created after stage B1's backfill migration ran would have zero attached
/// projects until someone manually created and attached one via the Project API, and
/// stage B2's team-item `project_id` resolution would silently find nothing for it.
/// Uses `create`-then-`attach_team` (stage A4's shape) rather than passing `team_id`
/// directly to `create`, so `attach_team`'s own member-seed cascade runs — though in
/// practice a brand-new team's only `ACTIVE` member is its own creator, already seeded
/// as the project's owner/admin by `create` itself.
pub async fn ensure_team_project(
    projects: &Arc<dyn ProjectRepo>,
    team_id: &str,
    team_name: &str,
    creator_user_id: &str,
) -> Result<(), ItemError> {
    if projects.get_by_team(team_id).await?.is_some() {
        return Ok(());
    }
    let project_id = projects.create(team_name, creator_user_id, None).await?;
    projects.attach_team(&project_id, team_id).await?;
    Ok(())
}

/// Fetches a single project. Requires the requester to be a member (owner, for a
/// personal project; an active team member, for a shared one) — same gating as
/// `list_project_members`.
pub async fn get_project(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
) -> Result<Project, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(projects.get(project_id).await?)
}

/// Renames a project. Requires the requester to already be a project admin, mirroring
/// `service::teams::update_team`'s own gating.
pub async fn update_project(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    name: &str,
) -> Result<(), ItemError> {
    require_project_admin(projects, teams, project_id, requester_user_id).await?;
    Ok(projects.update_name(project_id, name).await?)
}

/// Deletes a project and its whole `project_members` table (cascade lives in
/// `SqliteProjectRepo::delete`, stage A2). Requires the requester to already be a
/// project admin.
pub async fn delete_project(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
) -> Result<(), ItemError> {
    require_project_admin(projects, teams, project_id, requester_user_id).await?;
    Ok(projects.delete(project_id).await?)
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

/// Attaches `team_id` to `project_id`, seeding `project_members` from the team's
/// current ACTIVE members (`ProjectRepo::attach_team` does the actual seed insert —
/// see docs/project-abstraction-plan.md stage A4). Requires the requester to
/// already be a project admin; a personal project's only admin is its owner (seeded
/// at `create`), so in practice this means "the owner attaches a team."
pub async fn attach_team_to_project(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    team_id: &str,
) -> Result<(), ItemError> {
    require_project_admin(projects, teams, project_id, requester_user_id).await?;
    Ok(projects.attach_team(project_id, team_id).await?)
}

/// Detaches whatever team is attached to `project_id`, clearing every
/// team-synced `project_members` row (`ProjectRepo::detach_team` does the actual
/// clear — see docs/project-abstraction-plan.md stage A4). Same admin gating as
/// `attach_team_to_project`.
pub async fn detach_team_from_project(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
) -> Result<(), ItemError> {
    require_project_admin(projects, teams, project_id, requester_user_id).await?;
    Ok(projects.detach_team(project_id).await?)
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
    async fn ensure_default_project_creates_one_when_none_exist() {
        let mut mock = MockProjectRepo::new();
        mock.expect_list_for_user().returning(|_| Ok(vec![]));
        mock.expect_create()
            .withf(|name, owner_user_id, team_id: &Option<&str>| {
                name == "Personal" && owner_user_id == "u1" && team_id.is_none()
            })
            .returning(|_, _, _| Ok("p1".to_string()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        ensure_default_project(&projects, "u1").await.unwrap();
    }

    #[tokio::test]
    async fn ensure_default_project_is_a_noop_when_one_already_exists() {
        let mut mock = MockProjectRepo::new();
        mock.expect_list_for_user()
            .returning(|_| Ok(vec![personal_project()]));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        ensure_default_project(&projects, "u1").await.unwrap();
    }

    #[tokio::test]
    async fn ensure_team_project_creates_and_attaches_when_none_exists() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get_by_team()
            .withf(|team_id| team_id == "t1")
            .returning(|_| Ok(None));
        mock.expect_create()
            .withf(|name, owner_user_id, team_id: &Option<&str>| {
                name == "Family" && owner_user_id == "u1" && team_id.is_none()
            })
            .returning(|_, _, _| Ok("p1".to_string()));
        mock.expect_attach_team()
            .withf(|project_id, team_id| project_id == "p1" && team_id == "t1")
            .returning(|_, _| Ok(()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        ensure_team_project(&projects, "t1", "Family", "u1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ensure_team_project_is_a_noop_when_one_already_exists() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get_by_team()
            .returning(|_| Ok(Some(shared_project())));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        ensure_team_project(&projects, "team1", "Family", "u1")
            .await
            .unwrap();
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
    async fn attach_team_to_project_rejects_non_admin_requester() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let err = attach_team_to_project(&projects, &teams, "p1", "owner1", "team1")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn attach_team_to_project_allows_admin_requester() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        mock.expect_attach_team()
            .withf(|project_id, team_id| project_id == "p1" && team_id == "team1")
            .returning(|_, _| Ok(()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        attach_team_to_project(&projects, &teams, "p1", "owner1", "team1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn detach_team_from_project_rejects_non_admin_requester() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(shared_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);
        let err = detach_team_from_project(&projects, &teams, "p1", "member1")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn detach_team_from_project_allows_admin_requester() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(shared_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        mock.expect_detach_team()
            .withf(|project_id| project_id == "p1")
            .returning(|_| Ok(()));

        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);
        detach_team_from_project(&projects, &teams, "p1", "owner1")
            .await
            .unwrap();
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

    #[tokio::test]
    async fn get_project_rejects_non_member() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let err = get_project(&projects, &teams, "p1", "someone_else")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn get_project_allows_owner() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let project = get_project(&projects, &teams, "p1", "owner1").await.unwrap();
        assert_eq!(project.id, "p1");
    }

    #[tokio::test]
    async fn update_project_rejects_non_admin() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let err = update_project(&projects, &teams, "p1", "owner1", "New name")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_project_allows_admin() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        mock.expect_update_name()
            .withf(|project_id, name| project_id == "p1" && name == "New name")
            .returning(|_, _| Ok(()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        update_project(&projects, &teams, "p1", "owner1", "New name")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_project_rejects_non_admin() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let err = delete_project(&projects, &teams, "p1", "owner1")
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn delete_project_allows_admin() {
        let mut mock = MockProjectRepo::new();
        mock.expect_get().returning(|_| Ok(personal_project()));
        mock.expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        mock.expect_delete()
            .withf(|project_id| project_id == "p1")
            .returning(|_| Ok(()));

        let projects: Arc<dyn ProjectRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        delete_project(&projects, &teams, "p1", "owner1").await.unwrap();
    }
}
