use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::{Row, SqliteConnection};

/// Relaxes `activity_log.team_id` from `NOT NULL` to nullable, so a personal (non-team)
/// project's item completions can be logged too — see CLAUDE.md's Points section and
/// docs/issues.md's "unify completion-undo" note. SQLite has no
/// `ALTER TABLE ... ALTER COLUMN` to drop a constraint, so this uses the standard
/// rebuild-and-copy shape: a fresh `activity_log_new` with the relaxed schema, copy
/// every row across, drop the old table, rename the new one into place, then recreate
/// every index that lived on the dropped table (indexes don't survive a `DROP TABLE`).
///
/// Purely relaxing, not destructive: every existing row's `team_id` is preserved as-is
/// (still `NOT NULL` in practice for every row written before this migration), and the
/// legacy team-scoped `ListTeamActivityLog`/`UndoActivityLogEntry` ops are unaffected —
/// they already filter `WHERE team_id = ?`, so new personal-project rows (`team_id:
/// NULL`) simply never surface there, which is correct (those ops are inherently
/// team-scoped; a personal project never had a team to query by anyway).
pub struct ActivityLogTeamIdNullable;

async fn team_id_is_not_null(conn: &mut SqliteConnection) -> Result<bool, MigrationError> {
    let rows = sqlx::query("PRAGMA table_info(activity_log)")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.iter().any(|row| {
        row.get::<String, _>("name") == "team_id" && row.get::<i64, _>("notnull") != 0
    }))
}

#[async_trait]
impl Migration for ActivityLogTeamIdNullable {
    fn version(&self) -> i64 {
        22
    }

    fn name(&self) -> &str {
        "make activity_log.team_id nullable"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !team_id_is_not_null(conn).await? {
            return Ok(());
        }

        sqlx::query(
            "CREATE TABLE activity_log_new (
                id TEXT PRIMARY KEY,
                team_id TEXT,
                user_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                item_name TEXT NOT NULL,
                points_delta INTEGER NOT NULL,
                reversed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                project_id TEXT
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO activity_log_new (id, team_id, user_id, item_id, item_name, points_delta, reversed, created_at, project_id) \
             SELECT id, team_id, user_id, item_id, item_name, points_delta, reversed, created_at, project_id FROM activity_log",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("DROP TABLE activity_log")
            .execute(&mut *conn)
            .await?;
        sqlx::query("ALTER TABLE activity_log_new RENAME TO activity_log")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_activity_log_team_created ON activity_log (team_id, created_at DESC)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_activity_log_item_id ON activity_log (item_id)")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_activity_log_project_id ON activity_log (project_id)",
        )
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

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    async fn create_old_schema(pool: &SqlitePool) {
        sqlx::query(
            "CREATE TABLE activity_log (
                id TEXT PRIMARY KEY,
                team_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                item_name TEXT NOT NULL,
                points_delta INTEGER NOT NULL,
                reversed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                project_id TEXT
            )",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn relaxes_team_id_to_nullable_and_preserves_existing_rows() {
        let pool = test_pool().await;
        create_old_schema(&pool).await;
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, reversed, created_at, project_id) \
             VALUES ('e1', 't1', 'u1', 'i1', 'Mow the lawn', 20, 0, 1000, 'p1')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        ActivityLogTeamIdNullable.up(&mut conn).await.unwrap();

        assert!(!team_id_is_not_null(&mut conn).await.unwrap());
        let row = sqlx::query("SELECT team_id, points_delta FROM activity_log WHERE id = 'e1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("team_id"), "t1");
        assert_eq!(row.get::<i64, _>("points_delta"), 20);

        // The relaxed table now actually accepts a NULL team_id (personal-project row).
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, reversed, created_at, project_id) \
             VALUES ('e2', NULL, 'u1', 'i2', 'Buy milk', 0, 0, 2000, 'p2')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn is_idempotent_against_a_table_already_nullable() {
        let pool = test_pool().await;
        create_old_schema(&pool).await;

        let mut conn = pool.acquire().await.unwrap();
        ActivityLogTeamIdNullable.up(&mut conn).await.unwrap();
        ActivityLogTeamIdNullable.up(&mut conn).await.unwrap();

        assert!(!team_id_is_not_null(&mut conn).await.unwrap());
    }
}
