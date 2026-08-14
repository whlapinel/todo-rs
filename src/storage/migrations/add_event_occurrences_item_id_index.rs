use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Indexes `event_occurrences.item_id` for `EventSeriesRepo::find_occurrence_by_item_id` —
/// the reverse lookup stage 6 of docs/recurring-events-virtual-occurrences-rough-plan.md added
/// so deleting a materialized occurrence's item can find and mark its occurrence exdate.
/// `create_pool()` defines the identical baseline for a fresh DB, so this only does real work
/// against a DB that predates it.
pub struct AddEventOccurrencesItemIdIndex;

#[async_trait]
impl Migration for AddEventOccurrencesItemIdIndex {
    fn version(&self) -> i64 {
        15
    }

    fn name(&self) -> &str {
        "add event_occurrences item_id index"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_event_occurrences_item_id ON event_occurrences (item_id)",
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

    async fn pool_with_event_occurrences() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
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
        pool
    }

    #[tokio::test]
    async fn creates_index_and_is_idempotent() {
        let pool = pool_with_event_occurrences().await;
        let mut conn = pool.acquire().await.unwrap();

        AddEventOccurrencesItemIdIndex.up(&mut conn).await.unwrap();
        AddEventOccurrencesItemIdIndex.up(&mut conn).await.unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_event_occurrences_item_id'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}
