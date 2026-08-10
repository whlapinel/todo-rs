use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `activity_log` table — a brand-new table, so (unlike every other
/// migration in this module) there's no `column_exists`/`ALTER TABLE` dance: `CREATE
/// TABLE IF NOT EXISTS` is enough on its own. `create_pool()` defines the identical
/// baseline for a fresh DB, so this only does real work against a DB that predates it.
///
/// See CLAUDE.md's per-team roles/points design: `points_delta` is signed and is the
/// only "what happened" field (positive = completion); reversal flips `reversed`
/// rather than writing a second row. `item_name`/`team_id` are denormalized
/// deliberately — recurrence deletes/recreates items under new ids, and items can be
/// deleted outright, so the log must stay meaningful independent of the `items`
/// table's current state.
pub struct ActivityLog;

#[async_trait]
impl Migration for ActivityLog {
    fn version(&self) -> i64 {
        6
    }

    fn name(&self) -> &str {
        "create activity_log table"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS activity_log (
                id TEXT PRIMARY KEY,
                team_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                item_name TEXT NOT NULL,
                points_delta INTEGER NOT NULL,
                reversed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_activity_log_team_created \
             ON activity_log (team_id, created_at DESC)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_activity_log_item_id ON activity_log (item_id)")
            .execute(&mut *conn)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn empty_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    #[tokio::test]
    async fn creates_table_and_is_idempotent() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        ActivityLog.up(&mut conn).await.unwrap();
        ActivityLog.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, created_at) \
             VALUES ('1', 't1', 'u1', 'i1', 'Wash dishes', 5, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM activity_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
