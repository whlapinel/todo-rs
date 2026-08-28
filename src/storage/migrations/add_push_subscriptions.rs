use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `push_subscriptions` table and adds `reminders.push_sent_at` — the push
/// delivery stage of `docs/push-notifications-plan.md`. `push_sent_at` is a second,
/// independent "delivered" marker alongside `reminders.sent_at` (the pre-existing in-app
/// dismiss marker) so the two channels don't interfere with each other — see the plan doc.
/// `push_subscriptions` is brand-new, so `CREATE TABLE IF NOT EXISTS` alone is enough (no
/// `column_exists` dance, matching `AddReminders`/`AddItemDependencies`); `reminders` is
/// pre-existing, so its new column is guarded with `column_exists` (matching
/// `AddUserTimezone`).
pub struct AddPushSubscriptions;

#[async_trait]
impl Migration for AddPushSubscriptions {
    fn version(&self) -> i64 {
        30
    }

    fn name(&self) -> &str {
        "add push_subscriptions table and reminders.push_sent_at"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS push_subscriptions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                endpoint TEXT NOT NULL UNIQUE,
                p256dh_key TEXT NOT NULL,
                auth_key TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_push_subscriptions_user_id \
             ON push_subscriptions (user_id)",
        )
        .execute(&mut *conn)
        .await?;

        if !column_exists(conn, "reminders", "push_sent_at").await? {
            sqlx::query("ALTER TABLE reminders ADD COLUMN push_sent_at INTEGER")
                .execute(&mut *conn)
                .await?;
        }

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
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE reminders (
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
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn creates_table_and_column_and_is_idempotent() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddPushSubscriptions.up(&mut conn).await.unwrap();
        AddPushSubscriptions.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO push_subscriptions \
             (id, user_id, endpoint, p256dh_key, auth_key, created_at) \
             VALUES ('s1', 'u1', 'https://push.example/e1', 'p256dh', 'auth', 1000)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_subscriptions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);

        sqlx::query(
            "INSERT INTO reminders \
             (id, item_id, project_id, user_id, kind, remind_at, push_sent_at, created_at) \
             VALUES ('r1', 'i1', 'p1', 'u1', 'DUE', 1000, 2000, 1000)",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
}
