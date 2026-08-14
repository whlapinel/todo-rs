use super::internal;
use crate::storage::sqlite::{ItemRepo, ProjectRepo};
use std::collections::HashMap;
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

pub async fn list_items_due(
    input: input::ListItemsDueInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
) -> Result<output::ListItemsDueOutput, error::ListItemsDueError> {
    let after = input.deadline_after.map(|t| t.secs());
    let before = input.deadline_before.map(|t| t.secs());
    let due_items = repo
        .list_due(&input.user_id, after, before)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;

    let mut team_id_by_project: HashMap<String, Option<String>> = HashMap::new();
    for project_id in due_items.iter().filter_map(|di| di.item.project_id.clone()) {
        if let std::collections::hash_map::Entry::Vacant(entry) = team_id_by_project.entry(project_id.clone())
        {
            let project = projects
                .get(&project_id)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            entry.insert(project.team_id);
        }
    }

    let items = due_items
        .into_iter()
        .map(|di| {
            let team_id = di
                .item
                .project_id
                .as_ref()
                .and_then(|pid| team_id_by_project.get(pid).cloned().flatten());
            todo_server_sdk::model::DueItemSummary {
                item_id: di.item.id.clone(),
                name: di.item.name.clone(),
                owner_user_id: di.item.user_id.clone(),
                team_id,
                assigned_to_user_id: di.item.assigned_to_user_id(),
                parent_name: Some(di.parent_name),
                due_date: di
                    .item
                    .due_date()
                    .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
                scheduled_date: di
                    .item
                    .scheduled_date()
                    .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
                complete: Some(di.item.complete),
                recurrence: di.item.recurrence_pattern(),
                recurrence_basis: di.item.recurrence_basis(),
                has_due_time: Some(di.item.has_due_time()),
            }
        })
        .collect();
    Ok(output::ListItemsDueOutput { items })
}

pub async fn list_assigned_items(
    input: input::ListAssignedItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
) -> Result<output::ListAssignedItemsOutput, error::ListAssignedItemsError> {
    let items = repo
        .list_assigned(&input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;

    let mut team_id_by_project: HashMap<String, Option<String>> = HashMap::new();
    for project_id in items.iter().filter_map(|i| i.project_id.clone()) {
        if let std::collections::hash_map::Entry::Vacant(entry) = team_id_by_project.entry(project_id.clone())
        {
            let project = projects
                .get(&project_id)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            entry.insert(project.team_id);
        }
    }

    let items = items
        .into_iter()
        .map(|i| {
            let team_id = i
                .project_id
                .as_ref()
                .and_then(|pid| team_id_by_project.get(pid).cloned().flatten());
            todo_server_sdk::model::AssignedItemSummary {
                item_id: i.id.clone(),
                name: i.name.clone(),
                owner_user_id: i.user_id.clone().or(team_id).unwrap_or_default(),
                due_date: i
                    .due_date()
                    .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
                scheduled_date: i
                    .scheduled_date()
                    .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
                complete: Some(i.complete),
                recurrence: i.recurrence_pattern(),
                recurrence_basis: i.recurrence_basis(),
                has_due_time: Some(i.has_due_time()),
            }
        })
        .collect();
    Ok(output::ListAssignedItemsOutput { items })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::Item;
    use crate::domain::project::Project;
    use crate::storage::sqlite::{DueItem, MockItemRepo, MockProjectRepo};

    fn personal_project() -> Project {
        Project {
            id: "p-personal".to_string(),
            name: "Personal".to_string(),
            owner_user_id: "u1".to_string(),
            team_id: None,
        }
    }

    fn team_project() -> Project {
        Project {
            id: "p-team".to_string(),
            name: "Team".to_string(),
            owner_user_id: "u1".to_string(),
            team_id: Some("team1".to_string()),
        }
    }

    #[tokio::test]
    async fn list_items_due_resolves_team_id_via_project() {
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_list_due().returning(|_, _, _| {
            Ok(vec![
                DueItem {
                    item: Item {
                        id: "i-personal".to_string(),
                        project_id: Some("p-personal".to_string()),
                        ..Item::default()
                    },
                    parent_name: String::new(),
                },
                DueItem {
                    item: Item {
                        id: "i-team".to_string(),
                        project_id: Some("p-team".to_string()),
                        ..Item::default()
                    },
                    parent_name: String::new(),
                },
            ])
        });
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .times(1)
            .withf(|id: &str| id == "p-personal")
            .returning(|_| Ok(personal_project()));
        projects_mock
            .expect_get()
            .times(1)
            .withf(|id: &str| id == "p-team")
            .returning(|_| Ok(team_project()));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let output = list_items_due(
            input::ListItemsDueInput {
                user_id: "u1".to_string(),
                deadline_after: None,
                deadline_before: None,
            },
            server::Extension(repo),
            server::Extension(projects),
        )
        .await
        .unwrap();

        let personal = output.items.iter().find(|i| i.item_id == "i-personal").unwrap();
        assert_eq!(personal.team_id, None);
        let team = output.items.iter().find(|i| i.item_id == "i-team").unwrap();
        assert_eq!(team.team_id, Some("team1".to_string()));
    }

    #[tokio::test]
    async fn list_assigned_items_resolves_owner_via_project_team() {
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_list_assigned().returning(|_| {
            Ok(vec![
                Item {
                    id: "i-personal".to_string(),
                    user_id: Some("owner1".to_string()),
                    project_id: Some("p-personal".to_string()),
                    ..Item::default()
                },
                Item {
                    id: "i-team".to_string(),
                    project_id: Some("p-team".to_string()),
                    ..Item::default()
                },
            ])
        });
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .times(1)
            .withf(|id: &str| id == "p-personal")
            .returning(|_| Ok(personal_project()));
        projects_mock
            .expect_get()
            .times(1)
            .withf(|id: &str| id == "p-team")
            .returning(|_| Ok(team_project()));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let output = list_assigned_items(
            input::ListAssignedItemsInput {
                user_id: "u1".to_string(),
            },
            server::Extension(repo),
            server::Extension(projects),
        )
        .await
        .unwrap();

        let personal = output.items.iter().find(|i| i.item_id == "i-personal").unwrap();
        assert_eq!(personal.owner_user_id, "owner1");
        let team = output.items.iter().find(|i| i.item_id == "i-team").unwrap();
        assert_eq!(team.owner_user_id, "team1");
    }
}
