mod item_type_event_type;
mod scheduled_end_date;

use async_trait::async_trait;
use item_type_event_type::ItemTypeEventType;
use scheduled_end_date::ScheduledEndDate;
use sqlx::{Row, SqlitePool, SqliteConnection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait]
trait Migration: Send + Sync {
    fn version(&self) -> i64;
    fn name(&self) -> &str;
    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError>;
}

/// Whether `table` already has `column` — SQLite's `ALTER TABLE ... ADD
/// COLUMN`/`DROP COLUMN` have no `IF NOT EXISTS` form, so migrations that
/// alter an existing table must check first. `table`/`column` are always
/// internal literals from call sites, never user input.
async fn column_exists(
    conn: &mut SqliteConnection,
    table: &str,
    column: &str,
) -> Result<bool, MigrationError> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column))
}

fn all_migrations() -> Vec<Box<dyn Migration>> {
    vec![Box::new(ItemTypeEventType), Box::new(ScheduledEndDate)]
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), MigrationError> {
    let mut conn = pool.acquire().await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(&mut *conn)
    .await?;

    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _migrations")
        .fetch_all(&mut *conn)
        .await?;

    let mut migrations = all_migrations();
    migrations.sort_by_key(|m| m.version());

    for m in migrations {
        if applied.contains(&m.version()) {
            continue;
        }

        let mut tx = pool.begin().await?;
        m.up(&mut tx).await?;
        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?, ?)")
            .bind(m.version())
            .bind(m.name())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        tracing::info!(version = m.version(), name = m.name(), "applied migration");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    // `run_migrations` holds an acquired connection for its own setup while
    // also opening a second one per migration transaction, so the pool
    // needs room for >1 connection at once. Shared-cache mode keeps every
    // pooled connection pointed at the same in-memory database rather than
    // each getting its own isolated one.
    async fn memory_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    async fn old_schema_pool() -> SqlitePool {
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                due_date INTEGER,
                is_template INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn current_schema_pool() -> SqlitePool {
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                due_date INTEGER,
                scheduled_date INTEGER,
                scheduled_end_date INTEGER,
                has_scheduled_time INTEGER NOT NULL DEFAULT 0,
                has_end_time INTEGER NOT NULL DEFAULT 0,
                item_type TEXT NOT NULL DEFAULT 'TASK',
                event_type TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn migrates_old_schema_and_backfills_item_type() {
        let pool = old_schema_pool().await;
        sqlx::query("INSERT INTO items (id, name, is_template) VALUES ('1', 'tmpl', 1)")
            .execute(&pool)
            .await
            .unwrap();

        run_migrations(&pool).await.unwrap();

        let item_type: String = sqlx::query_scalar("SELECT item_type FROM items WHERE id = '1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(item_type, "TEMPLATE");

        let mut conn = pool.acquire().await.unwrap();
        assert!(!column_exists(&mut conn, "items", "is_template").await.unwrap());
        assert!(column_exists(&mut conn, "items", "scheduled_end_date").await.unwrap());
        assert!(column_exists(&mut conn, "items", "has_scheduled_time").await.unwrap());
        assert!(column_exists(&mut conn, "items", "has_end_time").await.unwrap());

        let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied_count, 2);
    }

    #[tokio::test]
    async fn is_a_noop_against_a_db_already_on_the_current_schema() {
        let pool = current_schema_pool().await;

        run_migrations(&pool).await.unwrap();

        let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied_count, 2);
    }

    #[tokio::test]
    async fn running_twice_is_idempotent() {
        let pool = old_schema_pool().await;

        run_migrations(&pool).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied_count, 2);
    }
}
