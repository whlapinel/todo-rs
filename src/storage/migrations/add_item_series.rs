use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Stage 7a of docs/recurring-events-virtual-occurrences-rough-plan.md's "generalizing
/// series to Tasks" scope change: renames `event_series`/`event_occurrences` to
/// `item_series`/`item_occurrences`, adding `item_type` (restricted to `TASK`/`EVENT`
/// at the service layer, stage 7b) so a series can back either kind, not just Events.
///
/// The old tables are left in place, unread from here on — matches this codebase's
/// precedent of not force-dropping superseded schema (see CLAUDE.md's Storage Layer
/// section on `items.user_id` surviving the Project abstraction). Every pre-existing
/// row is carried forward column-for-column with `item_type = 'EVENT'`, the only kind
/// that existed before this migration. `create_pool()` defines the identical baseline
/// (including `item_type`) for a fresh DB, so this only does real work — table creation
/// and the data copy — against a DB that predates it; on a fresh DB the tables already
/// exist and `event_series`/`event_occurrences` are empty, so the copy is a no-op.
pub struct AddItemSeries;

#[async_trait]
impl Migration for AddItemSeries {
    fn version(&self) -> i64 {
        16
    }

    fn name(&self) -> &str {
        "add item_series, item_occurrences (rename of event_series/event_occurrences), migrate data"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS item_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL,
                item_type TEXT NOT NULL DEFAULT 'EVENT'
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_item_series_project_id ON item_series (project_id)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS item_occurrences (
                series_id TEXT NOT NULL,
                occurrence_date INTEGER NOT NULL,
                item_id TEXT,
                is_exdate INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (series_id, occurrence_date)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_item_occurrences_item_id ON item_occurrences (item_id)",
        )
        .execute(&mut *conn)
        .await?;

        // Idempotent (guarded by `WHERE ... NOT IN (...)`) so re-running `up()` — as this
        // migration's own idempotency test does — never violates item_series' primary key.
        sqlx::query(
            "INSERT INTO item_series (id, project_id, name, description, event_type, recurrence, anchor_date, item_type) \
             SELECT id, project_id, name, description, event_type, recurrence, anchor_date, 'EVENT' \
             FROM event_series WHERE id NOT IN (SELECT id FROM item_series)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO item_occurrences (series_id, occurrence_date, item_id, is_exdate) \
             SELECT series_id, occurrence_date, item_id, is_exdate FROM event_occurrences \
             WHERE (series_id, occurrence_date) NOT IN (SELECT series_id, occurrence_date FROM item_occurrences)",
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

    async fn pool_with_event_series_data() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE event_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE event_occurrences (
                series_id TEXT NOT NULL,
                occurrence_date INTEGER NOT NULL,
                item_id TEXT,
                is_exdate INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (series_id, occurrence_date)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO event_series (id, project_id, name, description, event_type, recurrence, anchor_date) \
             VALUES ('s1', 'p1', 'Standup', 'Daily sync', 'meeting', 'every weekday', 1000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO event_occurrences (series_id, occurrence_date, item_id, is_exdate) \
             VALUES ('s1', 1000000, 'item-1', 0), ('s1', 1086400, NULL, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn creates_tables_and_migrates_data_column_for_column() {
        let pool = pool_with_event_series_data().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeries.up(&mut conn).await.unwrap();

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series_count, 1);

        let (name, item_type): (String, String) =
            sqlx::query_as("SELECT name, item_type FROM item_series WHERE id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(name, "Standup");
        assert_eq!(item_type, "EVENT");

        let occurrence_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_occurrences")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(occurrence_count, 2);
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = pool_with_event_series_data().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeries.up(&mut conn).await.unwrap();
        AddItemSeries.up(&mut conn).await.unwrap();

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series_count, 1);

        let occurrence_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_occurrences")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(occurrence_count, 2);
    }

    #[tokio::test]
    async fn no_op_against_a_fresh_db_where_event_series_is_already_empty() {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        // Fresh-DB baseline: both old and new tables already exist and are empty.
        sqlx::query(
            "CREATE TABLE event_series (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL,
                description TEXT, event_type TEXT, recurrence TEXT NOT NULL, anchor_date INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE event_occurrences (
                series_id TEXT NOT NULL, occurrence_date INTEGER NOT NULL, item_id TEXT,
                is_exdate INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (series_id, occurrence_date)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE item_series (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL,
                description TEXT, event_type TEXT, recurrence TEXT NOT NULL, anchor_date INTEGER NOT NULL,
                item_type TEXT NOT NULL DEFAULT 'EVENT'
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE item_occurrences (
                series_id TEXT NOT NULL, occurrence_date INTEGER NOT NULL, item_id TEXT,
                is_exdate INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (series_id, occurrence_date)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        AddItemSeries.up(&mut conn).await.unwrap();

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series_count, 0);
    }
}
