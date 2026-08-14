use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Stage 9 of docs/recurring-events-virtual-occurrences-rough-plan.md: adds the
/// forward-only cursor a Task-typed `item_series` uses to track its "current"
/// (possibly backlogged) occurrence — see `domain::item_series::ItemSeries::cursor_date`.
/// `CREATE TABLE IF NOT EXISTS item_series` already includes this column, so this is a
/// no-op against a fresh DB; it only does work against a DB that predates it.
pub struct AddItemSeriesCursorDate;

#[async_trait]
impl Migration for AddItemSeriesCursorDate {
    fn version(&self) -> i64 {
        17
    }

    fn name(&self) -> &str {
        "add cursor_date to item_series"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "item_series", "cursor_date").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN cursor_date INTEGER")
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
                item_type TEXT NOT NULL DEFAULT 'EVENT'
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn adds_cursor_date_column_when_missing() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(!column_exists(&mut conn, "item_series", "cursor_date").await.unwrap());

        AddItemSeriesCursorDate.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "item_series", "cursor_date").await.unwrap());
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesCursorDate.up(&mut conn).await.unwrap();
        AddItemSeriesCursorDate.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "item_series", "cursor_date").await.unwrap());
    }
}
