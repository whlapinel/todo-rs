use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::service::items::ItemError;
use crate::service::team_items::{
    self as team_item_service, CreateTeamItemParams, UpdateTeamItemParams,
};
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo, UserRepo};
use std::collections::HashMap;
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

async fn require_active_member(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<(), String> {
    crate::service::team_items::require_active_member(teams, team_id, user_id)
        .await
        .map_err(|e| e.to_string())
}

fn to_create_team_item_error(e: ItemError) -> error::CreateTeamItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

fn to_delete_team_item_error(e: ItemError) -> error::DeleteTeamItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

pub async fn create_team_item(
    input: input::CreateTeamItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateTeamItemOutput, error::CreateTeamItemError> {
    let due_date = input
        .due_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    let item_id = team_item_service::create_team_item(
        &repo,
        &teams,
        &auth.user_id,
        CreateTeamItemParams {
            team_id: input.team_id,
            name: input.name,
            due_date,
            complete: input.complete,
            recurrence: input.recurrence,
            recurrence_basis: input.recurrence_basis,
            has_due_time: input.has_due_time,
            has_tasks: input.has_tasks,
            parent_item_id: input.parent_item_id,
            due_offset_days: input.due_offset_days,
            assigned_to_user_id: input.assigned_to_user_id,
            timezone_offset_minutes: input.timezone_offset_minutes,
        },
    )
    .await
    .map_err(to_create_team_item_error)?;
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
        .due_date
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    let scheduled_date = item
        .scheduled_date
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    Ok(output::GetTeamItemOutput {
        name: item.name,
        due_date,
        scheduled_date,
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
    let due_date = input
        .due_date
        .and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc));
    team_item_service::update_team_item(
        &repo,
        &teams,
        &auth.user_id,
        UpdateTeamItemParams {
            team_id: input.team_id,
            item_id: input.item_id,
            name: input.name,
            due_date,
            complete: input.complete,
            recurrence: input.recurrence,
            recurrence_basis: input.recurrence_basis,
            has_due_time: input.has_due_time,
            has_tasks: input.has_tasks,
            parent_item_id: input.parent_item_id,
            due_offset_days: input.due_offset_days,
            assigned_to_user_id: input.assigned_to_user_id,
            timezone_offset_minutes: input.timezone_offset_minutes,
        },
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::UpdateTeamItemError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::UpdateTeamItemError::from(internal(msg))
        }
    })?;
    Ok(output::UpdateTeamItemOutput {})
}

pub async fn delete_team_item(
    input: input::DeleteTeamItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DeleteTeamItemOutput, error::DeleteTeamItemError> {
    team_item_service::delete_team_item(&repo, &teams, &auth.user_id, &input.team_id, &input.item_id)
        .await
        .map_err(to_delete_team_item_error)?;
    Ok(output::DeleteTeamItemOutput {})
}

pub async fn list_team_items(
    input: input::ListTeamItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(users): server::Extension<Arc<dyn UserRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListTeamItemsOutput, error::ListTeamItemsError> {
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(internal)?;
    let items = repo
        .list_team_items(&input.team_id, input.parent_item_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let mut names = HashMap::<String, String>::new();
    for item in items.iter() {
        if let Some(id) = &item.assigned_to_user_id {
            match get_user_name(id, &users).await {
                Some(name) => {
                    names.insert(id.to_string(), name);
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
        .map(|i| todo_server_sdk::model::TeamItemSummary {
            item_id: Some(i.id),
            name: Some(i.name),
            due_date: i
                .due_date
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            scheduled_date: i
                .scheduled_date
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(i.complete),
            recurrence: i.recurrence,
            recurrence_basis: i.recurrence_basis,
            has_due_time: Some(i.has_due_time),
            has_tasks: Some(i.has_tasks),
            parent_item_id: i.parent_item_id,
            has_children: Some(i.has_children),
            due_offset_days: i.due_offset_days,
            assigned_to_user_id: i.assigned_to_user_id.clone(),
            assigned_to_user_name: i
                .assigned_to_user_id
                .map(|id| names.get(&id).unwrap_or(&"<Name>".to_string()).clone()),
        })
        .collect();
    Ok(output::ListTeamItemsOutput { items })
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
