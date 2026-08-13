use super::internal;
use crate::storage::sqlite::ItemRepo;
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

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
