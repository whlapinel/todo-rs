use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::service::activity_log as activity_log_service;
use crate::service::items::ItemError;
use crate::service::team_items::require_active_member;
use crate::storage::sqlite::{ActivityLogRepo, TeamRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, model, output, server, types::DateTime as SmithyDateTime};

/// Server-capped since this app has no pagination concept anywhere in the Smithy
/// model (see CLAUDE.md) — a per-team activity feed doesn't need one at this app's
/// scale, so a flat cap is enough.
const ACTIVITY_LOG_LIMIT: i64 = 100;

fn to_msg(e: ItemError) -> error::PeoplesRepublicOfListsError {
    match e {
        ItemError::NotFound => not_found(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg),
    }
}

pub async fn list_team_activity_log(
    input: input::ListTeamActivityLogInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(activity_log): server::Extension<Arc<dyn ActivityLogRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListTeamActivityLogOutput, error::ListTeamActivityLogError> {
    require_active_member(&teams, &input.team_id, &auth.user_id)
        .await
        .map_err(|e| error::ListTeamActivityLogError::from(to_msg(e)))?;
    let entries = activity_log
        .list_activity_for_team(&input.team_id, ACTIVITY_LOG_LIMIT)
        .await
        .map_err(|e| error::ListTeamActivityLogError::from(internal(format!("{e:?}"))))?
        .into_iter()
        .map(|e| model::ActivityLogEntrySummary {
            entry_id: e.id,
            user_id: e.user_id,
            item_id: e.item_id,
            item_name: e.item_name,
            points_delta: e.points_delta,
            reversed: e.reversed,
            created_at: SmithyDateTime::from_secs(e.created_at.timestamp()),
        })
        .collect();
    Ok(output::ListTeamActivityLogOutput { entries })
}

pub async fn undo_activity_log_entry(
    input: input::UndoActivityLogEntryInput,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(activity_log): server::Extension<Arc<dyn ActivityLogRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UndoActivityLogEntryOutput, error::UndoActivityLogEntryError> {
    activity_log_service::undo_activity_log_entry(
        &teams,
        &activity_log,
        &input.team_id,
        &input.entry_id,
        &auth.user_id,
    )
    .await
    .map_err(|e| error::UndoActivityLogEntryError::from(to_msg(e)))?;
    Ok(output::UndoActivityLogEntryOutput {})
}
