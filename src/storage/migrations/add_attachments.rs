use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `attachments` table — root CLAUDE.md's Attachments section: schema only,
/// see `storage::sqlite::AttachmentRepo`, the sole writer. Originally assumed (like
/// `AddComments`) that `CREATE TABLE IF NOT EXISTS` alone was enough, no `column_exists`
/// dance needed since the table was brand-new — that assumption broke in practice (see
/// `EnsureAttachmentsCommentId`'s doc comment), so `idx_attachments_comment_id` no longer
/// lives here; `EnsureAttachmentsCommentId` (the migration that actually guarantees the
/// column exists first) creates it instead.
pub struct AddAttachments;

#[async_trait]
impl Migration for AddAttachments {
    fn version(&self) -> i64 {
        33
    }

    fn name(&self) -> &str {
        "add attachments table"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS attachments (
                id TEXT PRIMARY KEY,
                comment_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                uploaded_by_user_id TEXT NOT NULL,
                filename TEXT NOT NULL,
                content_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                storage_key TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_attachments_item_id ON attachments (item_id)")
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
    async fn creates_attachments_table() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        AddAttachments.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO attachments (id, comment_id, item_id, project_id, uploaded_by_user_id, \
             filename, content_type, size_bytes, storage_key, created_at) \
             VALUES ('a1', 'c1', 'i1', 'p1', 'u1', 'photo.jpg', 'image/jpeg', 100, 'p1/i1/a1_photo.jpg', 0)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn is_idempotent() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        AddAttachments.up(&mut conn).await.unwrap();
        AddAttachments.up(&mut conn).await.unwrap();
    }
}
