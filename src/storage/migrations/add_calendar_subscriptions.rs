use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// Creates the `calendar_subscriptions` table and adds `items.google_event_id`/
/// `items.calendar_subscription_id` — pure schema, nothing reads or writes any of this
/// yet (see docs/google-calendar-import-plan.md, Stage 1). `calendar_subscriptions` is
/// a brand-new table, so `CREATE TABLE IF NOT EXISTS` alone is enough, no
/// `column_exists` dance (matching `AddProjects`'s precedent); the two `items` columns
/// are on an existing table, so they need the usual guard.
///
/// Per CLAUDE.md's index-ordering note (see `AddProjects`/`AddItemSeriesId`): an index
/// on a column added to an *existing* table must be created inside the migration that
/// adds the column, never in `create_pool()`'s baseline, since baseline indexes run
/// before `run_migrations()` and would break against any DB that predates this
/// migration. `idx_items_calendar_subscription_id` and the partial unique index below
/// are both on `items` columns added by this same migration, so both live here, not in
/// the baseline. `idx_calendar_subscriptions_project_id` is on a brand-new table, so it
/// would be safe in the baseline too (like `idx_projects_team_id`) — but it's created
/// here as well (and in the baseline) since a migration must be correct standalone
/// against a DB that predates it, same as `AddItemSeries`'s own table+index pairing.
///
/// The unique partial index is a hard guarantee against ever double-importing the same
/// upstream event into the same subscription, even if the Stage 3 sync diff logic has
/// a bug — see docs/google-calendar-import-plan.md's Stage 1 section.
pub struct AddCalendarSubscriptions;

#[async_trait]
impl Migration for AddCalendarSubscriptions {
    fn version(&self) -> i64 {
        25
    }

    fn name(&self) -> &str {
        "add calendar_subscriptions, items.google_event_id, items.calendar_subscription_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS calendar_subscriptions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                ical_url TEXT NOT NULL,
                created_by_user_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_synced_at INTEGER,
                last_sync_error TEXT
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_calendar_subscriptions_project_id \
             ON calendar_subscriptions (project_id)",
        )
        .execute(&mut *conn)
        .await?;

        if !column_exists(conn, "items", "google_event_id").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN google_event_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        if !column_exists(conn, "items", "calendar_subscription_id").await? {
            sqlx::query("ALTER TABLE items ADD COLUMN calendar_subscription_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_items_calendar_subscription_id \
             ON items (calendar_subscription_id)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_items_calsub_google_event_id \
             ON items (calendar_subscription_id, google_event_id) \
             WHERE calendar_subscription_id IS NOT NULL",
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

    async fn pool_with_items_table() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn creates_table_and_columns_and_is_idempotent() {
        let pool = pool_with_items_table().await;
        let mut conn = pool.acquire().await.unwrap();

        AddCalendarSubscriptions.up(&mut conn).await.unwrap();
        AddCalendarSubscriptions.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO calendar_subscriptions \
             (id, project_id, ical_url, created_by_user_id, created_at) \
             VALUES ('sub1', 'p1', 'https://calendar.google.com/x.ics', 'u1', 1000000)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let sub_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM calendar_subscriptions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sub_count, 1);

        assert!(
            column_exists(&mut conn, "items", "google_event_id")
                .await
                .unwrap()
        );
        assert!(
            column_exists(&mut conn, "items", "calendar_subscription_id")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn is_idempotent_against_a_table_already_missing_the_new_columns() {
        // Mirrors `pool_with_items_table` but confirms a second `up()` call against a
        // DB that already has everything doesn't error (same idempotency shape every
        // other migration in this crate is tested against).
        let pool = pool_with_items_table().await;
        let mut conn = pool.acquire().await.unwrap();

        AddCalendarSubscriptions.up(&mut conn).await.unwrap();
        AddCalendarSubscriptions.up(&mut conn).await.unwrap();
        AddCalendarSubscriptions.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "items", "google_event_id")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn rejects_double_importing_the_same_google_event_into_the_same_subscription() {
        let pool = pool_with_items_table().await;
        let mut conn = pool.acquire().await.unwrap();
        AddCalendarSubscriptions.up(&mut conn).await.unwrap();

        sqlx::query(
            "INSERT INTO items (id, name, calendar_subscription_id, google_event_id) \
             VALUES ('i1', 'Dentist', 'sub1', 'gevent1')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let dup = sqlx::query(
            "INSERT INTO items (id, name, calendar_subscription_id, google_event_id) \
             VALUES ('i2', 'Dentist (dup)', 'sub1', 'gevent1')",
        )
        .execute(&pool)
        .await;
        assert!(dup.is_err());

        // A NULL calendar_subscription_id (an ordinary, non-imported item) is never
        // constrained by the partial index — two such items may share google_event_id
        // NULL freely.
        sqlx::query("INSERT INTO items (id, name) VALUES ('i3', 'Ordinary task')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO items (id, name) VALUES ('i4', 'Another ordinary task')")
            .execute(&pool)
            .await
            .unwrap();
    }
}
