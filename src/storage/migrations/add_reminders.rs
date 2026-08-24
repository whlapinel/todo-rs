use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `reminders` table — Stage 1 of the reminders feature
/// (docs/issues_and_features.md): schema + auto-population only, see
/// `service::reminders::sync_item_reminders`, the sole writer. Brand-new table, so (like
/// `AddCalendarSubscriptions`) `CREATE TABLE IF NOT EXISTS` alone is enough, no
/// `column_exists` dance.
pub struct AddReminders;

#[async_trait]
impl Migration for AddReminders {
    fn version(&self) -> i64 {
        28
    }

    fn name(&self) -> &str {
        "add reminders table"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS reminders (
                id TEXT PRIMARY KEY,
                item_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'AUTO',
                remind_at INTEGER NOT NULL,
                sent_at INTEGER,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_reminders_item_id ON reminders (item_id)")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_reminders_user_remind_at \
             ON reminders (user_id, remind_at)",
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn memory_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    #[tokio::test]
    async fn creates_table_and_is_idempotent() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddReminders.up(&mut conn).await.unwrap();
        AddReminders.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO reminders \
             (id, item_id, project_id, user_id, kind, remind_at, created_at) \
             VALUES ('r1', 'i1', 'p1', 'u1', 'DUE', 1000, 1000)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reminders")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
