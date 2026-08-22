use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `timezone` to `users` — see root CLAUDE.md's Google Calendar import notes and
/// `service::calendar_sync`. `CREATE TABLE IF NOT EXISTS users` already includes this
/// column, so this is a no-op against a fresh DB; it only does work against a DB that
/// predates it.
pub struct AddUserTimezone;

#[async_trait]
impl Migration for AddUserTimezone {
    fn version(&self) -> i64 {
        26
    }

    fn name(&self) -> &str {
        "add users.timezone"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "users", "timezone").await? {
            sqlx::query("ALTER TABLE users ADD COLUMN timezone TEXT")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
