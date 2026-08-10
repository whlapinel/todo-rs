use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `team_members.points`, the per-team point balance the points economy (see
/// CLAUDE.md's per-team roles/points design, Stage 6) awards to and reverses from.
/// `CREATE TABLE IF NOT EXISTS team_members` already includes this column, so this is
/// a no-op against a fresh DB; it only does work against a DB that predates it.
/// Defaults to 0 so pre-existing members simply start with no points banked, not NULL.
pub struct TeamMemberPoints;

#[async_trait]
impl Migration for TeamMemberPoints {
    fn version(&self) -> i64 {
        7
    }

    fn name(&self) -> &str {
        "add team_members.points"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "team_members", "points").await? {
            sqlx::query("ALTER TABLE team_members ADD COLUMN points INTEGER NOT NULL DEFAULT 0")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
