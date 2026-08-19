use crate::domain::activity_log::ActivityLogEntry;
use crate::service::item_series;
use crate::service::items::ItemError;
use crate::service::project_items::{self, UpdateProjectItemParams};
use crate::service::projects::require_project_member;
use crate::service::team_items::require_active_member;
use crate::storage::sqlite::{
    ActivityLogRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, RepoError, TeamRepo,
};
use std::sync::Arc;

/// Resolves the project balance `entry`'s points should be clawed back from: its own
/// `project_id` if set, falling back to a `get_by_team` lookup for any pre-B2 row
/// that still has `project_id: NULL` — same defensive-fallback shape
/// `team_activity.rs`'s own B2d read-path cutover already used for the analogous gap.
/// A personal-item entry always has `project_id: Some(..)` (see
/// `service::items::update_item`), so it never reaches the `get_by_team` fallback —
/// that branch only exists for pre-B2 *team* rows, which always had a real `team_id`.
async fn resolve_reversal_project_id(
    projects: &Arc<dyn ProjectRepo>,
    entry: &ActivityLogEntry,
) -> Result<String, ItemError> {
    if let Some(project_id) = &entry.project_id {
        return Ok(project_id.clone());
    }
    let team_id = entry.team_id.as_deref().ok_or_else(|| {
        ItemError::Internal(format!(
            "activity log entry {} has neither a project_id nor a team_id to reverse points against",
            entry.id
        ))
    })?;
    projects
        .get_by_team(team_id)
        .await?
        .map(|p| p.id)
        .ok_or_else(|| {
            ItemError::Internal(format!(
                "team {team_id} has no backing project to reverse points against"
            ))
        })
}

/// Reverses a specific, still-unreversed activity-log entry: claws back its signed
/// `points_delta` from the entry's own resolved project balance (see
/// `resolve_reversal_project_id`), and flips its `reversed` flag. Shared by the
/// automatic checkbox-based reversal (`team_items::update_team_item`'s
/// `just_uncompleted` branch, and `items::update_item`'s analogous branch for
/// personal items), and `reverse_and_reopen` below (the manual-Undo path) — see
/// CLAUDE.md's Points plan, Stage 6, stage C1 of docs/project-abstraction-plan.md
/// (points authority moved from `team_members` to `project_members`), and
/// docs/issues.md's "unify completion-undo" note. `mark_reversed` is attempted
/// *first*, since its `WHERE reversed = 0` guard is the only atomic "claim" either
/// path has; only the caller that wins that race goes on to touch the point balance,
/// so two concurrent reversals of the same entry can never double-deduct. A no-op
/// `points_delta: 0` entry (a personal item, or a team item with no points
/// configured) still goes through this unchanged — clawing back zero is harmless, and
/// keeping one code path beats special-casing it away.
pub(crate) async fn reverse_entry(
    projects: &Arc<dyn ProjectRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    entry: &ActivityLogEntry,
) -> Result<(), ItemError> {
    activity_log.mark_reversed(&entry.id).await?;
    let project_id = resolve_reversal_project_id(projects, entry).await?;
    projects
        .add_project_points(&project_id, &entry.user_id, -entry.points_delta)
        .await?;
    Ok(())
}

