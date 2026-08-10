use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `description` to `items` — free-form notes text, applicable to every item kind
/// (Task/Event/Template/Simple). `CREATE TABLE IF NOT EXISTS items` already includes
/// this column, so this is a no-op against a fresh DB; it only does work against a DB
/// that predates it.
pub struct AddItemDescription;

#[async_trait]
impl Migration for AddItemDescription {
    fn version(&self) -> i64 {
        8
    }

    fn name(&self) -> &str {
        "add items.description"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "description").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN description TEXT")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
