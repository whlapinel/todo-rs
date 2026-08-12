use crate::domain::activity_log::ActivityLogEntry;
use crate::service::items::ItemError;
use crate::service::team_items::require_active_member;
use crate::storage::sqlite::{ActivityLogRepo, RepoError, TeamRepo};
use std::sync::Arc;

/// Reverses a specific, still-unreversed activity-log entry: claws back its signed
/// `points_delta` from the entry's own `user_id`/`team_id`, and flips its `reversed`
/// flag. Shared by both the automatic checkbox-based reversal
/// (`team_items::update_team_item`'s `just_uncompleted` branch) and
/// `undo_activity_log_entry` below (the manual undo path for recurring completions,
/// where the original item row is already gone) — see CLAUDE.md's Points plan, Stage
/// 6. `mark_reversed` is attempted *first*, since its `WHERE reversed = 0` guard is
/// the only atomic "claim" either path has; only the caller that wins that race goes
/// on to touch the point balance, so two concurrent reversals of the same entry can
/// never double-deduct.
pub(crate) async fn reverse_entry(
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    entry: &ActivityLogEntry,
) -> Result<(), ItemError> {
    activity_log.mark_reversed(&entry.id).await?;
    teams
        .add_team_points(&entry.team_id, &entry.user_id, -entry.points_delta)
        .await?;
    Ok(())
}

/// Reverses a completion's points directly by log entry id, independent of the
/// item's own current state — the only way to undo a recurring item's completion,
/// since completing one deletes the old row entirely (see CLAUDE.md's Recurrence
/// section). Restricted to the entry's own `user_id`: only the person who
/// earned/lost the points can undo their own entry, no admin-override in this pass.
pub async fn undo_activity_log_entry(
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    team_id: &str,
    entry_id: &str,
    requester_user_id: &str,
) -> Result<(), ItemError> {
    require_active_member(teams, team_id, requester_user_id).await?;
    let entry = activity_log.get_entry(entry_id).await.map_err(|e| match e {
        RepoError::NotFound => ItemError::NotFound,
        _ => ItemError::Internal(format!("{e:?}")),
    })?;
    if entry.team_id != team_id {
        return Err(ItemError::NotFound);
    }
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
    reverse_entry(teams, activity_log, &entry).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{MockActivityLogRepo, MockTeamRepo};
    use chrono::Utc;

    fn entry(user_id: &str, team_id: &str, points_delta: i32, reversed: bool) -> ActivityLogEntry {
        ActivityLogEntry {
            id: "e1".to_string(),
            team_id: team_id.to_string(),
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

    #[tokio::test]
    async fn reverse_entry_reads_logged_delta_not_some_other_value() {
        let mut teams = MockTeamRepo::new();
        teams
            .expect_add_team_points()
            .withf(|team_id, user_id, delta| team_id == "t1" && user_id == "u1" && *delta == -30)
            .times(1)
            .returning(|_, _, _| Ok(0));

        let mut log = MockActivityLogRepo::new();
        log.expect_mark_reversed().returning(|_| Ok(()));

        let teams: Arc<dyn TeamRepo> = Arc::new(teams);
        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        reverse_entry(&teams, &log, &entry("u1", "t1", 30, false))
            .await
            .expect("should reverse");
    }

    #[tokio::test]
    async fn undo_activity_log_entry_rejects_non_owner() {
        let teams: Arc<dyn TeamRepo> = Arc::new(member_teams());

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "t1", 30, false)));

        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let err = undo_activity_log_entry(&teams, &log, "t1", "e1", "u2")
            .await
            .expect_err("should reject a non-owner's undo attempt");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn undo_activity_log_entry_rejects_already_reversed() {
        let teams: Arc<dyn TeamRepo> = Arc::new(member_teams());

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "t1", 30, true)));

        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        let err = undo_activity_log_entry(&teams, &log, "t1", "e1", "u1")
            .await
            .expect_err("should reject undoing an already-reversed entry");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    /// Real-DB round trip: mock-based tests above verify the call sequencing, but not
    /// that the actual SQL wiring (add_team_points' RETURNING clause, get_entry's
    /// lookup, mark_reversed's guard) agrees with itself end to end.
    #[tokio::test]
    async fn undo_activity_log_entry_round_trips_through_real_repos() {
        use crate::storage::sqlite::activity_log::SqliteActivityLogRepo;
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
                "CREATE TABLE activity_log (
                    id TEXT PRIMARY KEY,
                    team_id TEXT NOT NULL,
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

        let teams: Arc<dyn TeamRepo> = Arc::new(SqliteTeamRepo(pool.clone()));
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(SqliteActivityLogRepo(pool.clone()));

        let entry_id = activity_log
            .log_activity("t1", Some("p1"), "u1", "item1", "Mow the lawn", 30)
            .await
            .unwrap();

        undo_activity_log_entry(&teams, &activity_log, "t1", &entry_id, "u1")
            .await
            .expect("should undo");

        let points: i64 = sqlx::query_scalar(
            "SELECT points FROM team_members WHERE team_id = 't1' AND user_id = 'u1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(points, 0);

        let entry = activity_log.get_entry(&entry_id).await.unwrap();
        assert!(entry.reversed);

        let err = undo_activity_log_entry(&teams, &activity_log, "t1", &entry_id, "u1")
            .await
            .expect_err("should reject undoing an already-reversed entry");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn undo_activity_log_entry_allows_owner_of_unreversed_entry() {
        let teams_mock = {
            let mut teams = member_teams();
            teams
                .expect_add_team_points()
                .times(1)
                .returning(|_, _, _| Ok(0));
            teams
        };
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let mut log = MockActivityLogRepo::new();
        log.expect_get_entry()
            .returning(|_| Ok(entry("u1", "t1", 30, false)));
        log.expect_mark_reversed().times(1).returning(|_| Ok(()));

        let log: Arc<dyn ActivityLogRepo> = Arc::new(log);

        undo_activity_log_entry(&teams, &log, "t1", "e1", "u1")
            .await
            .expect("owner should be able to undo their own unreversed entry");
    }
}
