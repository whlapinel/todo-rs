use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Guards against the same ordering bug `AddItemSeriesId`'s doc comment describes for
/// `items.series_id` ("this bit us once already for source_event_id") — `AddAttachments`
/// (migration 33) assumed `attachments` was a genuinely brand-new table, so it created it
/// (and its `comment_id` column/index) via a bare `CREATE TABLE IF NOT EXISTS`, with no
/// `column_exists` guard. That assumption broke in practice: a prod DB ended up with an
/// `attachments` table that predates `comment_id` being part of its shape, so every
/// startup's baseline `CREATE INDEX idx_attachments_comment_id ON attachments (comment_id)`
/// (`create_pool()`, `src/storage/sqlite/mod.rs`) failed with "no such column: comment_id" —
/// `CREATE TABLE IF NOT EXISTS` is a permanent no-op against a table that already exists in
/// any shape, so nothing before this migration could ever have added the column. Empty
/// string default is safe here: this table's only real consumer
/// (`service::attachments::upload_attachment`) always writes a real `comment_id`, so any
/// row with the empty-string default is either a genuinely pre-`comment_id` artifact (there
/// should be none, since attachments always belong to a comment) or from local test setup.
pub struct EnsureAttachmentsCommentId;

#[async_trait]
impl Migration for EnsureAttachmentsCommentId {
    fn version(&self) -> i64 {
        34
    }

    fn name(&self) -> &str {
        "ensure attachments.comment_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "attachments", "comment_id").await? {
            sqlx::query("ALTER TABLE attachments ADD COLUMN comment_id TEXT NOT NULL DEFAULT ''")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_attachments_comment_id ON attachments (comment_id)",
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
    async fn adds_comment_id_to_a_pre_existing_attachments_table() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        // Simulate the prod shape: attachments table already exists, without comment_id.
        sqlx::query(
            "CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
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
        .await
        .unwrap();

        EnsureAttachmentsCommentId.up(&mut conn).await.unwrap();

        sqlx::query("SELECT comment_id FROM attachments")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn is_idempotent() {
        let pool = memory_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TABLE attachments (
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
        .await
        .unwrap();

        EnsureAttachmentsCommentId.up(&mut conn).await.unwrap();
        EnsureAttachmentsCommentId.up(&mut conn).await.unwrap();
    }
}
