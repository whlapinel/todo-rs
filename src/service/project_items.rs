use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::items::{self, CreateItemParams, UpdateItemParams};
use crate::service::projects::require_project_member;
use crate::service::team_items::{
    self, CreateTeamItemParams, UpdateTeamItemContext, UpdateTeamItemParams,
};
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, ProjectRepo, TeamRepo};
use chrono::{DateTime, Utc};
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

#[derive(Debug, Default)]
pub struct CreateProjectItemParams {
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: Option<bool>,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemKind>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub points: Option<i32>,
}

/// Stage B4's unified create path. Rather than reimplementing the recurrence/
/// offset/event-trigger/points machinery a third time, this resolves `project_id`
/// down to a plain `user_id` (personal project) or `team_id` (team-backed project)
/// and delegates straight to `items::create_item`/`team_items::create_team_item` —
/// the same functions the legacy `Item`/`TeamItem` operations already call, so
/// project-created items dual-write `user_id`/`team_id` exactly like those do,
/// keeping the legacy read APIs consistent (see docs/project-abstraction-plan.md's
/// stage B4 dual-write-bridge verification). `assigned_to_user_id`/`points` are
/// simply dropped for a personal project — `CreateItemParams` has no slot for
/// either, matching personal items never having carried a `TeamAssignment` at all.
pub async fn create_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    params: CreateProjectItemParams,
) -> Result<String, ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    let project = projects.get(&params.project_id).await?;
    match project.team_id {
        Some(team_id) => {
            team_items::create_team_item(
                repo,
                teams,
                projects,
                requester_user_id,
                CreateTeamItemParams {
                    team_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    recurrence: params.recurrence,
                    recurrence_basis: params.recurrence_basis,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    assigned_to_user_id: params.assigned_to_user_id,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                    points: params.points,
                },
            )
            .await
        }
        None => {
            items::create_item(
                repo,
                projects,
                CreateItemParams {
                    user_id: project.owner_user_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    recurrence: params.recurrence,
                    recurrence_basis: params.recurrence_basis,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                },
            )
            .await
        }
    }
}

#[derive(Debug, Default)]
pub struct UpdateProjectItemParams {
    pub project_id: String,
    pub item_id: String,
    pub name: String,
    pub description: Option<String>,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemKind>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub points: Option<i32>,
}

/// Stage B4's unified update path — same delegation shape as `create_project_item`.
/// `activity_log` is only ever touched on the team-backed branch (points award/
/// reversal, see `team_items::update_team_item`); the personal branch's
/// `items::update_item` has no use for it at all.
pub async fn update_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    requester_user_id: &str,
    params: UpdateProjectItemParams,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    let project = projects.get(&params.project_id).await?;
    match project.team_id {
        Some(team_id) => {
            team_items::update_team_item(
                repo,
                &UpdateTeamItemContext {
                    teams: teams.clone(),
                    projects: projects.clone(),
                    activity_log: activity_log.clone(),
                },
                requester_user_id,
                UpdateTeamItemParams {
                    team_id,
                    item_id: params.item_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    recurrence: params.recurrence,
                    recurrence_basis: params.recurrence_basis,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    assigned_to_user_id: params.assigned_to_user_id,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                    points: params.points,
                },
            )
            .await
        }
        None => {
            items::update_item(
                repo,
                UpdateItemParams {
                    user_id: project.owner_user_id,
                    item_id: params.item_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    recurrence: params.recurrence,
                    recurrence_basis: params.recurrence_basis,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                },
            )
            .await
        }
    }
}

/// Stage B4's unified delete path — same delegation shape as `create_project_item`.
pub async fn delete_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    project_id: &str,
    item_id: &str,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let project = projects.get(project_id).await?;
    match project.team_id {
        Some(team_id) => {
            team_items::delete_team_item(repo, teams, requester_user_id, &team_id, item_id).await
        }
        None => items::delete_item(repo, &project.owner_user_id, item_id).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::project::Project;
    use crate::domain::team::TeamRole;
    use crate::storage::sqlite::{MockActivityLogRepo, MockItemRepo, MockProjectRepo, MockTeamRepo};

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

    #[tokio::test]
    async fn create_project_item_delegates_to_personal_item_creation() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        projects_mock.expect_find_personal_project().returning(|_| Ok(None));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.user_id.as_deref() == Some("owner1") && item.team_id.is_none())
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item_id = create_project_item(
            &repo,
            &projects,
            &teams,
            "owner1",
            CreateProjectItemParams {
                project_id: "p1".to_string(),
                name: "Buy milk".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create personal project item");

        assert_eq!(item_id, "new-item-id");
    }

    #[tokio::test]
    async fn create_project_item_delegates_to_team_item_creation() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        projects_mock.expect_get_by_team().returning(|_| Ok(None));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        teams_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.team_id.as_deref() == Some("team1") && item.user_id.is_none())
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let item_id = create_project_item(
            &repo,
            &projects,
            &teams,
            "member1",
            CreateProjectItemParams {
                project_id: "p1".to_string(),
                name: "Mow the lawn".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create team project item");

        assert_eq!(item_id, "new-item-id");
    }

    #[tokio::test]
    async fn create_project_item_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = create_project_item(
            &repo,
            &projects,
            &teams,
            "not-owner",
            CreateProjectItemParams {
                project_id: "p1".to_string(),
                name: "Sneaky".to_string(),
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_project_item_delegates_to_personal_item_update() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Old name")));
        items_mock.expect_update().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            "owner1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "New name".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("should update personal project item");
    }

    #[tokio::test]
    async fn update_project_item_delegates_to_team_item_update() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        teams_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_team_item()
            .returning(|_, _| Ok(Item::new_team_item("team1", "Old name")));
        items_mock
            .expect_update_team_item()
            .times(1)
            .returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            "member1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "New name".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("should update team project item");
    }

    #[tokio::test]
    async fn delete_project_item_delegates_to_personal_item_delete() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Task")));
        items_mock.expect_list_children().returning(|_| Ok(vec![]));
        items_mock.expect_list_by_source_event().returning(|_| Ok(vec![]));
        items_mock.expect_delete().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        delete_project_item(&repo, &projects, &teams, "owner1", "p1", "i1")
            .await
            .expect("should delete personal project item");
    }

    #[tokio::test]
    async fn delete_project_item_delegates_to_team_item_delete() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut teams_mock = MockTeamRepo::new();
        teams_mock
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_team_item()
            .returning(|_, _| Ok(Item::new_team_item("team1", "Task")));
        items_mock.expect_list_children().returning(|_| Ok(vec![]));
        items_mock.expect_list_by_source_event().returning(|_| Ok(vec![]));
        items_mock.expect_delete().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        delete_project_item(&repo, &projects, &teams, "member1", "p1", "i1")
            .await
            .expect("should delete team project item");
    }
}
