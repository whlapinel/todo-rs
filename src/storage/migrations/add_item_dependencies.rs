use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `item_dependencies` table — "depends on"
/// (docs/issues_and_features.md): schema only, see `service::item_dependencies`, the sole
/// writer. Brand-new table, so (like `AddReminders`) `CREATE TABLE IF NOT EXISTS` alone is
/// enough, no `column_exists` dance.
pub struct AddItemDependencies;

#[async_trait]
impl Migration for AddItemDependencies {
    fn version(&self) -> i64 {
        29
    }

    fn name(&self) -> &str {
        "add item_dependencies table"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS item_dependencies (
                item_id TEXT NOT NULL,
                depends_on_item_id TEXT NOT NULL,
                PRIMARY KEY (item_id, depends_on_item_id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_item_dependencies_item_id \
             ON item_dependencies (item_id)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_item_dependencies_depends_on \
             ON item_dependencies (depends_on_item_id)",
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

        AddItemDependencies.up(&mut conn).await.unwrap();
        AddItemDependencies.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO item_dependencies (item_id, depends_on_item_id) VALUES ('i1', 'i2')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_dependencies")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
