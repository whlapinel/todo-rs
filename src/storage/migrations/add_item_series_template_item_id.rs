use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Stage 10 gap 3 of docs/recurring-events-virtual-occurrences-rough-plan.md: adds the
/// optional link from a Task-typed `item_series` to a `Template` item whose children get
/// copied onto every materialized occurrence — see
/// `domain::item_series::ItemSeries::template_item_id`. `CREATE TABLE IF NOT EXISTS
/// item_series` already includes this column, so this is a no-op against a fresh DB; it
/// only does work against a DB that predates it.
pub struct AddItemSeriesTemplateItemId;

#[async_trait]
impl Migration for AddItemSeriesTemplateItemId {
    fn version(&self) -> i64 {
        19
    }

    fn name(&self) -> &str {
        "add template_item_id to item_series"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "item_series", "template_item_id").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN template_item_id TEXT")
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
                basis TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn adds_template_item_id_column_when_missing() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            !column_exists(&mut conn, "item_series", "template_item_id")
                .await
                .unwrap()
        );

        AddItemSeriesTemplateItemId.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "item_series", "template_item_id")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesTemplateItemId.up(&mut conn).await.unwrap();
        AddItemSeriesTemplateItemId.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "item_series", "template_item_id")
                .await
                .unwrap()
        );
    }
}
