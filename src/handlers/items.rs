use super::{clone_children, internal, not_found};
use crate::domain::{item::Item, recurrence};
use crate::storage::{ItemRepo, RepoError};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

pub async fn create_item(
    input: input::CreateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::CreateItemOutput, error::CreateItemError> {
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(internal)?;
    }
    if input.recurrence.is_some() && input.parent_item_id.is_some() {
        return Err(internal("child items cannot have their own recurrence; set dueOffsetDays instead").into());
    }
    let mut item = Item::new_user_item(&input.user_id, &input.name);
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    item.complete = input.complete.unwrap_or(false);
    item.recurrence = input.recurrence;
    item.recurrence_basis = input.recurrence_basis;
    item.has_due_time = input.has_due_time.unwrap_or(false);
    item.has_tasks = input.has_tasks.unwrap_or(true);
    item.parent_item_id = input.parent_item_id.clone();
    item.due_offset_days = input.due_offset_days;

    // Child items of a template automatically become template items
    if let Some(ref parent_id) = input.parent_item_id {
        if let Ok(parent) = repo.get(&input.user_id, parent_id).await {
            if parent.is_template {
                item.is_template = true;
            }
        }
    }

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
    Ok(output::CreateItemOutput { item_id })
}

pub async fn update_item(
    input: input::UpdateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::UpdateItemOutput, error::UpdateItemError> {
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(internal)?;
    }
    if input.recurrence.is_some() && input.parent_item_id.is_some() {
        return Err(internal("child items cannot have their own recurrence; set dueOffsetDays instead").into());
    }

    let current = repo
        .get(&input.user_id, &input.item_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => error::UpdateItemError::from(not_found()),
            _ => error::UpdateItemError::from(internal(format!("{e:?}"))),
        })?;

    let mut item = Item::new_user_item(&input.user_id, &input.name);
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
    item.assigned_to_user_id = current.assigned_to_user_id.clone();

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
        return Ok(output::UpdateItemOutput {});
    }

    repo.update(&item).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateItemError::from(not_found()),
        _ => error::UpdateItemError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateItemOutput {})
}

pub async fn delete_item(
    input: input::DeleteItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::DeleteItemOutput, error::DeleteItemError> {
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
        RepoError::NotFound => error::DeleteItemError::from(not_found()),
        _ => error::DeleteItemError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::DeleteItemOutput {})
}

pub async fn get_item(
    input: input::GetItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::GetItemOutput, error::GetItemError> {
    let item = repo
        .get(&input.user_id, &input.item_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => error::GetItemError::from(not_found()),
            _ => error::GetItemError::from(internal(format!("{e:?}"))),
        })?;
    let due_date = item
        .deadline
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()))
        .unwrap_or(SmithyDateTime::from_secs(0));
    Ok(output::GetItemOutput {
        name: item.name,
        due_date,
        complete: item.complete,
        recurrence: item.recurrence,
        recurrence_basis: item.recurrence_basis,
        has_due_time: Some(item.has_due_time),
        has_tasks: Some(item.has_tasks),
        parent_item_id: item.parent_item_id,
        has_children: Some(item.has_children),
        is_template: Some(item.is_template),
        due_offset_days: item.due_offset_days,
        assigned_to_user_id: item.assigned_to_user_id,
    })
}

pub async fn list_items(
    input: input::ListItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListItemsOutput, error::ListItemsError> {
    let items = if let Some(ref parent_id) = input.parent_item_id {
        repo.list_children(parent_id)
            .await
            .map_err(|e| internal(format!("{e:?}")))?
    } else {
        repo.list(&input.user_id)
            .await
            .map_err(|e| internal(format!("{e:?}")))?
    };
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::ItemSummary {
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
            is_template: Some(i.is_template),
            due_offset_days: i.due_offset_days,
            assigned_to_user_id: i.assigned_to_user_id,
        })
        .collect();
    Ok(output::ListItemsOutput { items })
}

