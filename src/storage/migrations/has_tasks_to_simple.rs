use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Retires `has_tasks` in favor of `ItemType::Simple` (see CLAUDE.md's "Simple items"
/// discussion). `CREATE TABLE IF NOT EXISTS items` no longer defines `has_tasks`, so this
/// is a no-op against a fresh DB; it only does work against a DB that predates the switch.
pub struct HasTasksToSimple;

#[async_trait]
impl Migration for HasTasksToSimple {
    fn version(&self) -> i64 {
        3
    }

    fn name(&self) -> &str {
        "fold has_tasks into ItemType::Simple"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if column_exists(conn, "items", "has_tasks").await? {
            // Only rows still typed TASK — TEMPLATE rows (checklist children) also set
            // has_tasks = 0 today, but for unrelated reasons (see CLAUDE.md), and keep
            // their own bespoke field-shape rules rather than becoming Simple.
            sqlx::query(
                "UPDATE items SET item_type = 'SIMPLE' WHERE has_tasks = 0 AND item_type = 'TASK'",
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query("ALTER TABLE items DROP COLUMN has_tasks")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
