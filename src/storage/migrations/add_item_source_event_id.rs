use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `source_event_id` to `items` — a task's reference to the Event it tracks (see
/// `Item::source_event_id`), replacing the old `parent_item_id`-nesting an Event's
/// auto-copied/manually-linked children used before Events were disallowed from having
/// structural children. `CREATE TABLE IF NOT EXISTS items` already includes this column,
/// so this is a no-op against a fresh DB; it only does work against a DB that predates it.
///
/// Also backfills every pre-existing Event-nested child (both template-triggered and
/// manually added via the old "add sub-item" form) into a `sourceEventId`-referenced
/// top-level task: `source_event_id` takes over the old `parent_item_id` value, and
/// `parent_item_id` is cleared. `due_date` is left untouched — it was already
/// offset-derived against the event's own anchor at creation time, so re-pointing the
/// reference doesn't require recomputing it. Idempotent: once `parent_item_id` is NULL,
/// re-running the `UPDATE` matches nothing.
pub struct AddItemSourceEventId;

#[async_trait]
impl Migration for AddItemSourceEventId {
    fn version(&self) -> i64 {
        9
    }

    fn name(&self) -> &str {
        "add items.source_event_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "source_event_id").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN source_event_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        // `parent_item_id` predates the migration system itself (part of the original
        // hand-written base schema), so every real DB has it — this guard only matters
        // for a hypothetical DB that somehow doesn't, matching this file's own
        // `column_exists`-guard convention for every `ALTER TABLE` above.
        if column_exists(conn, "items", "parent_item_id").await? {
            sqlx::query(
                "UPDATE items SET source_event_id = parent_item_id, parent_item_id = NULL \
                 WHERE parent_item_id IN (SELECT id FROM items WHERE item_type = 'EVENT')",
            )
            .execute(&mut *conn)
            .await?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_items_source_event_id ON items (source_event_id)",
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}
