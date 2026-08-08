use crate::domain::item::{Item, ItemType};
use crate::domain::recurrence;
use crate::service::items::{clone_children, copy_template_children, ItemError};
use crate::storage::sqlite::{ItemRepo, TeamRepo};
use chrono::{DateTime, Utc};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct CreateTeamItemParams {
    pub team_id: String,
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: Option<bool>,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemType>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
}

/// Moved from `json_api::team_items::create_team_item`.
pub async fn create_team_item(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    params: CreateTeamItemParams,
) -> Result<String, ItemError> {
    require_active_member(teams, &params.team_id, requester_user_id).await?;
    if let Some(ref r) = params.recurrence {
        recurrence::parse(r).map_err(ItemError::Invalid)?;
    }
    if params.recurrence.is_some() && params.parent_item_id.is_some() {
        return Err(ItemError::Invalid(
            "child items cannot have their own recurrence; set dueOffsetDays instead".to_string(),
        ));
    }
    if params.item_type == Some(ItemType::Template) {
        return Err(ItemError::Invalid(
            "item_type Template is not supported for team items".to_string(),
        ));
    }
    if let (Some(start), Some(end)) = (params.scheduled_date, params.scheduled_end_date)
        && end < start
    {
        return Err(ItemError::Invalid(
            "scheduledEndDate cannot be before scheduledDate".to_string(),
        ));
    }
    let mut item = Item::new_team_item(&params.team_id, &params.name);
    item.due_date = params.due_date;
    item.scheduled_date = params.scheduled_date;
    item.scheduled_end_date = params.scheduled_end_date;
    item.complete = params.complete.unwrap_or(false);
    item.recurrence = params.recurrence.clone();
    item.recurrence_basis = params.recurrence_basis;
    item.has_due_time = params.has_due_time.unwrap_or(false);
    item.has_scheduled_time = params.has_scheduled_time.unwrap_or(false);
    item.has_end_time = params.has_end_time.unwrap_or(false);
    item.parent_item_id = params.parent_item_id;
    item.item_type = params.item_type.unwrap_or_default();
    item.event_type = params.event_type.clone();
    item.due_offset_days = params.due_offset_days;
    item.assigned_to_user_id =
        resolve_assignee(teams, &params.team_id, params.assigned_to_user_id).await?;
    item.validate().map_err(ItemError::Invalid)?;

    if let Some(ref pattern) = item.recurrence
        && let Ok(rule) = recurrence::parse(pattern)
    {
        let basis = item.recurrence_basis.as_deref().unwrap_or("DUE_DATE");
        let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
        if basis == "DUE_DATE" && item.due_date.is_none() {
            let mut deadline = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
            if rule.time_override.is_none() {
                deadline = recurrence::apply_end_of_day(deadline, tz_offset);
            } else {
                item.has_due_time = true;
            }
            item.due_date = Some(deadline);
        } else if basis != "DUE_DATE" && item.scheduled_date.is_none() {
            let when = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
            if rule.time_override.is_some() {
                item.has_scheduled_time = true;
            }
            item.scheduled_date = Some(when);
        }
    }
    let item_id = repo.create(&item).await?;

    // Checklist templates are a personal-item concept (scoped to the requester,
    // not the team), but a team event can still trigger one onto itself — same
    // mechanism as service::items::create_item's trigger step.
    if let Some(ref event_type) = item.event_type {
        let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
        let root_date = item.due_date.or(item.scheduled_date);
        let templates = repo.list_templates(requester_user_id).await?;
        for tpl in templates
            .iter()
            .filter(|t| t.event_type.as_deref() == Some(event_type.as_str()))
        {
            copy_template_children(repo, &tpl.id, &item_id, root_date, tz_offset).await?;
        }
    }
    Ok(item_id)
}

/// Moved from `json_api::team_items::delete_team_item`, with one behavior fix (same class as
/// `service::items::delete_item`'s): the original never confirmed `item_id` actually belongs
/// to `team_id` before deleting it — an active member of *any* team could delete *any* item
/// by id, team-scoping notwithstanding. `repo.get_team_item` below closes that gap; a
/// mismatched pair now surfaces as `ItemError::NotFound`.
pub async fn delete_team_item(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    team_id: &str,
    item_id: &str,
) -> Result<(), ItemError> {
    require_active_member(teams, team_id, requester_user_id).await?;
    repo.get_team_item(team_id, item_id).await?;

    let mut queue = vec![item_id.to_string()];
    while let Some(parent_id) = queue.first().cloned() {
        queue.remove(0);
        let children = repo.list_children(&parent_id).await?;
        for child in children {
            queue.push(child.id.clone());
            repo.delete(&child.id).await?;
        }
    }
    repo.delete(item_id).await?;
    Ok(())
}

