use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Drops `team_members.points` — see docs/project-abstraction-plan.md, stage C4.
/// Genuinely dead since stage C1 moved points authority to `project_members.points`:
/// nothing awards/reverses into it anymore (`TeamRepo::add_team_points` was removed in
/// this same stage), and `SqliteTeamRepo::list_members` already reads the live balance
/// via a `LEFT JOIN` through `projects`/`project_members` instead of this column.
///
/// `activity_log.team_id` — the other column C4's plan text originally proposed
/// dropping alongside this one — is deliberately **not** touched here. It's still read
/// by `list_activity_for_team`, which backs the legacy `ListTeamActivityLog`/
/// `UndoActivityLogEntry` JSON API operations, still called today by `prl teams
/// activity`/`prl teams undo-activity` and the MCP `list_team_activity_log` tool.
/// Dropping it would break those live call paths; deferred to a future stage once
/// that surface is retired or repointed at `project_id`, confirmed with the user
/// before this migration was written.
///
/// SQLite 3.35+ (this project's bundled `libsqlite3-sys`, confirmed in stage C's own
/// scope-correction note) supports real `ALTER TABLE ... DROP COLUMN`. Guarded with
/// `column_exists` — the inverse of every prior migration's ADD-COLUMN guard, but the
/// same reasoning: SQLite has no `DROP COLUMN IF EXISTS`, and this must be safe to run
/// against a DB that's already missing the column (a fresh DB, whose baseline
/// `CREATE TABLE IF NOT EXISTS team_members` no longer declares it).
pub struct DropTeamMemberPoints;

#[async_trait]
impl Migration for DropTeamMemberPoints {
    fn version(&self) -> i64 {
        12
    }

    fn name(&self) -> &str {
        "drop team_members.points"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if column_exists(conn, "team_members", "points").await? {
            sqlx::query("ALTER TABLE team_members DROP COLUMN points")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::{Row, SqlitePool};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    #[tokio::test]
    async fn drops_points_column_when_present() {
        let pool = test_pool().await;
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
            "INSERT INTO team_members (team_id, user_id, status, role, points) \
             VALUES ('t1', 'u1', 'ACTIVE', 'admin', 999)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        DropTeamMemberPoints.up(&mut conn).await.unwrap();

        assert!(!column_exists(&mut conn, "team_members", "points").await.unwrap());
        let role: String = sqlx::query("SELECT role FROM team_members WHERE team_id = 't1' AND user_id = 'u1'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("role");
        assert_eq!(role, "admin");
    }

    #[tokio::test]
    async fn is_idempotent_against_a_table_already_missing_points() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE team_members (
                team_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'PENDING',
                invited_by TEXT,
                role TEXT NOT NULL DEFAULT 'member',
                PRIMARY KEY (team_id, user_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        DropTeamMemberPoints.up(&mut conn).await.unwrap();
        DropTeamMemberPoints.up(&mut conn).await.unwrap();

        assert!(!column_exists(&mut conn, "team_members", "points").await.unwrap());
    }
}
