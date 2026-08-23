use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `personal_project_id` to `users` — see `service::projects::ensure_default_project`
/// and `docs/dialog-item-forms-plan.md`'s Stage 0. `CREATE TABLE IF NOT EXISTS users`
/// already includes this column, so this is a no-op against a fresh DB; it only does
/// work against a DB that predates it. Existing users are backfilled lazily by
/// `ensure_default_project` on next login rather than by this migration, since that
/// already has to run `find_personal_project`'s heuristic per user and this migration
/// system has no precedent for a cross-table backfill this involved outside
/// `backfill_projects.rs` (which ran once, right when `projects` itself was introduced).
pub struct AddUserPersonalProjectId;

#[async_trait]
impl Migration for AddUserPersonalProjectId {
    fn version(&self) -> i64 {
        27
    }

    fn name(&self) -> &str {
        "add users.personal_project_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "users", "personal_project_id").await? {
            sqlx::query("ALTER TABLE users ADD COLUMN personal_project_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