pub async fn list_items_due(
    input: input::ListItemsDueInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListItemsDueOutput, error::ListItemsDueError> {
    let after = input.deadline_after.map(|t| t.secs());
    let before = input.deadline_before.map(|t| t.secs());
    let due_items = repo
        .list_due(&input.user_id, after, before)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let items = due_items
        .into_iter()
        .map(|di| todo_server_sdk::model::DueItemSummary {
            item_id: di.item.id,
            name: di.item.name,
            owner_user_id: di.item.user_id.or(di.item.team_id).unwrap_or_default(),
            assigned_to_user_id: di.item.assigned_to_user_id,
            parent_name: Some(di.parent_name),
            due_date: di
                .item
                .deadline
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(di.item.complete),
            recurrence: di.item.recurrence,
            recurrence_basis: di.item.recurrence_basis,
            has_due_time: Some(di.item.has_due_time),
        })
        .collect();
    Ok(output::ListItemsDueOutput { items })
}

pub async fn list_assigned_items(
    input: input::ListAssignedItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListAssignedItemsOutput, error::ListAssignedItemsError> {
    let items = repo
        .list_assigned(&input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::AssignedItemSummary {
            item_id: i.id,
            name: i.name,
            owner_user_id: i.user_id.or(i.team_id).unwrap_or_default(),
            due_date: i
                .deadline
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(i.complete),
            recurrence: i.recurrence,
            recurrence_basis: i.recurrence_basis,
            has_due_time: Some(i.has_due_time),
        })
        .collect();
    Ok(output::ListAssignedItemsOutput { items })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memory::InMemoryItemRepo;

    fn update_input(item_id: &str, name: &str, complete: bool) -> input::UpdateItemInput {
        input::UpdateItemInput {
            user_id: "u1".to_string(),
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
            timezone_offset_minutes: None,
        }
    }

    #[tokio::test]
    async fn completing_recurring_item_carries_children_to_next_instance() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let mut parent = Item::new_user_item("u1", "Weekly review");
        parent.recurrence = Some("every 7 days".to_string());
        let parent_id = item_repo.create(&parent).await.unwrap();

        let mut child = Item::new_user_item("u1", "Check inbox");
        child.parent_item_id = Some(parent_id.clone());
        let child_id = item_repo.create(&child).await.unwrap();

        let items: Arc<dyn ItemRepo> = item_repo.clone();
        let mut input = update_input(&parent_id, "Weekly review", true);
        input.recurrence = Some("every 7 days".to_string());

        update_item(input, server::Extension(items.clone()))
            .await
            .unwrap();

        // Old parent is gone, and its old id is no longer a valid parent.
        assert!(items.get("u1", &parent_id).await.is_err());

        let remaining = item_repo
            .list("u1")
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.name == "Weekly review")
            .expect("next occurrence should exist");
        assert_ne!(remaining.id, parent_id);
        assert!(!remaining.complete);

        let new_children = items.list_children(&remaining.id).await.unwrap();
        assert_eq!(new_children.len(), 1);
        assert_eq!(new_children[0].name, "Check inbox");
        assert_ne!(new_children[0].id, child_id);
        assert!(!new_children[0].complete);

        // The old child row was cleaned up, not left dangling on the deleted parent.
        assert!(items.list_children(&parent_id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn recurrence_carries_deadline_to_children_via_offset() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let mut parent = Item::new_user_item("u1", "Weekly review");
        parent.recurrence = Some("every 7 days".to_string());
        parent.deadline = Some(chrono::Utc::now());
        let parent_id = item_repo.create(&parent).await.unwrap();

        let mut with_offset = Item::new_user_item("u1", "Prep agenda");
        with_offset.parent_item_id = Some(parent_id.clone());
        with_offset.due_offset_days = Some(-2);
        with_offset.deadline = Some(chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc));
        item_repo.create(&with_offset).await.unwrap();

        let mut without_offset = Item::new_user_item("u1", "Miscellaneous note");
        without_offset.parent_item_id = Some(parent_id.clone());
        item_repo.create(&without_offset).await.unwrap();

        let items: Arc<dyn ItemRepo> = item_repo.clone();
        let mut input = update_input(&parent_id, "Weekly review", true);
        input.recurrence = Some("every 7 days".to_string());

        update_item(input, server::Extension(items.clone()))
            .await
            .unwrap();

        let remaining = item_repo
            .list("u1")
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.name == "Weekly review")
            .unwrap();
        let new_children = items.list_children(&remaining.id).await.unwrap();

        let prepped = new_children.iter().find(|c| c.name == "Prep agenda").unwrap();
        let root_deadline = remaining.deadline.unwrap();
        assert_eq!(
            prepped.deadline.unwrap().date_naive(),
            (root_deadline - chrono::Duration::days(2)).date_naive()
        );

        let misc = new_children.iter().find(|c| c.name == "Miscellaneous note").unwrap();
        assert!(misc.deadline.is_none());
    }

    #[tokio::test]
    async fn create_item_rejects_recurrence_on_child() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let items: Arc<dyn ItemRepo> = item_repo;

        let result = create_item(
            input::CreateItemInput {
                user_id: "u1".to_string(),
                name: "Subtask".to_string(),
                due_date: None,
                complete: None,
                recurrence: Some("every day".to_string()),
                recurrence_basis: None,
                has_due_time: None,
                has_tasks: None,
                parent_item_id: Some("parent1".to_string()),
                due_offset_days: None,
                timezone_offset_minutes: None,
            },
            server::Extension(items),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_item_rejects_recurrence_on_child() {
        let item_repo = Arc::new(InMemoryItemRepo::new());
        let mut child = Item::new_user_item("u1", "Subtask");
        child.parent_item_id = Some("parent1".to_string());
        let child_id = item_repo.create(&child).await.unwrap();
        let items: Arc<dyn ItemRepo> = item_repo;

        let mut input = update_input(&child_id, "Subtask", false);
        input.recurrence = Some("every day".to_string());
        input.parent_item_id = Some("parent1".to_string());

        let result = update_item(input, server::Extension(items)).await;

        assert!(result.is_err());
    }
}
