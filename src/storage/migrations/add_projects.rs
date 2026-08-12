use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `projects`/`project_members` tables and adds `items.project_id` — pure
/// schema, nothing reads or writes any of this yet (see the Project-abstraction plan's
/// stage A1). `projects`/`project_members` are brand-new tables, so (like
/// `ActivityLog`) `CREATE TABLE IF NOT EXISTS` alone is enough, no `column_exists`
/// dance; `items.project_id` is a column on an existing table, so it needs the usual
/// guard. `create_pool()` defines the identical baseline for a fresh DB, so this only
/// does real work against a DB that predates it.
///
/// `idx_items_project_id` is created here, not in `create_pool()`'s baseline — per
/// CLAUDE.md's index-ordering note (we previously shipped a baseline index on a
/// column that only a later migration added, which broke on any DB that predated
/// that migration, since baseline indexes run before `run_migrations()`). Indexes on
/// the two brand-new tables are safe in the baseline (see `idx_projects_team_id`/
/// `idx_project_members_user_id` there) since the whole table — columns and all — is
/// created together either way; only an index on an *added column of an existing
/// table* has to live inside the migration that adds the column.
pub struct AddProjects;

#[async_trait]
impl Migration for AddProjects {
    fn version(&self) -> i64 {
        10
    }

    fn name(&self) -> &str {
        "add projects, project_members, items.project_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                owner_user_id TEXT NOT NULL,
                team_id TEXT
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS project_members (
                project_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'member',
                points INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (project_id, user_id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        if !column_exists(conn, "items", "project_id").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN project_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_project_id ON items (project_id)")
            .execute(&mut *conn)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn empty_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    #[tokio::test]
    async fn creates_tables_and_is_idempotent() {
        let pool = empty_pool().await;
        sqlx::query("CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        AddProjects.up(&mut conn).await.unwrap();
        AddProjects.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO projects (id, name, owner_user_id) VALUES ('p1', 'Home', 'u1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role, points) \
             VALUES ('p1', 'u1', 'admin', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE items SET project_id = 'p1' WHERE id = 'i1'")
            .execute(&pool)
            .await
            .unwrap();

        let project_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(project_count, 1);

        let member_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_members")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(member_count, 1);

        assert!(column_exists(&mut conn, "items", "project_id").await.unwrap());
    }
}
