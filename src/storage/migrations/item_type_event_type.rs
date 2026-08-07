use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Replaces the old `is_template: bool` flag with `item_type`/`event_type`
/// (see CLAUDE.md's Events section). `CREATE TABLE IF NOT EXISTS items`
/// already includes both columns, so this is a no-op against a fresh DB;
/// it only does work against a DB that predates `item_type`.
pub struct ItemTypeEventType;

#[async_trait]
impl Migration for ItemTypeEventType {
    fn version(&self) -> i64 {
        1
    }

    fn name(&self) -> &str {
        "replace is_template with item_type/event_type"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "item_type").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN item_type TEXT NOT NULL DEFAULT 'TASK'")
                .execute(&mut *conn)
                .await?;
        }
        if !column_exists(conn, "items", "event_type").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN event_type TEXT")
                .execute(&mut *conn)
                .await?;
        }
        if column_exists(conn, "items", "is_template").await? {
            sqlx::query("UPDATE items SET item_type = 'TEMPLATE' WHERE is_template = 1")
                .execute(&mut *conn)
                .await?;
            sqlx::query("ALTER TABLE items DROP COLUMN is_template")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