/// Moved from `json_api::team_items::require_active_member`.
pub(crate) async fn require_active_member(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<(), ItemError> {
    let status = teams
        .member_status(team_id, user_id)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
    match status.as_deref() {
        Some("ACTIVE") => Ok(()),
        Some(_) => Err(ItemError::Invalid("team invite not yet accepted".to_string())),
        None => Err(ItemError::Invalid(format!(
            "user id: {user_id} is not a member of team id: {team_id}"
        ))),
    }
}

/// Moved from `json_api::team_items::resolve_assignee`.
pub(crate) async fn resolve_assignee(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    assignee_id: Option<String>,
) -> Result<Option<String>, ItemError> {
    let Some(assignee_id) = assignee_id else {
        return Ok(None);
    };
    let status = teams
        .member_status(team_id, &assignee_id)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
    if status.as_deref() != Some("ACTIVE") {
        return Err(ItemError::Invalid(
            "assignee must be an active member of this team".to_string(),
        ));
    }
    Ok(Some(assignee_id))
}

#[derive(Debug, Default)]
pub struct UpdateTeamItemParams {
    pub team_id: String,
    pub item_id: String,
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemType>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
}

/// Moved from `json_api::team_items::update_team_item`.
pub async fn update_team_item(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    params: UpdateTeamItemParams,
) -> Result<(), ItemError> {
    require_active_member(teams, &params.team_id, requester_user_id).await?;
    if let Some(ref r) = params.recurrence {
        recurrence::parse(r).map_err(ItemError::Invalid)?;
    }
    if params.recurrence.is_some() && params.parent_item_id.is_some() {
        return Err(ItemError::Invalid(
            "child items cannot have their own recurrence; set dueOffsetDays instead"
                .to_string(),
        ));
    }
    if params.item_type == Some(ItemType::Template) {
        return Err(ItemError::Invalid(
            "item_type Template is not supported for team items".to_string(),
        ));
    }
    if let (Some(start), Some(end)) = (params.scheduled_date, params.scheduled_end_date)
        && end < start
    {
        return Err(ItemError::Invalid(
            "scheduledEndDate cannot be before scheduledDate".to_string(),
        ));
    }
    let current = repo.get_team_item(&params.team_id, &params.item_id).await?;

    let mut item = Item::new_team_item(&params.team_id, &params.name);
    item.id = params.item_id.clone();
    item.complete = params.complete;
    item.due_date = params.due_date;
    item.scheduled_date = params.scheduled_date;
    item.scheduled_end_date = params.scheduled_end_date;
    item.recurrence = params.recurrence.clone();
    item.recurrence_basis = params.recurrence_basis.clone();
    item.has_due_time = params.has_due_time.unwrap_or(false);
    item.has_scheduled_time = params.has_scheduled_time.unwrap_or(false);
    item.has_end_time = params.has_end_time.unwrap_or(false);
    item.parent_item_id = params.parent_item_id.clone();
    item.item_type = params.item_type.unwrap_or(current.item_type);
    item.event_type = params.event_type.clone();
    item.due_offset_days = params.due_offset_days;
    item.assigned_to_user_id = if params.assigned_to_user_id == current.assigned_to_user_id {
        current.assigned_to_user_id.clone()
    } else {
        resolve_assignee(teams, &params.team_id, params.assigned_to_user_id).await?
    };
    item.validate().map_err(ItemError::Invalid)?;

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if let Some((next_item, next_anchor)) = item.next_recurrence(chrono::Utc::now(), tz_offset) {
        let next_id = repo.create(&next_item).await?;
        clone_children(repo, &item.id, &next_id, next_anchor, tz_offset).await?;
        repo.delete(&item.id).await?;
        return Ok(());
    }

    repo.update_team_item(&item).await?;
    Ok(())
}
