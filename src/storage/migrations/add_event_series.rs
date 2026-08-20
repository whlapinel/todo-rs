use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `event_series`/`event_occurrences` tables — pure schema, nothing
/// reads or writes any of this yet (see
/// docs/recurring-events-virtual-occurrences-rough-plan.md's staged breakdown,
/// stage 2). Both are brand-new tables, so (like `AddProjects`) `CREATE TABLE IF
/// NOT EXISTS` alone is enough, no `column_exists` dance. `create_pool()` defines
/// the identical baseline for a fresh DB, so this only does real work against a DB
/// that predates it.
pub struct AddEventSeries;

#[async_trait]
impl Migration for AddEventSeries {
    fn version(&self) -> i64 {
        13
    }

    fn name(&self) -> &str {
        "add event_series, event_occurrences"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS event_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_event_series_project_id ON event_series (project_id)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS event_occurrences (
                series_id TEXT NOT NULL,
                occurrence_date INTEGER NOT NULL,
                item_id TEXT,
                is_exdate INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (series_id, occurrence_date)
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
    use sqlx::SqlitePool;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
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
        let mut conn = pool.acquire().await.unwrap();

        AddEventSeries.up(&mut conn).await.unwrap();
        AddEventSeries.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO event_series (id, project_id, name, recurrence, anchor_date) \
             VALUES ('s1', 'p1', 'Standup', 'every weekday', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO event_occurrences (series_id, occurrence_date, item_id, is_exdate) \
             VALUES ('s1', 86400, NULL, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series_count, 1);

        let occurrence_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event_occurrences")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(occurrence_count, 1);
    }
}
