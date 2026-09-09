use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Retires the scheduled-date basis a Task-typed `item_series` row could carry (see
/// `domain::item_series::ItemSeries::basis`'s doc comment, decided
/// docs/issues_and_features.md 2026-09-05): a Task series now always materializes onto
/// `due_date`, so `basis` on a Task series only ever means "the default" (`NULL`) or
/// `'COMPLETION'` going forward — `service::item_series::validate_series_basis` rejects
/// any other value on new writes, including the retired `'DUE_DATE'` opt-in.
///
/// Normalizes every already-stored row to that same shape rather than leaving a live row
/// in a state new writes can no longer produce: a Task-typed row's `basis` is cleared to
/// `NULL` unless it's already `'COMPLETION'` — collapsing both the old scheduled-date
/// default (`NULL`) and the old due-date opt-in (`'DUE_DATE''`) onto the one value that
/// now means the same thing, `NULL` (the old due-date opt-in was spelled `'DUE_DATE'`).
/// An Event-typed row's `basis` was never legally
/// anything but `NULL` (`validate_series_basis` has always rejected a non-`NULL` value on
/// a non-Task series), but is cleared too in case one was ever written outside that path.
pub struct RetireItemSeriesScheduledBasis;

#[async_trait]
impl Migration for RetireItemSeriesScheduledBasis {
    fn version(&self) -> i64 {
        36
    }

    fn name(&self) -> &str {
        "retire item_series scheduled-date basis"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "UPDATE item_series SET basis = NULL \
             WHERE item_type = 'TASK' AND basis IS NOT NULL AND basis != 'COMPLETION'",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "UPDATE item_series SET basis = NULL WHERE item_type != 'TASK' AND basis IS NOT NULL",
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;
    use sqlx::SqlitePool;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn pool_with_series(rows: &[(&str, &str, Option<&str>)]) -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE item_series (
                id TEXT PRIMARY KEY,
                item_type TEXT NOT NULL,
                basis TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (id, item_type, basis) in rows {
            sqlx::query("INSERT INTO item_series (id, item_type, basis) VALUES (?, ?, ?)")
                .bind(id)
                .bind(item_type)
                .bind(basis)
                .execute(&pool)
                .await
                .unwrap();
        }
        pool
    }

    async fn basis_of(pool: &SqlitePool, id: &str) -> Option<String> {
        sqlx::query("SELECT basis FROM item_series WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
            .get("basis")
    }

    #[tokio::test]
    async fn clears_due_date_and_null_basis_on_task_series_but_keeps_completion() {
        let pool = pool_with_series(&[
            ("t-due", "TASK", Some("DUE_DATE")),
            ("t-null", "TASK", None),
            ("t-completion", "TASK", Some("COMPLETION")),
        ])
        .await;
        let mut conn = pool.acquire().await.unwrap();

        RetireItemSeriesScheduledBasis.up(&mut conn).await.unwrap();

        assert_eq!(basis_of(&pool, "t-due").await, None);
        assert_eq!(basis_of(&pool, "t-null").await, None);
        assert_eq!(
            basis_of(&pool, "t-completion").await,
            Some("COMPLETION".to_string())
        );
    }

    #[tokio::test]
    async fn clears_any_basis_on_a_non_task_series() {
        let pool = pool_with_series(&[
            ("e-due", "EVENT", Some("DUE_DATE")),
            ("e-null", "EVENT", None),
        ])
        .await;
        let mut conn = pool.acquire().await.unwrap();

        RetireItemSeriesScheduledBasis.up(&mut conn).await.unwrap();

        assert_eq!(basis_of(&pool, "e-due").await, None);
        assert_eq!(basis_of(&pool, "e-null").await, None);
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = pool_with_series(&[("t-due", "TASK", Some("DUE_DATE"))]).await;
        let mut conn = pool.acquire().await.unwrap();

        RetireItemSeriesScheduledBasis.up(&mut conn).await.unwrap();
        RetireItemSeriesScheduledBasis.up(&mut conn).await.unwrap();

        assert_eq!(basis_of(&pool, "t-due").await, None);
    }
}
