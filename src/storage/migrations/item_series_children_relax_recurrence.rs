use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::{Row, SqliteConnection};

/// docs/series-sub-items-plan.md, stage 2: relaxes `item_series.recurrence`/`anchor_date`
/// from `NOT NULL` to nullable — a child series (`parent_series_id.is_some()`) has no
/// cadence of its own (decision 2: its "current occurrence" is always exactly its parent's),
/// so it carries `NULL` for both. SQLite has no `ALTER TABLE ... ALTER COLUMN` to drop a
/// constraint, so this uses the same rebuild-and-copy shape `ActivityLogTeamIdNullable`
/// established: a fresh `item_series_new` with the relaxed schema, copy every row across,
/// drop the old table, rename the new one into place, then recreate the index that lived on
/// the dropped table (indexes don't survive a `DROP TABLE`).
///
/// Purely relaxing, not destructive: every existing row's `recurrence`/`anchor_date` is
/// preserved as-is (still non-`NULL` in practice for every row written before this
/// migration, since nothing could create a child series before this stage).
pub struct ItemSeriesChildrenRelaxRecurrence;

async fn recurrence_is_not_null(conn: &mut SqliteConnection) -> Result<bool, MigrationError> {
    let rows = sqlx::query("PRAGMA table_info(item_series)")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.iter().any(|row| {
        row.get::<String, _>("name") == "recurrence" && row.get::<i64, _>("notnull") != 0
    }))
}

#[async_trait]
impl Migration for ItemSeriesChildrenRelaxRecurrence {
    fn version(&self) -> i64 {
        30
    }

    fn name(&self) -> &str {
        "make item_series.recurrence/anchor_date nullable"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        if !recurrence_is_not_null(conn).await? {
            return Ok(());
        }

        sqlx::query(
            "CREATE TABLE item_series_new (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT,
                anchor_date INTEGER,
                item_type TEXT NOT NULL DEFAULT 'EVENT',
                cursor_date INTEGER,
                basis TEXT,
                parent_series_id TEXT,
                due_offset_days INTEGER,
                assigned_to_user_id TEXT,
                points INTEGER
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO item_series_new (id, project_id, name, description, event_type, recurrence, anchor_date, item_type, cursor_date, basis, parent_series_id, due_offset_days, assigned_to_user_id, points) \
             SELECT id, project_id, name, description, event_type, recurrence, anchor_date, item_type, cursor_date, basis, parent_series_id, due_offset_days, assigned_to_user_id, points FROM item_series",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("DROP TABLE item_series")
            .execute(&mut *conn)
            .await?;
        sqlx::query("ALTER TABLE item_series_new RENAME TO item_series")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_item_series_project_id ON item_series (project_id)",
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

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    async fn create_old_schema(pool: &SqlitePool) {
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
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn relaxes_recurrence_and_anchor_date_and_preserves_existing_rows() {
        let pool = test_pool().await;
        create_old_schema(&pool).await;
        sqlx::query(
            "INSERT INTO item_series (id, project_id, name, recurrence, anchor_date, item_type) \
             VALUES ('s1', 'p1', 'Standup', 'every weekday', 1000, 'TASK')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        ItemSeriesChildrenRelaxRecurrence
            .up(&mut conn)
            .await
            .unwrap();

        assert!(!recurrence_is_not_null(&mut conn).await.unwrap());
        let row = sqlx::query("SELECT recurrence, anchor_date FROM item_series WHERE id = 's1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("recurrence"), "every weekday");
        assert_eq!(row.get::<i64, _>("anchor_date"), 1000);

        // The relaxed table now actually accepts NULL recurrence/anchor_date (a child series).
        sqlx::query(
            "INSERT INTO item_series (id, project_id, name, recurrence, anchor_date, item_type, parent_series_id) \
             VALUES ('s2', 'p1', 'Order supplies', NULL, NULL, 'TASK', 's1')",
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
        ItemSeriesChildrenRelaxRecurrence
            .up(&mut conn)
            .await
            .unwrap();
        ItemSeriesChildrenRelaxRecurrence
            .up(&mut conn)
            .await
            .unwrap();

        assert!(!recurrence_is_not_null(&mut conn).await.unwrap());
    }
}
