use crate::domain::item::Item;
use crate::domain::recurrence;
use crate::service::items::{clone_children, ItemError};
use crate::storage::sqlite::{ItemRepo, TeamRepo};
use chrono::{DateTime, Utc};
use std::sync::Arc;

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
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_tasks: Option<bool>,
    pub parent_item_id: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
}

/// Moved from `json_api::team_items::update_team_item`. Only this one function is extracted
/// so far (not the full team_items CRUD set) — it's the one `web_ui` needs today, for the
/// dashboard's team-owned-item complete-toggle (see `web_ui::dashboard`). The rest of
/// `json_api::team_items` moves the same way once Stage 3 (team items screen) needs it from
/// `web_ui` too.
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
    let current = repo.get_team_item(&params.team_id, &params.item_id).await?;

    let mut item = Item::new_team_item(&params.team_id, &params.name);
    item.id = params.item_id.clone();
    item.complete = params.complete;
    item.due_date = params.due_date;
    item.recurrence = params.recurrence.clone();
    item.recurrence_basis = params.recurrence_basis.clone();
    item.has_due_time = params.has_due_time.unwrap_or(false);
    item.has_tasks = params.has_tasks.unwrap_or(true);
    item.parent_item_id = params.parent_item_id.clone();
    item.due_offset_days = params.due_offset_days;
    item.assigned_to_user_id = if params.assigned_to_user_id == current.assigned_to_user_id {
        current.assigned_to_user_id.clone()
    } else {
        resolve_assignee(teams, &params.team_id, params.assigned_to_user_id).await?
    };

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if let Some(next_item) = item.next_recurrence(chrono::Utc::now(), tz_offset) {
        let next_deadline = next_item
            .due_date
            .expect("next_recurrence always sets a deadline");
        let next_id = repo.create(&next_item).await?;
        clone_children(repo, &item.id, &next_id, next_deadline, tz_offset).await?;
        repo.delete(&item.id).await?;
        return Ok(());
    }

    repo.update_team_item(&item).await?;
    Ok(())
}
