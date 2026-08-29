use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `priority` to `items` and `item_series` (see root CLAUDE.md's Priority
/// section). Unlike `points`/`assigned_to_user_id`, this is a plain,
/// personal-productivity field with no team/admin gating. `CREATE TABLE IF NOT
/// EXISTS` for both tables already includes this column, so this is a no-op
/// against a fresh DB; it only does work against a DB that predates it.
pub struct AddItemPriority;

#[async_trait]
impl Migration for AddItemPriority {
    fn version(&self) -> i64 {
        31
    }

    fn name(&self) -> &str {
        "add priority to items/item_series"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "priority").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN priority INTEGER")
                .execute(&mut *conn)
                .await?;
        }
        if !column_exists(conn, "item_series", "priority").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN priority INTEGER")
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
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE item_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn adds_priority_columns_when_missing() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(!column_exists(&mut conn, "items", "priority").await.unwrap());
        assert!(
            !column_exists(&mut conn, "item_series", "priority")
                .await
                .unwrap()
        );

        AddItemPriority.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "items", "priority").await.unwrap());
        assert!(
            column_exists(&mut conn, "item_series", "priority")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemPriority.up(&mut conn).await.unwrap();
        AddItemPriority.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "items", "priority").await.unwrap());
        assert!(
            column_exists(&mut conn, "item_series", "priority")
                .await
                .unwrap()
        );
    }
}
