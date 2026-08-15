use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Stage 10 gap 1 of docs/recurring-events-virtual-occurrences-rough-plan.md: adds the
/// completion-vs-schedule basis flag a Task-typed `item_series` can carry — see
/// `domain::item_series::ItemSeries::basis`. `CREATE TABLE IF NOT EXISTS item_series`
/// already includes this column, so this is a no-op against a fresh DB; it only does
/// work against a DB that predates it.
pub struct AddItemSeriesBasis;

#[async_trait]
impl Migration for AddItemSeriesBasis {
    fn version(&self) -> i64 {
        18
    }

    fn name(&self) -> &str {
        "add basis to item_series"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "item_series", "basis").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN basis TEXT")
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
                cursor_date INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn adds_basis_column_when_missing() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(!column_exists(&mut conn, "item_series", "basis").await.unwrap());

        AddItemSeriesBasis.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "item_series", "basis").await.unwrap());
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesBasis.up(&mut conn).await.unwrap();
        AddItemSeriesBasis.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "item_series", "basis").await.unwrap());
    }
}
