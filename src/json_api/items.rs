use super::{internal, not_found, to_domain_item_type, to_sdk_item_type};
use crate::service::items::{self as item_service, ItemError};
use crate::storage::sqlite::{ItemRepo, ProjectRepo, RepoError};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

fn to_create_item_error(e: ItemError) -> error::CreateItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

fn to_update_item_error(e: ItemError) -> error::UpdateItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

fn to_delete_item_error(e: ItemError) -> error::DeleteItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

pub async fn create_item(
    input: input::CreateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
) -> Result<output::CreateItemOutput, error::CreateItemError> {
    let due_date = input
        .due_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    let scheduled_date = input
        .scheduled_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    let scheduled_end_date = input
        .scheduled_end_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    let item_id = item_service::create_item(
        &repo,
        &projects,
        item_service::CreateItemParams {
            user_id: input.user_id,
            name: input.name,
            description: input.description,
            due_date,
            scheduled_date,
            scheduled_end_date,
            complete: input.complete,
            recurrence: input.recurrence,
            recurrence_basis: input.recurrence_basis,
            has_due_time: input.has_due_time,
            has_scheduled_time: input.has_scheduled_time,
            has_end_time: input.has_end_time,
            parent_item_id: input.parent_item_id,
            item_type: to_domain_item_type(input.item_type),
            event_type: input.event_type,
            due_offset_days: input.due_offset_days,
            source_event_id: input.source_event_id,
            timezone_offset_minutes: input.timezone_offset_minutes,
        },
    )
    .await
    .map_err(to_create_item_error)?;
    Ok(output::CreateItemOutput { item_id })
}

pub async fn update_item(
    input: input::UpdateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::UpdateItemOutput, error::UpdateItemError> {
    let due_date = input
        .due_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    let scheduled_date = input
        .scheduled_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    let scheduled_end_date = input
        .scheduled_end_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    item_service::update_item(
        &repo,
        item_service::UpdateItemParams {
            user_id: input.user_id,
            item_id: input.item_id,
            name: input.name,
            description: input.description,
            due_date,
            scheduled_date,
            scheduled_end_date,
            complete: input.complete,
            recurrence: input.recurrence,
            recurrence_basis: input.recurrence_basis,
            has_due_time: input.has_due_time,
            has_scheduled_time: input.has_scheduled_time,
            has_end_time: input.has_end_time,
            parent_item_id: input.parent_item_id,
            item_type: to_domain_item_type(input.item_type),
            event_type: input.event_type,
            due_offset_days: input.due_offset_days,
            source_event_id: input.source_event_id,
            timezone_offset_minutes: input.timezone_offset_minutes,
        },
    )
    .await
    .map_err(to_update_item_error)?;
    Ok(output::UpdateItemOutput {})
}

pub async fn delete_item(
    input: input::DeleteItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::DeleteItemOutput, error::DeleteItemError> {
    item_service::delete_item(&repo, &input.user_id, &input.item_id)
        .await
        .map_err(to_delete_item_error)?;
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
        .due_date()
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    let scheduled_date = item
        .scheduled_date()
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    let scheduled_end_date = item
        .scheduled_end_date()
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    Ok(output::GetItemOutput {
        name: item.name.clone(),
        description: item.description.clone(),
        due_date,
        scheduled_date,
        scheduled_end_date,
        complete: item.complete,
        recurrence: item.recurrence_pattern(),
        recurrence_basis: item.recurrence_basis(),
        has_due_time: Some(item.has_due_time()),
        has_scheduled_time: Some(item.has_scheduled_time()),
        has_end_time: Some(item.has_end_time()),
        parent_item_id: item.parent_item_id.clone(),
        has_children: Some(item.has_children),
        item_type: Some(to_sdk_item_type(item.kind())),
        event_type: item.event_type(),
        due_offset_days: item.due_offset_days(),
        assigned_to_user_id: item.assigned_to_user_id(),
        source_event_id: item.source_event_id(),
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
            item_id: Some(i.id.clone()),
            name: Some(i.name.clone()),
            description: i.description.clone(),
            due_date: i
                .due_date()
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            scheduled_date: i
                .scheduled_date()
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            scheduled_end_date: i
                .scheduled_end_date()
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(i.complete),
            recurrence: i.recurrence_pattern(),
            recurrence_basis: i.recurrence_basis(),
            has_due_time: Some(i.has_due_time()),
            has_scheduled_time: Some(i.has_scheduled_time()),
            has_end_time: Some(i.has_end_time()),
            parent_item_id: i.parent_item_id.clone(),
            has_children: Some(i.has_children),
            item_type: Some(to_sdk_item_type(i.kind())),
            event_type: i.event_type(),
            due_offset_days: i.due_offset_days(),
            assigned_to_user_id: i.assigned_to_user_id(),
            source_event_id: i.source_event_id(),
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
            item_id: di.item.id.clone(),
            name: di.item.name.clone(),
            owner_user_id: di.item.user_id.clone(),
            team_id: di.item.team_id.clone(),
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
            item_id: i.id.clone(),
            name: i.name.clone(),
            owner_user_id: i.user_id.clone().or(i.team_id.clone()).unwrap_or_default(),
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
        })
        .collect();
    Ok(output::ListAssignedItemsOutput { items })
}

#[cfg(test)]
mod tests {
    use super::*;
}
