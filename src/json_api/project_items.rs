use super::{internal, not_found, to_domain_item_type, to_sdk_item_type};
use crate::auth::AuthUser;
use crate::service::items::ItemError;
use crate::service::project_items::{
    self as project_item_service, CreateProjectItemParams, UpdateProjectItemParams,
};
use crate::storage::sqlite::{
    ActivityLogRepo, EventSeriesRepo, ItemRepo, ProjectRepo, RepoError, TeamRepo, UserRepo,
};
use std::collections::HashMap;
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

fn to_create_project_item_error(e: ItemError) -> error::CreateProjectItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

fn to_delete_project_item_error(e: ItemError) -> error::DeleteProjectItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

pub async fn create_project_item(
    input: input::CreateProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateProjectItemOutput, error::CreateProjectItemError> {
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
    let item_id = project_item_service::create_project_item(
        &repo,
        &projects,
        &teams,
        &auth.user_id,
        CreateProjectItemParams {
            project_id: input.project_id,
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
            assigned_to_user_id: input.assigned_to_user_id,
            source_event_id: input.source_event_id,
            timezone_offset_minutes: input.timezone_offset_minutes,
            points: input.points,
        },
    )
    .await
    .map_err(to_create_project_item_error)?;
    Ok(output::CreateProjectItemOutput { item_id })
}

pub async fn get_project_item(
    input: input::GetProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::GetProjectItemOutput, error::GetProjectItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &input.project_id,
        &auth.user_id,
        &input.item_id,
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::GetProjectItemError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::GetProjectItemError::from(internal(msg))
        }
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
    Ok(output::GetProjectItemOutput {
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
        points: item.points(),
        source_event_id: item.source_event_id(),
    })
}

pub async fn update_project_item(
    input: input::UpdateProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(activity_log): server::Extension<Arc<dyn ActivityLogRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateProjectItemOutput, error::UpdateProjectItemError> {
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
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &auth.user_id,
        UpdateProjectItemParams {
            project_id: input.project_id,
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
            assigned_to_user_id: input.assigned_to_user_id,
            source_event_id: input.source_event_id,
            timezone_offset_minutes: input.timezone_offset_minutes,
            points: input.points,
        },
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::UpdateProjectItemError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::UpdateProjectItemError::from(internal(msg))
        }
    })?;
    Ok(output::UpdateProjectItemOutput {})
}

pub async fn delete_project_item(
    input: input::DeleteProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(event_series): server::Extension<Arc<dyn EventSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DeleteProjectItemOutput, error::DeleteProjectItemError> {
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &event_series,
        &auth.user_id,
        &input.project_id,
        &input.item_id,
    )
    .await
    .map_err(to_delete_project_item_error)?;
    Ok(output::DeleteProjectItemOutput {})
}

pub async fn list_project_items(
    input: input::ListProjectItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(users): server::Extension<Arc<dyn UserRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListProjectItemsOutput, error::ListProjectItemsError> {
    let items = project_item_service::list_project_items(
        &repo,
        &projects,
        &teams,
        &input.project_id,
        &auth.user_id,
        input.parent_item_id,
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::ListProjectItemsError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::ListProjectItemsError::from(internal(msg))
        }
    })?;
    let mut names = HashMap::<String, String>::new();
    for item in items.iter() {
        if let Some(id) = item.assigned_to_user_id() {
            match get_user_name(&id, &users).await {
                Some(name) => {
                    names.insert(id, name);
                }
                None => {
                    tracing::error!(
                        "unable to map assigned user id to assigned username: get_user_name returned None"
                    );
                }
            }
        }
    }
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::ProjectItemSummary {
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
            assigned_to_user_name: i
                .assigned_to_user_id()
                .map(|id| names.get(&id).unwrap_or(&"<Name>".to_string()).clone()),
            points: i.points(),
            source_event_id: i.source_event_id(),
        })
        .collect();
    Ok(output::ListProjectItemsOutput { items })
}

async fn get_user_name(id: &str, user_repo: &Arc<dyn UserRepo>) -> Option<String> {
    let user = user_repo
        .get(&id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => {
                tracing::error!("error: id {} not found", id);
            }
            RepoError::Internal(s) => {
                tracing::error!("internal error: {s}");
            }
        })
        .ok()?;
    Some(format!("{} {}", user.first_name, user.last_name))
}

#[cfg(test)]
mod tests {
    use super::*;
}
