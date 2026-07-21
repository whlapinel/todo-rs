use super::{clone_children, internal, not_found};
use crate::auth::AuthUser;
use crate::domain::{item::Item, recurrence};
use crate::storage::{ItemRepo, RepoError, TeamRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

async fn require_active_member(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<(), String> {
    let status = teams
        .member_status(team_id, user_id)
        .await
        .map_err(|e| format!("{e:?}"))?;
    match status.as_deref() {
        Some("ACTIVE") => Ok(()),
        Some(_) => Err("team invite not yet accepted".to_string()),
        None => Err("not a member of this team".to_string()),
    }
}

async fn resolve_assignee(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    assignee_id: Option<String>,
) -> Result<Option<String>, String> {
    let Some(assignee_id) = assignee_id else {
        return Ok(None);
    };
    let status = teams
        .member_status(team_id, &assignee_id)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if status.as_deref() != Some("ACTIVE") {
        return Err("assignee must be an active member of this team".to_string());
    }
    Ok(Some(assignee_id))
}

pub async fn create_team_item(
    input: input::CreateTeamItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateTeamItemOutput, error::CreateTeamItemError> {
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(internal)?;
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(internal)?;
    }
    if input.recurrence.is_some() && input.parent_item_id.is_some() {
        return Err(internal("child items cannot have their own recurrence; set dueOffsetDays instead").into());
    }
    let mut item = Item::new_team_item(&input.team_id, &input.name);
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    item.complete = input.complete.unwrap_or(false);
    item.recurrence = input.recurrence;
    item.recurrence_basis = input.recurrence_basis;
    item.has_due_time = input.has_due_time.unwrap_or(false);
    item.has_tasks = input.has_tasks.unwrap_or(true);
    item.parent_item_id = input.parent_item_id;
    item.due_offset_days = input.due_offset_days;
    item.assigned_to_user_id = resolve_assignee(&teams, &input.team_id, input.assigned_to_user_id)
        .await
        .map_err(internal)?;

    if item.deadline.is_none() {
        if let Some(ref pattern) = item.recurrence {
            if let Ok(rule) = recurrence::parse(pattern) {
                let tz_offset = input.timezone_offset_minutes.unwrap_or(0);
                let mut deadline = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
                if rule.time_override.is_none() {
                    deadline = recurrence::apply_end_of_day(deadline, tz_offset);
                } else {
                    item.has_due_time = true;
                }
                item.deadline = Some(deadline);
            }
        }
    }
    let item_id = repo
        .create(&item)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateTeamItemOutput { item_id })
}

pub async fn get_team_item(
    input: input::GetTeamItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::GetTeamItemOutput, error::GetTeamItemError> {
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(internal)?;
    let item = repo
        .get_team_item(&input.team_id, &input.item_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => error::GetTeamItemError::from(not_found()),
            _ => error::GetTeamItemError::from(internal(format!("{e:?}"))),
        })?;
    let due_date = item
        .deadline
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()))
        .unwrap_or(SmithyDateTime::from_secs(0));
    Ok(output::GetTeamItemOutput {
        name: item.name,
        due_date,
        complete: item.complete,
        recurrence: item.recurrence,
        recurrence_basis: item.recurrence_basis,
        has_due_time: Some(item.has_due_time),
        has_tasks: Some(item.has_tasks),
        parent_item_id: item.parent_item_id,
        has_children: Some(item.has_children),
        due_offset_days: item.due_offset_days,
        assigned_to_user_id: item.assigned_to_user_id,
    })
}

