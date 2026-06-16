use super::{internal, not_found};
use crate::domain::item::Item;
use crate::storage::{ItemRepo, RepoError};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server};

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
            assigned_to_user_id: None,
        })
        .collect();
    Ok(output::ListTemplatesOutput { items })
}