/// Reopens `entry_item_id`'s item — flips it back to incomplete — if it still exists
/// and is currently complete. Delegates into `project_items::update_project_item`'s
/// `complete: false` path, the exact same path the checkbox itself already uses, so
/// every consequence of un-completing an item (points reversal via
/// `team_items::update_team_item`'s own `just_uncompleted` branch, series-cursor
/// restore via `item_series::record_task_uncompletion`, the completion-guard rules)
/// applies identically regardless of whether "uncomplete" was triggered by the
/// checkbox or by the activity feed's Undo button — see docs/issues.md's "unify
/// completion-undo" note, the reason this function exists at all. Every other field
/// is round-tripped from the item's own current state, per this app's usual
/// direct-overwrite convention (see CLAUDE.md's Recurrence/Scheduled sections).
///
/// A no-op if the item is gone entirely (the legacy recurring-item case this whole
/// mechanism originally existed for — completing one used to delete the row) or is
/// already incomplete (e.g. it was already un-completed via the checkbox, and this
/// entry is only being reversed for its points).
async fn reopen_item_if_still_complete(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    project_id: &str,
    item_id: &str,
    requester_user_id: &str,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    let Ok(item) = repo.get_by_project(project_id, item_id).await else {
        return Ok(());
    };
    if !item.complete {
        return Ok(());
    }
    project_items::update_project_item(
        repo,
        projects,
        teams,
        activity_log,
        event_series,
        requester_user_id,
        UpdateProjectItemParams {
            project_id: project_id.to_string(),
            item_id: item_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone(),
            due_date: item.due_date(),
            scheduled_date: item.scheduled_date(),
            scheduled_end_date: item.scheduled_end_date(),
            complete: false,
            has_due_time: Some(item.has_due_time()),
            has_scheduled_time: Some(item.has_scheduled_time()),
            has_end_time: Some(item.has_end_time()),
            parent_item_id: item.parent_item_id.clone(),
            item_type: Some(item.kind()),
            event_type: item.event_type(),
            due_offset_days: item.due_offset_days(),
            assigned_to_user_id: item.assigned_to_user_id(),
            source_event_id: item.source_event_id(),
            timezone_offset_minutes: Some(tz_offset_minutes),
            points: item.points(),
        },
    )
    .await
}

/// Shared core of both `undo_activity_log_entry` (team-scoped, legacy) and
/// `undo_project_activity_log_entry` (project-scoped) below — ownership/reversed
/// checks, then reverse the entry's points and reopen its item if one still exists.
/// The two public wrappers differ only in which scoping key they check membership
/// and entry ownership against (team vs project) — see docs/issues.md's "unify
/// completion-undo" note for why this isn't duplicated between them.
async fn reverse_and_reopen(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    entry: &ActivityLogEntry,
    requester_user_id: &str,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    if entry.user_id != requester_user_id {
        return Err(ItemError::Invalid(
            "only the user who earned this entry can undo it".to_string(),
        ));
    }
    if entry.reversed {
        return Err(ItemError::Invalid(
            "this activity log entry has already been reversed".to_string(),
        ));
    }
    // Validate the series-order constraint *before* `reverse_entry` touches
    // anything — the same "only the series' most recently completed occurrence can
    // be uncompleted" rule the checkbox path enforces pre-persistence
    // (`item_series::validate_uncompletable`) must gate this path too, and it has to
    // run before points are clawed back, not after: `reopen_item_if_still_complete`
    // below already runs this same check internally (via `update_project_item`), but
    // by then it would be too late — `reverse_entry` has no undo of its own, so a
    // rejected reopen would otherwise leave the entry marked reversed and the points
    // gone, with no way to restore them, while the item stays complete and the
    // cursor never moves. A no-op for a non-series item (`validate_uncompletable`'s
    // own no-op) or one that's already incomplete/gone.
    if let Some(project_id) = &entry.project_id
        && let Ok(item) = repo.get_by_project(project_id, &entry.item_id).await
        && item.complete
    {
        item_series::validate_uncompletable(event_series, &entry.item_id).await?;
    }
    reverse_entry(projects, activity_log, entry).await?;
    if let Some(project_id) = &entry.project_id {
        reopen_item_if_still_complete(
            repo,
            projects,
            teams,
            activity_log,
            event_series,
            project_id,
            &entry.item_id,
            requester_user_id,
            tz_offset_minutes,
        )
        .await?;
    }
    Ok(())
}