pub async fn update_team_item(
    input: input::UpdateTeamItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateTeamItemOutput, error::UpdateTeamItemError> {
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(internal)?;
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(internal)?;
    }
    if input.recurrence.is_some() && input.parent_item_id.is_some() {
        return Err(internal("child items cannot have their own recurrence; set dueOffsetDays instead").into());
    }
    let current = repo
        .get_team_item(&input.team_id, &input.item_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => error::UpdateTeamItemError::from(not_found()),
            _ => error::UpdateTeamItemError::from(internal(format!("{e:?}"))),
        })?;

    let mut item = Item::new_team_item(&input.team_id, &input.name);
    item.id = input.item_id.clone();
    item.complete = input.complete;
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    item.recurrence = input.recurrence.clone();
    item.recurrence_basis = input.recurrence_basis.clone();
    item.has_due_time = input.has_due_time.unwrap_or(false);
    item.has_tasks = input.has_tasks.unwrap_or(true);
    item.parent_item_id = input.parent_item_id.clone();
    item.due_offset_days = input.due_offset_days;
    item.assigned_to_user_id = if input.assigned_to_user_id == current.assigned_to_user_id {
        current.assigned_to_user_id.clone()
    } else {
        resolve_assignee(&teams, &input.team_id, input.assigned_to_user_id)
            .await
            .map_err(internal)?
    };

    let tz_offset = input.timezone_offset_minutes.unwrap_or(0);
    if let Some(next_item) = item.next_recurrence(chrono::Utc::now(), tz_offset) {
        let next_deadline = next_item.deadline.expect("next_recurrence always sets a deadline");
        let next_id = repo
            .create(&next_item)
            .await
            .map_err(|e| internal(format!("{e:?}")))?;
        clone_children(&repo, &item.id, &next_id, next_deadline, tz_offset)
            .await
            .map_err(|e| internal(format!("{e:?}")))?;
        repo.delete(&item.id)
            .await
            .map_err(|e| internal(format!("{e:?}")))?;
        return Ok(output::UpdateTeamItemOutput {});
    }

    repo.update_team_item(&item).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateTeamItemError::from(not_found()),
        _ => error::UpdateTeamItemError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateTeamItemOutput {})
}

