use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `scheduled_end_date` and its accompanying `has_scheduled_time`/
/// `has_end_time` flags (see CLAUDE.md's Scheduled start/end section).
/// `CREATE TABLE IF NOT EXISTS items` already includes these columns, so
/// this is a no-op against a fresh DB; it only does work against a DB that
/// predates them.
pub struct ScheduledEndDate;

#[async_trait]
impl Migration for ScheduledEndDate {
    fn version(&self) -> i64 {
        2
    }

    fn name(&self) -> &str {
        "add scheduled_end_date and time flags"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "scheduled_end_date").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN scheduled_end_date INTEGER")
                .execute(&mut *conn)
                .await?;
        }
        if !column_exists(conn, "items", "has_scheduled_time").await? {
            sqlx::query(
                "ALTER TABLE items ADD COLUMN has_scheduled_time INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&mut *conn)
            .await?;
        }
        if !column_exists(conn, "items", "has_end_time").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN has_end_time INTEGER NOT NULL DEFAULT 0")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
