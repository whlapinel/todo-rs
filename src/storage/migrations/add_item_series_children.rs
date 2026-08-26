use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::SqliteConnection;

/// docs/series-sub-items-plan.md: lets a Task-typed `item_series` have sub-item series
/// (`parent_series_id`, `due_offset_days` — mirroring `Item.parent_item_id`/
/// `Item.due_offset_days`) and removes the `template_item_id`-linked-template mechanism
/// outright (confirmed with the user that nothing depends on it — no backfill/migration of
/// existing values).
///
/// Deliberately does *not* touch `recurrence`/`anchor_date`'s `NOT NULL` constraint yet —
/// that relaxation (needed once a child series with no cadence of its own can exist) is
/// bundled into the stage that actually introduces child series validation instead, so this
/// migration stays a plain guarded ADD/DROP COLUMN pair, no rebuild-and-copy needed (see
/// `ActivityLogTeamIdNullable` for that shape, deferred to the next stage).
pub struct AddItemSeriesChildren;

#[async_trait]
impl Migration for AddItemSeriesChildren {
    fn version(&self) -> i64 {
        29
    }

    fn name(&self) -> &str {
        "add item_series parent_series_id/due_offset_days, drop template_item_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !column_exists(conn, "item_series", "parent_series_id").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN parent_series_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        if !column_exists(conn, "item_series", "due_offset_days").await? {
            sqlx::query("ALTER TABLE item_series ADD COLUMN due_offset_days INTEGER")
                .execute(&mut *conn)
                .await?;
        }
        if column_exists(conn, "item_series", "template_item_id").await? {
            sqlx::query("ALTER TABLE item_series DROP COLUMN template_item_id")
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn old_schema_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE item_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL,
                item_type TEXT NOT NULL DEFAULT 'EVENT',
                cursor_date INTEGER,
                basis TEXT,
                template_item_id TEXT,
                assigned_to_user_id TEXT,
                points INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn adds_new_columns_and_drops_template_item_id() {
        let pool = old_schema_pool().await;
        sqlx::query(
            "INSERT INTO item_series (id, project_id, name, recurrence, anchor_date, item_type, template_item_id) \
             VALUES ('s1', 'p1', 'Standup', 'every weekday', 1000, 'TASK', 'template-1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesChildren.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "item_series", "parent_series_id")
                .await
                .unwrap()
        );
        assert!(
            column_exists(&mut conn, "item_series", "due_offset_days")
                .await
                .unwrap()
        );
        assert!(
            !column_exists(&mut conn, "item_series", "template_item_id")
                .await
                .unwrap()
        );
        let name: String = sqlx::query_scalar("SELECT name FROM item_series WHERE id = 's1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "Standup");
    }

    #[tokio::test]
    async fn is_idempotent_when_run_twice() {
        let pool = old_schema_pool().await;
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesChildren.up(&mut conn).await.unwrap();
        AddItemSeriesChildren.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "item_series", "parent_series_id")
                .await
                .unwrap()
        );
        assert!(
            !column_exists(&mut conn, "item_series", "template_item_id")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn is_a_noop_against_a_table_already_on_the_new_shape() {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE item_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL,
                item_type TEXT NOT NULL DEFAULT 'EVENT',
                cursor_date INTEGER,
                basis TEXT,
                parent_series_id TEXT,
                due_offset_days INTEGER,
                assigned_to_user_id TEXT,
                points INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        AddItemSeriesChildren.up(&mut conn).await.unwrap();

        assert!(
            column_exists(&mut conn, "item_series", "parent_series_id")
                .await
                .unwrap()
        );
    }
}