pub async fn delete_team_item(
    input: input::DeleteTeamItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DeleteTeamItemOutput, error::DeleteTeamItemError> {
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(internal)?;
    let mut queue = vec![input.item_id.clone()];
    while let Some(parent_id) = queue.first().cloned() {
        queue.remove(0);
        let children = repo
            .list_children(&parent_id)
            .await
            .map_err(|e| internal(format!("{e:?}")))?;
        for child in children {
            queue.push(child.id.clone());
            repo.delete(&child.id)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
        }
    }
    repo.delete(&input.item_id).await.map_err(|e| match e {
        RepoError::NotFound => error::DeleteTeamItemError::from(not_found()),
        _ => error::DeleteTeamItemError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::DeleteTeamItemOutput {})
}

pub async fn list_team_items(
    input: input::ListTeamItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListTeamItemsOutput, error::ListTeamItemsError> {
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(internal)?;
    let items = repo
        .list_team_items(&input.team_id, input.parent_item_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::TeamItemSummary {
            item_id: Some(i.id),
            name: Some(i.name),
            due_date: i
                .deadline
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(i.complete),
            recurrence: i.recurrence,
            recurrence_basis: i.recurrence_basis,
            has_due_time: Some(i.has_due_time),
            has_tasks: Some(i.has_tasks),
            parent_item_id: i.parent_item_id,
            has_children: Some(i.has_children),
            due_offset_days: i.due_offset_days,
            assigned_to_user_id: i.assigned_to_user_id,
        })
        .collect();
    Ok(output::ListTeamItemsOutput { items })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use crate::domain::item::Item;
    use crate::storage::memory::InMemoryItemRepo;
    use crate::storage::{MockTeamRepo, RepoError};

    fn auth(user_id: &str) -> AuthUser {
        AuthUser { user_id: user_id.to_string() }
    }

    fn active_mock() -> MockTeamRepo {
        let mut m = MockTeamRepo::new();
        m.expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        m
    }

    fn non_member_mock() -> MockTeamRepo {
        let mut m = MockTeamRepo::new();
        m.expect_member_status()
            .returning(|_, _| Ok(None));
        m
    }

    fn pending_mock() -> MockTeamRepo {
        let mut m = MockTeamRepo::new();
        m.expect_member_status()
            .returning(|_, _| Ok(Some("PENDING".to_string())));
        m
    }

    fn create_input(team_id: &str, name: &str) -> input::CreateTeamItemInput {
        input::CreateTeamItemInput {
            team_id: team_id.to_string(),
            name: name.to_string(),
            due_date: None,
            complete: None,
            recurrence: None,
            recurrence_basis: None,
            has_due_time: None,
            has_tasks: None,
            parent_item_id: None,
            due_offset_days: None,
            assigned_to_user_id: None,
            timezone_offset_minutes: None,
        }
    }

    #[tokio::test]
    async fn create_succeeds_for_active_member() {
        let items: Arc<dyn ItemRepo> = Arc::new(InMemoryItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(active_mock());

        let result = create_team_item(
            create_input("t1", "Deploy server"),
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await;

        assert!(result.is_ok());
        assert!(!result.unwrap().item_id.is_empty());
    }

    #[tokio::test]
    async fn create_blocked_for_non_member() {
        let items: Arc<dyn ItemRepo> = Arc::new(InMemoryItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(non_member_mock());

        let result = create_team_item(
            create_input("t1", "Sneaky task"),
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_blocked_for_pending_invite() {
        let items: Arc<dyn ItemRepo> = Arc::new(InMemoryItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(pending_mock());

        let result = create_team_item(
            create_input("t1", "Too early"),
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_returns_correct_fields() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let item_id = item_repo
            .create(&Item::new_team_item("t1", "Build feature"))
            .await
            .unwrap();

        let items: Arc<dyn ItemRepo> = item_repo;
        let teams: Arc<dyn TeamRepo> = Arc::new(active_mock());

        let result = get_team_item(
            input::GetTeamItemInput { team_id: "t1".to_string(), item_id },
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await
        .unwrap();

        assert_eq!(result.name, "Build feature");
        assert!(!result.complete);
    }

    #[tokio::test]
    async fn get_blocked_for_non_member() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let item_id = item_repo
            .create(&Item::new_team_item("t1", "Secret"))
            .await
            .unwrap();

        let items: Arc<dyn ItemRepo> = item_repo;
        let teams: Arc<dyn TeamRepo> = Arc::new(non_member_mock());

        let result = get_team_item(
            input::GetTeamItemInput { team_id: "t1".to_string(), item_id },
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("outsider")),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_returns_items_for_team() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        item_repo.create(&Item::new_team_item("t1", "Alpha")).await.unwrap();
        item_repo.create(&Item::new_team_item("t1", "Beta")).await.unwrap();
        item_repo.create(&Item::new_team_item("t2", "Other")).await.unwrap();

        let items: Arc<dyn ItemRepo> = item_repo;
        let teams: Arc<dyn TeamRepo> = Arc::new(active_mock());

        let result = list_team_items(
            input::ListTeamItemsInput { team_id: "t1".to_string(), parent_item_id: None },
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await
        .unwrap();

        assert_eq!(result.items.len(), 2);
        assert!(result.items.iter().all(|i| i.name.as_deref() != Some("Other")));
    }

    #[tokio::test]
    async fn delete_removes_item() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let item_id = item_repo
            .create(&Item::new_team_item("t1", "To delete"))
            .await
            .unwrap();

        let items: Arc<dyn ItemRepo> = item_repo.clone();
        let teams: Arc<dyn TeamRepo> = Arc::new(active_mock());

        delete_team_item(
            input::DeleteTeamItemInput { team_id: "t1".to_string(), item_id: item_id.clone() },
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await
        .unwrap();

        let fetch = item_repo.get_team_item("t1", &item_id).await;
        assert!(matches!(fetch, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn create_rejects_assignee_not_in_team() {
        let items: Arc<dyn ItemRepo> = Arc::new(InMemoryItemRepo::new());

        // First call (require_active_member for creator) → ACTIVE
        // Second call (resolve_assignee for assignee) → None
        let mut mock = MockTeamRepo::new();
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        mock.expect_member_status().returning(move |_, _| {
            let n = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(Some("ACTIVE".to_string()))
            } else {
                Ok(None)
            }
        });
        let teams: Arc<dyn TeamRepo> = Arc::new(mock);

        let mut inp = create_input("t1", "Assign task");
        inp.assigned_to_user_id = Some("outsider".to_string());

        let result = create_team_item(
            inp,
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await;

        assert!(result.is_err());
    }

    fn update_input(item_id: &str, name: &str, complete: bool) -> input::UpdateTeamItemInput {
        input::UpdateTeamItemInput {
            team_id: "t1".to_string(),
            item_id: item_id.to_string(),
            name: name.to_string(),
            due_date: None,
            complete,
            recurrence: None,
            recurrence_basis: None,
            has_due_time: None,
            has_tasks: None,
            parent_item_id: None,
            due_offset_days: None,
            assigned_to_user_id: None,
            timezone_offset_minutes: None,
        }
    }

    #[tokio::test]
    async fn recurrence_carries_children_and_offset_deadline() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let mut parent = Item::new_team_item("t1", "Weekly sync");
        parent.recurrence = Some("every 7 days".to_string());
        parent.deadline = Some(chrono::Utc::now());
        let parent_id = item_repo.create(&parent).await.unwrap();

        let mut with_offset = Item::new_team_item("t1", "Prep agenda");
        with_offset.parent_item_id = Some(parent_id.clone());
        with_offset.due_offset_days = Some(-2);
        item_repo.create(&with_offset).await.unwrap();

        let items: Arc<dyn ItemRepo> = item_repo.clone();
        let teams: Arc<dyn TeamRepo> = Arc::new(active_mock());
        let mut input = update_input(&parent_id, "Weekly sync", true);
        input.recurrence = Some("every 7 days".to_string());

        update_team_item(
            input,
            server::Extension(items.clone()),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await
        .unwrap();

        assert!(item_repo.get_team_item("t1", &parent_id).await.is_err());

        let remaining = item_repo
            .list_team_items("t1", None)
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.name == "Weekly sync")
            .expect("next occurrence should exist");
        let new_children = items.list_children(&remaining.id).await.unwrap();
        assert_eq!(new_children.len(), 1);
        let prepped = &new_children[0];
        assert_eq!(
            prepped.deadline.unwrap().date_naive(),
            (remaining.deadline.unwrap() - chrono::Duration::days(2)).date_naive()
        );
    }

    #[tokio::test]
    async fn create_team_item_rejects_recurrence_on_child() {
        let items: Arc<dyn ItemRepo> = Arc::new(InMemoryItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(active_mock());

        let mut inp = create_input("t1", "Subtask");
        inp.recurrence = Some("every day".to_string());
        inp.parent_item_id = Some("parent1".to_string());

        let result = create_team_item(
            inp,
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_team_item_rejects_recurrence_on_child() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let mut child = Item::new_team_item("t1", "Subtask");
        child.parent_item_id = Some("parent1".to_string());
        let child_id = item_repo.create(&child).await.unwrap();
        let items: Arc<dyn ItemRepo> = item_repo;
        let teams: Arc<dyn TeamRepo> = Arc::new(active_mock());

        let mut input = update_input(&child_id, "Subtask", false);
        input.recurrence = Some("every day".to_string());
        input.parent_item_id = Some("parent1".to_string());

        let result = update_team_item(
            input,
            server::Extension(items),
            server::Extension(teams),
            server::Extension(auth("u1")),
        )
        .await;

        assert!(result.is_err());
    }
}
