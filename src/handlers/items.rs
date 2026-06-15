use super::{internal, not_found};
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
    let mut item = Item::new(&input.user_id, &input.name);
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

    let mut item = Item::new(&input.user_id, &input.name);
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

    if item.complete {
        if let Some(ref pattern) = item.recurrence {
            if let Ok(rule) = recurrence::parse(pattern) {
                let reference = if item.recurrence_basis.as_deref() == Some("COMPLETION_DATE") {
                    chrono::Utc::now()
                } else {
                    item.deadline.unwrap_or_else(chrono::Utc::now)
                };
                let tz_offset = input.timezone_offset_minutes.unwrap_or(0);
                let next_deadline = recurrence::next_date(&rule, reference, tz_offset);
                let mut next_item = Item::new(&item.user_id, &item.name);
                next_item.deadline = Some(next_deadline);
                next_item.recurrence = item.recurrence.clone();
                next_item.recurrence_basis = item.recurrence_basis.clone();
                next_item.has_tasks = item.has_tasks;
                next_item.parent_item_id = item.parent_item_id.clone();
                next_item.has_due_time = if rule.time_override.is_some() {
                    true
                } else {
                    item.has_due_time
                };
                repo.create(&next_item)
                    .await
                    .map_err(|e| internal(format!("{e:?}")))?;
                repo.delete(&item.id)
                    .await
                    .map_err(|e| internal(format!("{e:?}")))?;
                return Ok(output::UpdateItemOutput {});
            }
        }
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
    // BFS cascade-delete all descendants before deleting this item
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
        has_due_time: Some(item.has_due_time),
        has_tasks: Some(item.has_tasks),
        parent_item_id: item.parent_item_id,
        has_children: Some(item.has_children),
        is_template: Some(item.is_template),
        due_offset_days: item.due_offset_days,
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

pub async fn create_template(
    input: input::CreateTemplateInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::CreateTemplateOutput, error::CreateTemplateError> {
    let mut item = Item::new(&input.user_id, &input.name);
    item.is_template = true;

    if let Some(source_id) = input.source_item_id {
        let source = repo
            .get(&input.user_id, &source_id)
            .await
            .map_err(|e| match e {
                RepoError::NotFound => error::CreateTemplateError::from(not_found()),
                _ => error::CreateTemplateError::from(internal(format!("{e:?}"))),
            })?;
        item.name = source.name;
        item.recurrence = source.recurrence;
        item.recurrence_basis = source.recurrence_basis;
        item.has_due_time = source.has_due_time;
        item.has_tasks = source.has_tasks;
        item.due_offset_days = source.due_offset_days;
        // deadline intentionally not copied — templates have no dates
    }

    let template_id = repo
        .create(&item)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateTemplateOutput { template_id })
}

pub async fn list_templates(
    input: input::ListTemplatesInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListTemplatesOutput, error::ListTemplatesError> {
    let items = repo
        .list_templates(&input.user_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::ItemSummary {
            item_id: Some(i.id),
            name: Some(i.name),
            due_date: None,
            complete: Some(i.complete),
            recurrence: i.recurrence,
            recurrence_basis: i.recurrence_basis,
            has_due_time: Some(i.has_due_time),
            has_tasks: Some(i.has_tasks),
            parent_item_id: i.parent_item_id,
            has_children: Some(i.has_children),
            is_template: Some(true),
            due_offset_days: i.due_offset_days,
        })
        .collect();
    Ok(output::ListTemplatesOutput { items })
}
