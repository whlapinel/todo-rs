use super::Migration;
use crate::storage::migrations::MigrationError;
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `item_series_rotation_members` — see docs/assignment-rotation-plan.md. A new
/// table, not an `ALTER TABLE` on an existing one, so this needs no `column_exists`
/// guard: `CREATE TABLE IF NOT EXISTS` is naturally idempotent, and `create_pool()`'s
/// baseline already includes this table for a fresh DB, so this only does real work
/// against a DB that predates it — same shape as `AddItemSeries`/`AddEventSeries`.
pub struct AddItemSeriesRotationMembers;

#[async_trait]
impl Migration for AddItemSeriesRotationMembers {
    fn version(&self) -> i64 {
        24
    }

    fn name(&self) -> &str {
        "add item_series_rotation_members"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS item_series_rotation_members (
                series_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                PRIMARY KEY (series_id, user_id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::{Row, SqlitePool};
    use std::str::FromStr;

    async fn empty_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    async fn table_exists(conn: &mut SqliteConnection, table: &str) -> bool {
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_optional(&mut *conn)
            .await
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn creates_the_table_when_missing() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(!table_exists(&mut conn, "item_series_rotation_members").await);

        AddItemSeriesRotationMembers.up(&mut conn).await.unwrap();

        assert!(table_exists(&mut conn, "item_series_rotation_members").await);
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesRotationMembers.up(&mut conn).await.unwrap();
        AddItemSeriesRotationMembers.up(&mut conn).await.unwrap();

        assert!(table_exists(&mut conn, "item_series_rotation_members").await);
    }

    #[tokio::test]
    async fn table_enforces_series_id_user_id_uniqueness() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        AddItemSeriesRotationMembers.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO item_series_rotation_members (series_id, user_id) VALUES ('s1', 'u1')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let err = sqlx::query(
            "INSERT INTO item_series_rotation_members (series_id, user_id) VALUES ('s1', 'u1')",
        )
        .execute(&mut *conn)
        .await
        .unwrap_err();
        assert!(err.to_string().contains("UNIQUE") || err.to_string().contains("constraint"));

        let row = sqlx::query("SELECT COUNT(*) as c FROM item_series_rotation_members")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let count: i64 = row.get("c");
        assert_eq!(count, 1);
    }
}
