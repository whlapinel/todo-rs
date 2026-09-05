use super::Migration;
use crate::storage::migrations::MigrationError;
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Adds `item_series_children` (the per-series sub-item *definitions*) and
/// `series_child_occurrences` (their per-cycle materialization state). New tables, not
/// `ALTER TABLE`s, so this needs no `column_exists` guard — `CREATE TABLE IF NOT EXISTS`
/// is naturally idempotent, and `create_pool()`'s baseline already includes both for a
/// fresh DB, so this only does real work against a DB that predates them. Same shape as
/// `AddItemSeriesRotationMembers`.
///
/// `series_child_occurrences` deliberately mirrors `item_occurrences` — including the
/// `item_id` index backing the reverse lookup — with one deliberate omission: no
/// `is_exdate`. A series sub-item has no Skip action, so it has only two states (virtual =
/// no row, materialized = a row), which is also why `item_id` is `NOT NULL` here where
/// `item_occurrences.item_id` is nullable. `occurrence_date` is the *parent* series' cycle
/// date, which is the stable identity; the sub-item's own due date is derived from it plus
/// the definition's `days_before` and is never the lookup key, since editing the offset
/// would otherwise invalidate existing rows.
pub struct AddItemSeriesChildren;

#[async_trait]
impl Migration for AddItemSeriesChildren {
    fn version(&self) -> i64 {
        35
    }

    fn name(&self) -> &str {
        "add item_series_children and series_child_occurrences"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS item_series_children (
                id TEXT PRIMARY KEY,
                series_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                days_before INTEGER NOT NULL DEFAULT 0,
                priority INTEGER,
                sort_order INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_item_series_children_series_id \
             ON item_series_children (series_id)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS series_child_occurrences (
                child_id TEXT NOT NULL,
                occurrence_date INTEGER NOT NULL,
                item_id TEXT NOT NULL,
                PRIMARY KEY (child_id, occurrence_date)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_series_child_occurrences_item_id \
             ON series_child_occurrences (item_id)",
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

    async fn index_exists(conn: &mut SqliteConnection, index: &str) -> bool {
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'index' AND name = ?")
            .bind(index)
            .fetch_optional(&mut *conn)
            .await
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn creates_both_tables_when_missing() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(!table_exists(&mut conn, "item_series_children").await);
        assert!(!table_exists(&mut conn, "series_child_occurrences").await);

        AddItemSeriesChildren.up(&mut conn).await.unwrap();

        assert!(table_exists(&mut conn, "item_series_children").await);
        assert!(table_exists(&mut conn, "series_child_occurrences").await);
        assert!(index_exists(&mut conn, "idx_item_series_children_series_id").await);
        assert!(index_exists(&mut conn, "idx_series_child_occurrences_item_id").await);
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesChildren.up(&mut conn).await.unwrap();
        AddItemSeriesChildren.up(&mut conn).await.unwrap();

        assert!(table_exists(&mut conn, "item_series_children").await);
        assert!(table_exists(&mut conn, "series_child_occurrences").await);
    }

    #[tokio::test]
    async fn child_occurrences_are_unique_per_child_and_cycle_date() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        AddItemSeriesChildren.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO series_child_occurrences (child_id, occurrence_date, item_id) \
             VALUES ('c1', 100, 'i1')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let err = sqlx::query(
            "INSERT INTO series_child_occurrences (child_id, occurrence_date, item_id) \
             VALUES ('c1', 100, 'i2')",
        )
        .execute(&mut *conn)
        .await
        .unwrap_err();
        assert!(err.to_string().contains("UNIQUE") || err.to_string().contains("constraint"));

        let row = sqlx::query("SELECT COUNT(*) as c FROM series_child_occurrences")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let count: i64 = row.get("c");
        assert_eq!(count, 1);
    }

    /// The same child definition materializes independently for each parent cycle, so the
    /// same `child_id` at a different `occurrence_date` must be allowed.
    #[tokio::test]
    async fn one_child_can_materialize_for_multiple_cycles() {
        let pool = empty_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        AddItemSeriesChildren.up(&mut conn).await.unwrap();

        for (date, item) in [(100, "i1"), (200, "i2")] {
            sqlx::query(
                "INSERT INTO series_child_occurrences (child_id, occurrence_date, item_id) \
                 VALUES ('c1', ?, ?)",
            )
            .bind(date)
            .bind(item)
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        let row = sqlx::query("SELECT COUNT(*) as c FROM series_child_occurrences")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let count: i64 = row.get("c");
        assert_eq!(count, 2);
    }
}
