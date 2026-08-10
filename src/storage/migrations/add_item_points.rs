use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `points` to `items` (see CLAUDE.md's Points/Roles plan, Stage 2). Only ever
/// populated on top-level team items by a team admin — personal items and children
/// simply never set it. `CREATE TABLE IF NOT EXISTS items` already includes this
/// column, so this is a no-op against a fresh DB; it only does work against a DB
/// that predates it.
pub struct AddItemPoints;

#[async_trait]
impl Migration for AddItemPoints {
    fn version(&self) -> i64 {
        5
    }

    fn name(&self) -> &str {
        "add items.points"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "points").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN points INTEGER")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
