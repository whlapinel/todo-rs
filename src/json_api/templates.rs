use super::{internal, not_found, to_sdk_item_type};
use crate::service::items::ItemError;
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::ItemRepo;
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server};

pub async fn create_template(
    input: input::CreateTemplateInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::CreateTemplateOutput, error::CreateTemplateError> {
    let template_id = template_service::create_template(
        &repo,
        CreateTemplateParams {
            user_id: input.user_id,
            name: input.name,
            description: input.description,
            source_item_id: input.source_item_id,
            event_type: input.event_type,
        },
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::CreateTemplateError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::CreateTemplateError::from(internal(msg))
        }
    })?;
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
            item_id: Some(i.id.clone()),
            name: Some(i.name.clone()),
            description: i.description.clone(),
            due_date: None,
            scheduled_date: None,
            scheduled_end_date: None,
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
            assigned_to_user_id: None,
            source_event_id: None,
        })
        .collect();
    Ok(output::ListTemplatesOutput { items })
}
