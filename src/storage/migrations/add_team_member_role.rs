use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `team_members.role`, the per-team admin/member distinction the points system
/// gates on (see CLAUDE.md's per-team roles design). `CREATE TABLE IF NOT EXISTS
/// team_members` already defines `role` for a fresh DB, so this is a no-op there; it
/// only does work against a DB that predates the column.
///
/// Backfill: every existing team predates any admin concept, so without this every
/// team would end up permanently adminless (no in-app path ever promotes a first
/// admin from `member`). Whoever created the team — the member row with
/// `invited_by IS NULL` — is promoted to `admin`. `TeamRepo::create` inserts new
/// teams' creators as `invited_by = NULL` too, so this backfill rule and the
/// forward-going behavior agree on what "creator" means.
pub struct AddTeamMemberRole;

#[async_trait]
impl Migration for AddTeamMemberRole {
    fn version(&self) -> i64 {
        4
    }

    fn name(&self) -> &str {
        "add team_members.role, backfill admins from invited_by IS NULL"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "team_members", "role").await? {
            sqlx::query("ALTER TABLE team_members ADD COLUMN role TEXT NOT NULL DEFAULT 'member'")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query(
            "UPDATE team_members SET role = 'admin' WHERE invited_by IS NULL AND role != 'admin'",
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn old_schema_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE team_members (
                team_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'PENDING',
                invited_by TEXT,
                PRIMARY KEY (team_id, user_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn backfills_creator_to_admin_and_leaves_invitees_as_member() {
        let pool = old_schema_pool().await;
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, invited_by) VALUES \
             ('t1', 'creator', 'ACTIVE', NULL), \
             ('t1', 'invitee', 'ACTIVE', 'creator')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        AddTeamMemberRole.up(&mut conn).await.unwrap();

        let creator_role: String = sqlx::query_scalar(
            "SELECT role FROM team_members WHERE team_id = 't1' AND user_id = 'creator'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(creator_role, "admin");

        let invitee_role: String = sqlx::query_scalar(
            "SELECT role FROM team_members WHERE team_id = 't1' AND user_id = 'invitee'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(invitee_role, "member");
    }

    #[tokio::test]
    async fn running_up_twice_is_idempotent() {
        let pool = old_schema_pool().await;
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, invited_by) VALUES ('t1', 'creator', 'ACTIVE', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        AddTeamMemberRole.up(&mut conn).await.unwrap();
        AddTeamMemberRole.up(&mut conn).await.unwrap();

        let role: String = sqlx::query_scalar(
            "SELECT role FROM team_members WHERE team_id = 't1' AND user_id = 'creator'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(role, "admin");
    }
}
