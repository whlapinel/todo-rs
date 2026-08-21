use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::service::activity_log as activity_log_service;
use crate::service::items::ItemError;
use crate::service::team_items::require_active_member;
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
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
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(activity_log): server::Extension<Arc<dyn ActivityLogRepo>>,
    server::Extension(series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UndoActivityLogEntryOutput, error::UndoActivityLogEntryError> {
    // No `timezoneOffsetMinutes` field on this legacy Smithy operation (see
    // CLAUDE.md's Smithy section — kept alive only for `prl teams undo-activity`/its
    // MCP tool, not worth extending its shape for) — 0 is the same fallback used
    // throughout this codebase when a real per-request offset isn't available. This
    // only affects the rare case of restoring an `item_series` cursor for a series
    // whose recurrence rule has an explicit time-of-day override.
    activity_log_service::undo_activity_log_entry(
        &repo,
        &teams,
        &projects,
        &activity_log,
        &series,
        &input.team_id,
        &input.entry_id,
        &auth.user_id,
        0,
    )
    .await
    .map_err(|e| error::UndoActivityLogEntryError::from(to_msg(e)))?;
    Ok(output::UndoActivityLogEntryOutput {})
}
