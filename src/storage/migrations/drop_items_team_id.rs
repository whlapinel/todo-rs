use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Drops `items.team_id` — the final step of docs/team-id-removal-plan.md's Stage 6.
/// Genuinely dead by this point: `project_id` has been the sole scoping key for item
/// storage/business logic since that plan's Stage 4 (team items) and Stage 5
/// (team templates), and the last external read sites still keying off this column —
/// `list_items_due`'s/`list_assigned_items`'s `teamId`/`ownerUserId` fields, and the
/// legacy `ListTeamTemplates` JSON API operation — were repointed at `project_id`
/// (via each item's own `project_id` → `ProjectRepo` lookup) in that same stage. No
/// remaining `ItemRepo` method reads or writes this column.
///
/// `activity_log.team_id` is a separate, unrelated column on a different table — kept
/// permanently (see `drop_team_member_points.rs`'s own note on why), untouched here.
///
/// SQLite 3.35+ (this project's bundled `libsqlite3-sys`) supports real
/// `ALTER TABLE ... DROP COLUMN`. Guarded with `column_exists` — the inverse of every
/// prior migration's ADD-COLUMN guard, but the same reasoning: SQLite has no
/// `DROP COLUMN IF EXISTS`, and this must be safe to run against a DB that's already
/// missing the column (a fresh DB, whose baseline `CREATE TABLE IF NOT EXISTS items`
/// no longer declares it).
pub struct DropItemsTeamId;

#[async_trait]
impl Migration for DropItemsTeamId {
    fn version(&self) -> i64 {
        14
    }

    fn name(&self) -> &str {
        "drop items.team_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if column_exists(conn, "items", "team_id").await? {
            sqlx::query("ALTER TABLE items DROP COLUMN team_id")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::{Row, SqlitePool};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    #[tokio::test]
    async fn drops_team_id_column_when_present() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                team_id TEXT,
                project_id TEXT,
                name TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO items (id, user_id, team_id, project_id, name) \
             VALUES ('i1', 'u1', 't1', 'p1', 'Task')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        DropItemsTeamId.up(&mut conn).await.unwrap();

        assert!(!column_exists(&mut conn, "items", "team_id").await.unwrap());
        let name: String = sqlx::query("SELECT name FROM items WHERE id = 'i1'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("name");
        assert_eq!(name, "Task");
    }

    #[tokio::test]
    async fn is_idempotent_against_a_table_already_missing_team_id() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                project_id TEXT,
                name TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        DropItemsTeamId.up(&mut conn).await.unwrap();
        DropItemsTeamId.up(&mut conn).await.unwrap();

        assert!(!column_exists(&mut conn, "items", "team_id").await.unwrap());
    }
}
