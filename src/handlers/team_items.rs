use super::{clone_children, internal, not_found};
use crate::auth::AuthUser;
use crate::domain::{item::Item, recurrence};
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo};
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
        return Err(internal(
            "child items cannot have their own recurrence; set dueOffsetDays instead",
        )
        .into());
    }
    let mut item = Item::new_team_item(&input.team_id, &input.name);
    if let Some(dt) = input.due_date {
        item.due_date = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
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

    if item.due_date.is_none()
        && let Some(ref pattern) = item.recurrence
        && let Ok(rule) = recurrence::parse(pattern)
    {
        let tz_offset = input.timezone_offset_minutes.unwrap_or(0);
        let mut deadline = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
        if rule.time_override.is_none() {
            deadline = recurrence::apply_end_of_day(deadline, tz_offset);
        } else {
            item.has_due_time = true;
        }
        item.due_date = Some(deadline);
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
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(internal)?;
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(internal)?;
    }
    if input.recurrence.is_some() && input.parent_item_id.is_some() {
        return Err(internal(
            "child items cannot have their own recurrence; set dueOffsetDays instead",
        )
        .into());
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
        item.due_date = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
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
        let next_deadline = next_item
            .due_date
            .expect("next_recurrence always sets a deadline");
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
            assigned_to_user_id: i.assigned_to_user_id,
        })
        .collect();
    Ok(output::ListTeamItemsOutput { items })
}

#[cfg(test)]
mod tests {
    use super::*;
}
