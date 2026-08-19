use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `series_id` to `items` (see CLAUDE.md's Domain Models section and
/// `docs/unify-virtual-materialized-occurrences-plan.md` Stage A). Set once, at
/// creation, only by `service::item_series::get_or_materialize_occurrence` — no
/// Smithy field, CLI flag, or MCP parameter can ever set it. `CREATE TABLE IF NOT
/// EXISTS items` already includes this column, so this is a no-op against a fresh
/// DB; it only does work against a DB that predates it. Also adds
/// `idx_items_series_id`, created here (not just in the baseline) since a DB that
/// predates this migration has neither the column nor the index yet.
pub struct AddItemSeriesId;

#[async_trait]
impl Migration for AddItemSeriesId {
    fn version(&self) -> i64 {
        23
    }

    fn name(&self) -> &str {
        "add items.series_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "series_id").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN series_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_series_id ON items (series_id)")
            .execute(&mut *conn)
            .await?;
        Ok(())
    }
}
