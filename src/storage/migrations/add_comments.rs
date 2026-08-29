use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `comments` table — "Add comments for tasks"
/// (docs/issues_and_features.md): schema only, see `service::comments`, the sole writer.
/// Brand-new table, so (like `AddReminders`/`AddItemDependencies`) `CREATE TABLE IF NOT
/// EXISTS` alone is enough, no `column_exists` dance.
pub struct AddComments;

#[async_trait]
impl Migration for AddComments {
    fn version(&self) -> i64 {
        32
    }

    fn name(&self) -> &str {
        "add comments table"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS comments (
                id TEXT PRIMARY KEY,
                item_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                author_user_id TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_comments_item_id ON comments (item_id)")
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
    async fn creates_comments_table() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        AddComments.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO comments (id, item_id, project_id, author_user_id, body, created_at) \
             VALUES ('c1', 'i1', 'p1', 'u1', 'hello', 0)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn is_idempotent() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        AddComments.up(&mut conn).await.unwrap();
        AddComments.up(&mut conn).await.unwrap();
    }
}
