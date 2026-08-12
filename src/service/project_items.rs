use crate::domain::item::Item;
use crate::service::error::ItemError;
use crate::service::projects::require_project_member;
use crate::storage::sqlite::{ItemRepo, ProjectRepo, TeamRepo};
use std::sync::Arc;

/// Stage B3's unified read path — replaces the personal-vs-team authorization
/// branch (`repo.get`/`repo.get_team_item`, gated by two different checks) with a
/// single membership check against the item's owning project, per
/// docs/project-abstraction-plan.md's access-check formula. Not yet reachable via
/// HTTP (that's stage B4's `ProjectItem` Smithy resource) — unit-tested only, same
/// precedent stages A2/A3 set.
pub async fn get_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    item_id: &str,
) -> Result<Item, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(repo.get_by_project(project_id, item_id).await?)
}

/// Stage B3's unified list path — same shape as `get_project_item` above, wrapping
/// `ItemRepo::list_by_project`.
pub async fn list_project_items(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    parent_item_id: Option<String>,
) -> Result<Vec<Item>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(repo.list_by_project(project_id, parent_item_id).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::project::Project;
    use crate::storage::sqlite::{MockItemRepo, MockProjectRepo, MockTeamRepo};

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
    async fn get_project_item_allows_owner_on_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "i1")
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Task")));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_project_item(&repo, &projects, &teams, "p1", "owner1", "i1")
            .await
            .unwrap();
        assert_eq!(item.name, "Task");
    }

    #[tokio::test]
    async fn get_project_item_rejects_non_owner_on_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let items_mock = MockItemRepo::new();

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_project_item(&repo, &projects, &teams, "p1", "not-owner", "i1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_project_item_allows_active_team_member_on_shared_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_team_item("team1", "Task")));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let item = get_project_item(&repo, &projects, &teams, "p1", "member1", "i1")
            .await
            .unwrap();
        assert_eq!(item.name, "Task");
    }

    #[tokio::test]
    async fn get_project_item_rejects_inactive_team_member_on_shared_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("PENDING".to_string())));
        let items_mock = MockItemRepo::new();

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let result = get_project_item(&repo, &projects, &teams, "p1", "pending1", "i1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_project_items_delegates_to_repo_after_membership_check() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_list_by_project()
            .withf(|project_id: &str, parent_item_id: &Option<String>| {
                project_id == "p1" && parent_item_id.is_none()
            })
            .returning(|_, _| {
                Ok(vec![
                    Item::new_user_item("owner1", "One"),
                    Item::new_user_item("owner1", "Two"),
                ])
            });

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let items = list_project_items(&repo, &projects, &teams, "p1", "owner1", None)
            .await
            .unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn list_project_items_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let items_mock = MockItemRepo::new();

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = list_project_items(&repo, &projects, &teams, "p1", "not-owner", None).await;
        assert!(result.is_err());
    }
}
