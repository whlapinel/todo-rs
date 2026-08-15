use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds points/assignment authority to `item_series` — see
/// `domain::item_series::ItemSeries::assigned_to_user_id`/`points`. `CREATE TABLE IF
/// NOT EXISTS item_series` already includes both columns, so this is a no-op against a
/// fresh DB; it only does work against a DB that predates it.
pub struct AddItemSeriesAssignment;

#[async_trait]
impl Migration for AddItemSeriesAssignment {
    fn version(&self) -> i64 {
        21
    }

    fn name(&self) -> &str {
        "add assigned_to_user_id/points to item_series"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "item_series", "assigned_to_user_id").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN assigned_to_user_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        if !column_exists(conn, "item_series", "points").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN points INTEGER")
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
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn old_schema_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE item_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL,
                item_type TEXT NOT NULL DEFAULT 'EVENT',
                cursor_date INTEGER,
                basis TEXT,
                template_item_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn adds_assignment_columns_when_missing() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(!column_exists(&mut conn, "item_series", "assigned_to_user_id").await.unwrap());
        assert!(!column_exists(&mut conn, "item_series", "points").await.unwrap());

        AddItemSeriesAssignment.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "item_series", "assigned_to_user_id").await.unwrap());
        assert!(column_exists(&mut conn, "item_series", "points").await.unwrap());
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesAssignment.up(&mut conn).await.unwrap();
        AddItemSeriesAssignment.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "item_series", "assigned_to_user_id").await.unwrap());
        assert!(column_exists(&mut conn, "item_series", "points").await.unwrap());
    }
}
