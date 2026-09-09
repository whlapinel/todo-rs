use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Stage 3 of the templates-assignment work: adds `items.source_template_id` (the root
/// `TemplateItem` whose event-trigger/"Use" instantiation produced a task — see
/// `Item::source_template_id`) and `template_rotation_members` (mirrors
/// `item_series_rotation_members` exactly — root CLAUDE.md's Assignment rotation
/// section). Both already exist in `create_pool()`'s baseline for a fresh DB, so this
/// only does real work against a DB that predates them.
pub struct AddItemTemplateAssignment;

#[async_trait]
impl Migration for AddItemTemplateAssignment {
    fn version(&self) -> i64 {
        37
    }

    fn name(&self) -> &str {
        "add items.source_template_id and template_rotation_members"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "items", "source_template_id").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN source_template_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS template_rotation_members (
                template_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                PRIMARY KEY (template_id, user_id)
            )",
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
    use sqlx::{Row, SqlitePool};
    use std::str::FromStr;

    async fn empty_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    async fn table_exists(conn: &mut SqliteConnection, table: &str) -> bool {
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_optional(&mut *conn)
            .await
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn adds_source_template_id_and_creates_rotation_table() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("CREATE TABLE items (id TEXT PRIMARY KEY)")
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            !column_exists(&mut conn, "items", "source_template_id")
                .await
                .unwrap()
        );
        assert!(!table_exists(&mut conn, "template_rotation_members").await);

        AddItemTemplateAssignment.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "items", "source_template_id")
                .await
                .unwrap()
        );
        assert!(table_exists(&mut conn, "template_rotation_members").await);
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("CREATE TABLE items (id TEXT PRIMARY KEY)")
            .execute(&mut *conn)
            .await
            .unwrap();

        AddItemTemplateAssignment.up(&mut conn).await.unwrap();
        AddItemTemplateAssignment.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "items", "source_template_id")
                .await
                .unwrap()
        );
        assert!(table_exists(&mut conn, "template_rotation_members").await);
    }

    #[tokio::test]
    async fn rotation_table_enforces_template_id_user_id_uniqueness() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("CREATE TABLE items (id TEXT PRIMARY KEY)")
            .execute(&mut *conn)
            .await
            .unwrap();
        AddItemTemplateAssignment.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO template_rotation_members (template_id, user_id) VALUES ('t1', 'u1')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let err = sqlx::query(
            "INSERT INTO template_rotation_members (template_id, user_id) VALUES ('t1', 'u1')",
        )
        .execute(&mut *conn)
        .await
        .unwrap_err();
        assert!(err.to_string().contains("UNIQUE") || err.to_string().contains("constraint"));

        let row = sqlx::query("SELECT COUNT(*) as c FROM template_rotation_members")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let count: i64 = row.get("c");
        assert_eq!(count, 1);
    }
}