/// Reverses a completion's points directly by log entry id, and reopens its item if
/// one still exists (see `reverse_and_reopen`) — team-scoped, kept for the legacy
/// `UndoActivityLogEntry` Smithy operation (`prl teams undo-activity` and its MCP
/// tool). `undo_project_activity_log_entry` below is the project-scoped sibling the
/// web UI's activity feed actually calls, since a personal project has no team to
/// scope by.
pub async fn undo_activity_log_entry(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    projects: &Arc<dyn ProjectRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    team_id: &str,
    entry_id: &str,
    requester_user_id: &str,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    require_active_member(teams, team_id, requester_user_id).await?;
    let entry = activity_log.get_entry(entry_id).await.map_err(|e| match e {
        RepoError::NotFound => ItemError::NotFound,
        _ => ItemError::Internal(format!("{e:?}")),
    })?;
    if entry.team_id.as_deref() != Some(team_id) {
        return Err(ItemError::NotFound);
    }
    reverse_and_reopen(
        repo,
        projects,
        teams,
        activity_log,
        event_series,
        &entry,
        requester_user_id,
        tz_offset_minutes,
    )
    .await
}

/// Project-scoped sibling of `undo_activity_log_entry` above, for the web UI's
/// project activity feed (`src/web_ui/project_activity.rs`) — works for any project,
/// personal or team-backed, since it's gated by project membership
/// (`require_project_member`) rather than team membership. See docs/issues.md's
/// "unify completion-undo" note.
pub async fn undo_project_activity_log_entry(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    project_id: &str,
    entry_id: &str,
    requester_user_id: &str,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let entry = activity_log.get_entry(entry_id).await.map_err(|e| match e {
        RepoError::NotFound => ItemError::NotFound,
        _ => ItemError::Internal(format!("{e:?}")),
    })?;
    if entry.project_id.as_deref() != Some(project_id) {
        return Err(ItemError::NotFound);
    }
    reverse_and_reopen(
        repo,
        projects,
        teams,
        activity_log,
        event_series,
        &entry,
        requester_user_id,
        tz_offset_minutes,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::Item;
    use crate::storage::sqlite::{
        MockActivityLogRepo, MockItemRepo, MockItemSeriesRepo, MockProjectRepo, MockTeamRepo,
    };
    use chrono::{DateTime, Utc};

    fn entry(user_id: &str, team_id: &str, points_delta: i32, reversed: bool) -> ActivityLogEntry {
        ActivityLogEntry {
            id: "e1".to_string(),
            team_id: Some(team_id.to_string()),
            project_id: Some("p1".to_string()),
            user_id: user_id.to_string(),
            item_id: "item1".to_string(),
            item_name: "Mow the lawn".to_string(),
            points_delta,
            reversed,
            created_at: Utc::now(),
        }
    }

    fn member_teams() -> MockTeamRepo {
        let mut teams = MockTeamRepo::new();
        teams
            .expect_member_status()
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));
        teams
    }

    fn projects_with_points() -> MockProjectRepo {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_add_project_points()
            .withf(|project_id, user_id, delta| {
                project_id == "p1" && user_id == "u1" && *delta == -30
            })
            .times(1)
            .returning(|_, _, _| Ok(0));
        projects
    }

    /// The item behind `entry()` is already gone (or was never fetched) — every test
    /// that isn't specifically exercising `reopen_item_if_still_complete` can pass
    /// this, so `reverse_and_reopen`'s reopen step is a harmless no-op.
    fn no_item_to_reopen() -> Arc<dyn ItemRepo> {
        let mut mock = MockItemRepo::new();
        mock.expect_get_by_project()
            .returning(|_, _| Err(RepoError::NotFound));
        Arc::new(mock)
    }

    fn no_op_event_series() -> Arc<dyn ItemSeriesRepo> {
        let mut mock = MockItemSeriesRepo::new();
        mock.expect_find_occurrence_by_item_id()
            .returning(|_| Ok(None));
        Arc::new(mock)
    }

    #[tokio::test]
    async fn reverse_entry_reads_logged_delta_not_some_other_value() {
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_with_points());

        let mut log = MockActivityLogRepo::new();
        log.expect_mark_reversed().returning(|_| Ok(()));

        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        reverse_entry(&projects, &log, &entry("u1", "t1", 30, false))
            .await
            .expect("should reverse");
    }

    #[tokio::test]
    async fn reverse_entry_falls_back_to_teams_backing_project_when_entry_has_none() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get_by_team()
            .withf(|team_id| team_id == "t1")
            .returning(|_| {
                Ok(Some(crate::domain::project::Project {
                    id: "p1".to_string(),
                    name: "Family".to_string(),
                    owner_user_id: "owner1".to_string(),
                    team_id: Some("t1".to_string()),
                }))
            });
        projects
            .expect_add_project_points()
            .withf(|project_id, user_id, delta| {
                project_id == "p1" && user_id == "u1" && *delta == -30
            })
            .times(1)
            .returning(|_, _, _| Ok(0));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);

        let mut log = MockActivityLogRepo::new();
        log.expect_mark_reversed().returning(|_| Ok(()));
        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let mut pre_b2_entry = entry("u1", "t1", 30, false);
        pre_b2_entry.project_id = None;

        reverse_entry(&projects, &log, &pre_b2_entry)
            .await
            .expect("should reverse via the resolved backing project");
    }

    #[tokio::test]
    async fn undo_activity_log_entry_rejects_non_owner() {
        let repo = no_item_to_reopen();
        let teams: Arc<dyn TeamRepo> = Arc::new(member_teams());
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let event_series = no_op_event_series();

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "t1", 30, false)));

        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let err = undo_activity_log_entry(
            &repo,
            &teams,
            &projects,
            &log,
            &event_series,
            "t1",
            "e1",
            "u2",
            0,
        )
        .await
        .expect_err("should reject a non-owner's undo attempt");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn undo_activity_log_entry_rejects_already_reversed() {
        let repo = no_item_to_reopen();
        let teams: Arc<dyn TeamRepo> = Arc::new(member_teams());
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let event_series = no_op_event_series();

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "t1", 30, true)));

        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let err = undo_activity_log_entry(
            &repo,
            &teams,
            &projects,
            &log,
            &event_series,
            "t1",
            "e1",
            "u1",
            0,
        )
        .await
        .expect_err("should reject undoing an already-reversed entry");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    /// Real-DB round trip: mock-based tests above verify the call sequencing, but not
    /// that the actual SQL wiring (add_project_points' RETURNING clause, get_entry's
    /// lookup, mark_reversed's guard) agrees with itself end to end.
    #[tokio::test]
    async fn undo_activity_log_entry_round_trips_through_real_repos() {
        use crate::storage::sqlite::activity_log::SqliteActivityLogRepo;
        use crate::storage::sqlite::projects::SqliteProjectRepo;
        use crate::storage::sqlite::teams::SqliteTeamRepo;
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use sqlx::SqlitePool;
        use std::str::FromStr;

        async fn test_pool() -> SqlitePool {
            let opts = SqliteConnectOptions::from_str("sqlite::memory:")
                .unwrap()
                .shared_cache(true);
            let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
            sqlx::query(
                "CREATE TABLE team_members (
                    team_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'PENDING',
                    invited_by TEXT,
                    role TEXT NOT NULL DEFAULT 'member',
                    points INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (team_id, user_id)
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE projects (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    owner_user_id TEXT NOT NULL,
                    team_id TEXT
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE project_members (
                    project_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    role TEXT NOT NULL DEFAULT 'member',
                    points INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (project_id, user_id)
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE activity_log (
                    id TEXT PRIMARY KEY,
                    team_id TEXT,
                    project_id TEXT,
                    user_id TEXT NOT NULL,
                    item_id TEXT NOT NULL,
                    item_name TEXT NOT NULL,
                    points_delta INTEGER NOT NULL,
                    reversed INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            pool
        }

        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role, points) \
             VALUES ('t1', 'u1', 'ACTIVE', 'member', 30)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO projects (id, name, owner_user_id, team_id) \
             VALUES ('p1', 'Family', 'u1', 't1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role, points) \
             VALUES ('p1', 'u1', 'member', 30)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let repo = no_item_to_reopen();
        let teams: Arc<dyn TeamRepo> = Arc::new(SqliteTeamRepo(pool.clone()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(SqliteProjectRepo(pool.clone()));
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(SqliteActivityLogRepo(pool.clone()));
        let event_series = no_op_event_series();

        let entry_id = activity_log
            .log_activity(Some("t1"), Some("p1"), "u1", "item1", "Mow the lawn", 30)
            .await
            .unwrap();

        undo_activity_log_entry(
            &repo,
            &teams,
            &projects,
            &activity_log,
            &event_series,
            "t1",
            &entry_id,
            "u1",
            0,
        )
        .await
        .expect("should undo");

        let points: i64 = sqlx::query_scalar(
            "SELECT points FROM project_members WHERE project_id = 'p1' AND user_id = 'u1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(points, 0);

        let entry = activity_log.get_entry(&entry_id).await.unwrap();
        assert!(entry.reversed);

        let err = undo_activity_log_entry(
            &repo,
            &teams,
            &projects,
            &activity_log,
            &event_series,
            "t1",
            &entry_id,
            "u1",
            0,
        )
        .await
        .expect_err("should reject undoing an already-reversed entry");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn undo_activity_log_entry_allows_owner_of_unreversed_entry() {
        let repo = no_item_to_reopen();
        let teams: Arc<dyn TeamRepo> = Arc::new(member_teams());
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_with_points());
        let event_series = no_op_event_series();

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "t1", 30, false)));
        log.expect_mark_reversed().times(1).returning(|_| Ok(()));

        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        undo_activity_log_entry(
            &repo,
            &teams,
            &projects,
            &log,
            &event_series,
            "t1",
            "e1",
            "u1",
            0,
        )
        .await
        .expect("owner should be able to undo their own unreversed entry");
    }

    #[tokio::test]
    async fn undo_activity_log_entry_rejects_entry_from_a_different_team() {
        let repo = no_item_to_reopen();
        let teams: Arc<dyn TeamRepo> = Arc::new(member_teams());
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let event_series = no_op_event_series();

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "other-team", 30, false)));
        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let err = undo_activity_log_entry(
            &repo,
            &teams,
            &projects,
            &log,
            &event_series,
            "t1",
            "e1",
            "u1",
            0,
        )
        .await
        .expect_err("should reject an entry that doesn't belong to this team");
        assert!(matches!(err, ItemError::NotFound));
    }

    #[tokio::test]
    async fn undo_project_activity_log_entry_reopens_a_still_complete_item() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                project_id: Some("p1".to_string()),
                complete: true,
                ..Item::default()
            })
        });
        // `items::update_item` (the personal dispatch target) fetches its own
        // `current` via the personal-shaped `get`, separate from the `get_by_project`
        // fetch above.
        items.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                user_id: Some("u1".to_string()),
                project_id: Some("p1".to_string()),
                complete: true,
                ..Item::default()
            })
        });
        items.expect_update().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(crate::domain::project::Project {
                id: "p1".to_string(),
                name: "Personal".to_string(),
                owner_user_id: "u1".to_string(),
                team_id: None,
            }));
        projects_mock
            .expect_add_project_points()
            .withf(|project_id, user_id, delta| {
                project_id == "p1" && user_id == "u1" && *delta == 0
            })
            .times(1)
            .returning(|_, _, _| Ok(0));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let event_series = no_op_event_series();

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry().returning(|_| {
            Ok(ActivityLogEntry {
                id: "e1".to_string(),
                team_id: None,
                project_id: Some("p1".to_string()),
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                item_name: "Mow the lawn".to_string(),
                points_delta: 0,
                reversed: false,
                created_at: Utc::now(),
            })
        });
        log.expect_mark_reversed().times(1).returning(|_| Ok(()));
        // `items::update_item`'s own uncomplete-transition detection (inside the
        // `reopen_item_if_still_complete` -> `update_project_item` delegation) looks
        // for an entry to reverse too — nothing left, since the one entry here was
        // already reversed directly above.
        log.expect_most_recent_unreversed()
            .returning(|_, _| Ok(None));
        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        undo_project_activity_log_entry(
            &repo,
            &projects,
            &teams,
            &log,
            &event_series,
            "p1",
            "e1",
            "u1",
            0,
        )
        .await
        .expect("should reverse the entry and reopen the item");
    }

    /// 2026-08-19 fix: undoing an entry for a series occurrence that is no longer the
    /// series' current cursor position (some later occurrence has since been
    /// completed/skipped) must be rejected *before* points are touched — not after,
    /// the way a naive reverse-then-reopen ordering would. `add_project_points` and
    /// `mark_reversed` must never fire on a rejected undo.
    #[tokio::test]
    async fn undo_project_activity_log_entry_rejects_an_out_of_order_series_occurrence_without_reversing_points()
     {
        use crate::domain::item_series::{ItemOccurrence, ItemSeries};

        let occurrence_date = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let later_cursor = occurrence_date + chrono::Duration::days(7);

        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                name: "Standup".to_string(),
                project_id: Some("p1".to_string()),
                complete: true,
                ..Item::default()
            })
        });
        // Reopening must never even be attempted — the pre-check rejects first.
        items.expect_update().times(0);
        let repo: Arc<dyn ItemRepo> = Arc::new(items);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| {
            Ok(crate::domain::project::Project {
                id: "p1".to_string(),
                name: "Personal".to_string(),
                owner_user_id: "u1".to_string(),
                team_id: None,
            })
        });
        // The whole point of this test: points must never be touched.
        projects_mock.expect_add_project_points().times(0);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(move |_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date,
                    item_id: Some("item1".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock.expect_get_series().returning(move |_| {
            Ok(ItemSeries {
                id: "s1".to_string(),
                project_id: "p1".to_string(),
                name: "Standup".to_string(),
                description: None,
                event_type: None,
                recurrence: "every 7 days".to_string(),
                anchor_date: occurrence_date,
                item_type: crate::domain::item::ItemKind::Task,
                // The cursor has already moved past this occurrence — some later
                // occurrence was completed/skipped after it.
                cursor_date: Some(later_cursor),
                basis: None,
                template_item_id: None,
                assigned_to_user_id: None,
                points: None,
            })
        });
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: None,
                is_exdate: false,
            }))
        });
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry().returning(|_| {
            Ok(ActivityLogEntry {
                id: "e1".to_string(),
                team_id: None,
                project_id: Some("p1".to_string()),
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                item_name: "Standup".to_string(),
                points_delta: 0,
                reversed: false,
                created_at: Utc::now(),
            })
        });
        // Never reached — the pre-check rejects before reverse_entry runs.
        log.expect_mark_reversed().times(0);
        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let err = undo_project_activity_log_entry(
            &repo,
            &projects,
            &teams,
            &log,
            &event_series,
            "p1",
            "e1",
            "u1",
            0,
        )
        .await
        .expect_err("should reject undoing an out-of-order series occurrence");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn undo_project_activity_log_entry_rejects_entry_from_a_different_project() {
        let repo = no_item_to_reopen();
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(crate::domain::project::Project {
                id: "p1".to_string(),
                name: "Personal".to_string(),
                owner_user_id: "u1".to_string(),
                team_id: None,
            }));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let event_series = no_op_event_series();

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "t1", 0, false)));
        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let err = undo_project_activity_log_entry(
            &repo,
            &projects,
            &teams,
            &log,
            &event_series,
            "some-other-project",
            "e1",
            "u1",
            0,
        )
        .await
        .expect_err("should reject an entry that doesn't belong to this project");
        assert!(matches!(err, ItemError::NotFound));
    }
}
