use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::domain::item::ItemKind;
use crate::domain::item_series::{
    ItemOccurrence, ItemSeries, ItemSeriesChild, SeriesChildOccurrence,
};
use crate::storage::sqlite::{ItemSeriesRepo, RepoError, db_err, not_found};

pub struct SqliteItemSeriesRepo(pub SqlitePool);

fn to_secs(dt: DateTime<Utc>) -> i64 {
    dt.timestamp()
}

fn from_secs(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .with_timezone(&Utc)
}

fn row_to_series(row: &sqlx::sqlite::SqliteRow) -> ItemSeries {
    let anchor_secs: i64 = row.get("anchor_date");
    let item_type: String = row.get("item_type");
    let cursor_secs: Option<i64> = row.get("cursor_date");
    ItemSeries {
        id: row.get("id"),
        project_id: row.get("project_id"),
        name: row.get("name"),
        description: row.get("description"),
        event_type: row.get("event_type"),
        recurrence: row.get("recurrence"),
        anchor_date: from_secs(anchor_secs),
        item_type: item_type.parse().unwrap_or(ItemKind::Event),
        cursor_date: cursor_secs.map(from_secs),
        basis: row.get("basis"),
        assigned_to_user_id: row.get("assigned_to_user_id"),
        points: row.get("points"),
        priority: row.get("priority"),
    }
}

fn row_to_occurrence(row: &sqlx::sqlite::SqliteRow) -> ItemOccurrence {
    let occurrence_secs: i64 = row.get("occurrence_date");
    let is_exdate: i64 = row.get("is_exdate");
    ItemOccurrence {
        series_id: row.get("series_id"),
        occurrence_date: from_secs(occurrence_secs),
        item_id: row.get("item_id"),
        is_exdate: is_exdate != 0,
    }
}

fn row_to_series_child(row: &sqlx::sqlite::SqliteRow) -> ItemSeriesChild {
    ItemSeriesChild {
        id: row.get("id"),
        series_id: row.get("series_id"),
        name: row.get("name"),
        description: row.get("description"),
        days_before: row.get("days_before"),
        priority: row.get("priority"),
        sort_order: row.get("sort_order"),
    }
}

fn row_to_child_occurrence(row: &sqlx::sqlite::SqliteRow) -> SeriesChildOccurrence {
    let occurrence_secs: i64 = row.get("occurrence_date");
    SeriesChildOccurrence {
        child_id: row.get("child_id"),
        occurrence_date: from_secs(occurrence_secs),
        item_id: row.get("item_id"),
    }
}

#[async_trait]
impl ItemSeriesRepo for SqliteItemSeriesRepo {
    async fn create_series(&self, series: &ItemSeries) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO item_series (id, project_id, name, description, event_type, recurrence, anchor_date, item_type, basis, assigned_to_user_id, points, priority) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&series.project_id)
        .bind(&series.name)
        .bind(&series.description)
        .bind(&series.event_type)
        .bind(&series.recurrence)
        .bind(to_secs(series.anchor_date))
        .bind(series.item_type.as_str())
        .bind(&series.basis)
        .bind(&series.assigned_to_user_id)
        .bind(series.points)
        .bind(series.priority)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn update_series(&self, series_id: &str, series: &ItemSeries) -> Result<(), RepoError> {
        let result = sqlx::query(
            "UPDATE item_series SET name = ?, description = ?, event_type = ?, recurrence = ?, anchor_date = ?, item_type = ?, basis = ?, assigned_to_user_id = ?, points = ?, priority = ? \
             WHERE id = ?",
        )
        .bind(&series.name)
        .bind(&series.description)
        .bind(&series.event_type)
        .bind(&series.recurrence)
        .bind(to_secs(series.anchor_date))
        .bind(series.item_type.as_str())
        .bind(&series.basis)
        .bind(&series.assigned_to_user_id)
        .bind(series.points)
        .bind(series.priority)
        .bind(series_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(not_found());
        }
        Ok(())
    }

    async fn get_series(&self, series_id: &str) -> Result<ItemSeries, RepoError> {
        sqlx::query(
            "SELECT id, project_id, name, description, event_type, recurrence, anchor_date, item_type, cursor_date, basis, assigned_to_user_id, points, priority \
             FROM item_series WHERE id = ?",
        )
        .bind(series_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        .map(|row| row_to_series(&row))
        .ok_or_else(not_found)
    }

    async fn list_series_for_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<ItemSeries>, RepoError> {
        sqlx::query(
            "SELECT id, project_id, name, description, event_type, recurrence, anchor_date, item_type, cursor_date, basis, assigned_to_user_id, points, priority \
             FROM item_series WHERE project_id = ? ORDER BY name ASC",
        )
        .bind(project_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| rows.iter().map(row_to_series).collect())
    }

    async fn get_occurrence(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<Option<ItemOccurrence>, RepoError> {
        sqlx::query(
            "SELECT series_id, occurrence_date, item_id, is_exdate FROM item_occurrences \
             WHERE series_id = ? AND occurrence_date = ?",
        )
        .bind(series_id)
        .bind(to_secs(occurrence_date))
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)
        .map(|row| row.map(|row| row_to_occurrence(&row)))
    }

    async fn record_materialized_occurrence(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
        item_id: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO item_occurrences (series_id, occurrence_date, item_id, is_exdate) \
             VALUES (?, ?, ?, 0) \
             ON CONFLICT (series_id, occurrence_date) \
             DO UPDATE SET item_id = excluded.item_id, is_exdate = 0",
        )
        .bind(series_id)
        .bind(to_secs(occurrence_date))
        .bind(item_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn mark_exdate(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO item_occurrences (series_id, occurrence_date, item_id, is_exdate) \
             VALUES (?, ?, NULL, 1) \
             ON CONFLICT (series_id, occurrence_date) \
             DO UPDATE SET is_exdate = 1, item_id = NULL",
        )
        .bind(series_id)
        .bind(to_secs(occurrence_date))
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_occurrence(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM item_occurrences WHERE series_id = ? AND occurrence_date = ?")
            .bind(series_id)
            .bind(to_secs(occurrence_date))
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_occurrences_between(
        &self,
        series_id: &str,
        range_start: DateTime<Utc>,
        range_end: DateTime<Utc>,
    ) -> Result<Vec<ItemOccurrence>, RepoError> {
        sqlx::query(
            "SELECT series_id, occurrence_date, item_id, is_exdate FROM item_occurrences \
             WHERE series_id = ? AND occurrence_date BETWEEN ? AND ? \
             ORDER BY occurrence_date ASC",
        )
        .bind(series_id)
        .bind(to_secs(range_start))
        .bind(to_secs(range_end))
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| rows.iter().map(row_to_occurrence).collect())
    }

    async fn find_occurrence_by_item_id(
        &self,
        item_id: &str,
    ) -> Result<Option<ItemOccurrence>, RepoError> {
        sqlx::query(
            "SELECT series_id, occurrence_date, item_id, is_exdate FROM item_occurrences \
             WHERE item_id = ?",
        )
        .bind(item_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)
        .map(|row| row.map(|row| row_to_occurrence(&row)))
    }

    async fn advance_cursor(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        let secs = to_secs(occurrence_date);
        // SQLite's multi-arg max() is the scalar "largest of these values" form, not the
        // single-arg aggregate — `COALESCE(cursor_date, ?)` makes a NULL cursor compare as
        // `secs` itself, so a first-ever advance sets cursor_date = secs in the same
        // statement as a forward move, with no separate read-then-write race.
        let result = sqlx::query(
            "UPDATE item_series SET cursor_date = MAX(COALESCE(cursor_date, ?), ?) WHERE id = ?",
        )
        .bind(secs)
        .bind(secs)
        .bind(series_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(not_found());
        }
        Ok(())
    }

    async fn retreat_cursor(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        let secs = to_secs(occurrence_date);
        let result = sqlx::query(
            "UPDATE item_series SET cursor_date = MIN(COALESCE(cursor_date, ?), ?) WHERE id = ?",
        )
        .bind(secs)
        .bind(secs)
        .bind(series_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(not_found());
        }
        Ok(())
    }

    async fn clear_cursor(
        &self,
        series_id: &str,
        expected_occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        sqlx::query("UPDATE item_series SET cursor_date = NULL WHERE id = ? AND cursor_date = ?")
            .bind(series_id)
            .bind(to_secs(expected_occurrence_date))
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn delete_series(&self, series_id: &str) -> Result<(), RepoError> {
        // No FK/ON DELETE CASCADE between item_occurrences.series_id and item_series.id
        // (see the schema in create_pool()), so both deletes run in one transaction to
        // avoid ever leaving orphaned item_occurrences rows behind on a partial failure.
        let mut tx = self.0.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM item_occurrences WHERE series_id = ?")
            .bind(series_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let result = sqlx::query("DELETE FROM item_series WHERE id = ?")
            .bind(series_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(not_found());
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn list_rotation_members(&self, series_id: &str) -> Result<Vec<String>, RepoError> {
        sqlx::query(
            "SELECT user_id FROM item_series_rotation_members \
             WHERE series_id = ? ORDER BY user_id ASC",
        )
        .bind(series_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| rows.iter().map(|row| row.get("user_id")).collect())
    }

    async fn set_rotation_members(
        &self,
        series_id: &str,
        user_ids: &[String],
    ) -> Result<(), RepoError> {
        let mut tx = self.0.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM item_series_rotation_members WHERE series_id = ?")
            .bind(series_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        for user_id in user_ids {
            sqlx::query(
                "INSERT INTO item_series_rotation_members (series_id, user_id) VALUES (?, ?)",
            )
            .bind(series_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn list_series_children(
        &self,
        series_id: &str,
    ) -> Result<Vec<ItemSeriesChild>, RepoError> {
        sqlx::query(
            "SELECT id, series_id, name, description, days_before, priority, sort_order \
             FROM item_series_children WHERE series_id = ? ORDER BY sort_order ASC, id ASC",
        )
        .bind(series_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| rows.iter().map(row_to_series_child).collect())
    }

    async fn get_series_child(&self, child_id: &str) -> Result<ItemSeriesChild, RepoError> {
        sqlx::query(
            "SELECT id, series_id, name, description, days_before, priority, sort_order \
             FROM item_series_children WHERE id = ?",
        )
        .bind(child_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        .map(|row| row_to_series_child(&row))
        .ok_or_else(not_found)
    }

    async fn create_series_child(&self, child: &ItemSeriesChild) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO item_series_children \
             (id, series_id, name, description, days_before, priority, sort_order) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&child.series_id)
        .bind(&child.name)
        .bind(&child.description)
        .bind(child.days_before)
        .bind(child.priority)
        .bind(child.sort_order)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn update_series_child(
        &self,
        child_id: &str,
        child: &ItemSeriesChild,
    ) -> Result<(), RepoError> {
        let result = sqlx::query(
            "UPDATE item_series_children \
             SET name = ?, description = ?, days_before = ?, priority = ?, sort_order = ? \
             WHERE id = ?",
        )
        .bind(&child.name)
        .bind(&child.description)
        .bind(child.days_before)
        .bind(child.priority)
        .bind(child.sort_order)
        .bind(child_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(not_found());
        }
        Ok(())
    }

    async fn delete_series_child(&self, child_id: &str) -> Result<(), RepoError> {
        // Same one-transaction reasoning as `delete_series`: there's no FK cascade between
        // series_child_occurrences.child_id and item_series_children.id, so a partial
        // failure must not leave orphaned occurrence rows behind.
        let mut tx = self.0.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM series_child_occurrences WHERE child_id = ?")
            .bind(child_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let result = sqlx::query("DELETE FROM item_series_children WHERE id = ?")
            .bind(child_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(not_found());
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn delete_series_children_for_series(&self, series_id: &str) -> Result<(), RepoError> {
        let mut tx = self.0.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM series_child_occurrences WHERE child_id IN \
             (SELECT id FROM item_series_children WHERE series_id = ?)",
        )
        .bind(series_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query("DELETE FROM item_series_children WHERE series_id = ?")
            .bind(series_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        // No rows_affected check: unlike `delete_series_child`, this is the
        // `delete_series` cascade, and a series with no sub-items at all is the norm.
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn get_child_occurrence(
        &self,
        child_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<Option<SeriesChildOccurrence>, RepoError> {
        sqlx::query(
            "SELECT child_id, occurrence_date, item_id FROM series_child_occurrences \
             WHERE child_id = ? AND occurrence_date = ?",
        )
        .bind(child_id)
        .bind(to_secs(occurrence_date))
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)
        .map(|row| row.map(|row| row_to_child_occurrence(&row)))
    }

    async fn list_child_occurrences_for_series(
        &self,
        series_id: &str,
        range_start: DateTime<Utc>,
        range_end: DateTime<Utc>,
    ) -> Result<Vec<SeriesChildOccurrence>, RepoError> {
        sqlx::query(
            "SELECT o.child_id, o.occurrence_date, o.item_id FROM series_child_occurrences o \
             JOIN item_series_children c ON c.id = o.child_id \
             WHERE c.series_id = ? AND o.occurrence_date BETWEEN ? AND ? \
             ORDER BY o.occurrence_date ASC",
        )
        .bind(series_id)
        .bind(to_secs(range_start))
        .bind(to_secs(range_end))
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| rows.iter().map(row_to_child_occurrence).collect())
    }

    async fn record_materialized_child_occurrence(
        &self,
        child_id: &str,
        occurrence_date: DateTime<Utc>,
        item_id: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO series_child_occurrences (child_id, occurrence_date, item_id) \
             VALUES (?, ?, ?) \
             ON CONFLICT (child_id, occurrence_date) DO UPDATE SET item_id = excluded.item_id",
        )
        .bind(child_id)
        .bind(to_secs(occurrence_date))
        .bind(item_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_child_occurrence(
        &self,
        child_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "DELETE FROM series_child_occurrences WHERE child_id = ? AND occurrence_date = ?",
        )
        .bind(child_id)
        .bind(to_secs(occurrence_date))
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn find_child_occurrence_by_item_id(
        &self,
        item_id: &str,
    ) -> Result<Option<SeriesChildOccurrence>, RepoError> {
        sqlx::query(
            "SELECT child_id, occurrence_date, item_id FROM series_child_occurrences \
             WHERE item_id = ?",
        )
        .bind(item_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)
        .map(|row| row.map(|row| row_to_child_occurrence(&row)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
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
                points INTEGER,
                priority INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE item_occurrences (
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
            "CREATE TABLE item_series_rotation_members (
                series_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                PRIMARY KEY (series_id, user_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE item_series_children (
                id TEXT PRIMARY KEY,
                series_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                days_before INTEGER NOT NULL DEFAULT 0,
                priority INTEGER,
                sort_order INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE series_child_occurrences (
                child_id TEXT NOT NULL,
                occurrence_date INTEGER NOT NULL,
                item_id TEXT NOT NULL,
                PRIMARY KEY (child_id, occurrence_date)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn dt(secs: i64) -> DateTime<Utc> {
        from_secs(secs)
    }

    fn sample_child(series_id: &str, name: &str, days_before: i32) -> ItemSeriesChild {
        ItemSeriesChild {
            id: String::new(),
            series_id: series_id.to_string(),
            name: name.to_string(),
            description: None,
            days_before,
            priority: None,
            sort_order: 0,
        }
    }

    fn sample_series(project_id: &str) -> ItemSeries {
        ItemSeries {
            id: String::new(),
            project_id: project_id.to_string(),
            name: "Standup".to_string(),
            description: Some("Daily sync".to_string()),
            event_type: Some("meeting".to_string()),
            recurrence: "every weekday".to_string(),
            anchor_date: dt(1_000_000),
            item_type: ItemKind::Event,
            cursor_date: None,
            basis: None,
            assigned_to_user_id: None,
            points: None,
            priority: None,
        }
    }

    #[tokio::test]
    async fn create_and_get_round_trip() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);

        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        let series = repo.get_series(&id).await.unwrap();

        assert_eq!(series.project_id, "p1");
        assert_eq!(series.name, "Standup");
        assert_eq!(series.description, Some("Daily sync".to_string()));
        assert_eq!(series.event_type, Some("meeting".to_string()));
        assert_eq!(series.recurrence, "every weekday");
        assert_eq!(series.anchor_date, dt(1_000_000));
        assert_eq!(series.item_type, ItemKind::Event);
    }

    #[tokio::test]
    async fn create_and_get_round_trip_for_a_task_series() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let mut task_series = sample_series("p1");
        task_series.item_type = ItemKind::Task;

        let id = repo.create_series(&task_series).await.unwrap();
        let series = repo.get_series(&id).await.unwrap();

        assert_eq!(series.item_type, ItemKind::Task);
    }

    #[tokio::test]
    async fn create_and_get_round_trip_for_a_series_with_assignment_and_points() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let mut task_series = sample_series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.assigned_to_user_id = Some("user-1".to_string());
        task_series.points = Some(10);

        let id = repo.create_series(&task_series).await.unwrap();
        let series = repo.get_series(&id).await.unwrap();

        assert_eq!(series.assigned_to_user_id, Some("user-1".to_string()));
        assert_eq!(series.points, Some(10));
    }

    #[tokio::test]
    async fn create_and_get_round_trip_for_a_completion_basis_series() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let mut task_series = sample_series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.basis = Some("COMPLETION".to_string());

        let id = repo.create_series(&task_series).await.unwrap();
        let series = repo.get_series(&id).await.unwrap();

        assert_eq!(series.basis, Some("COMPLETION".to_string()));
    }

    #[tokio::test]
    async fn update_series_overwrites_fields_but_not_project_id() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();

        let mut update = sample_series("p2");
        update.name = "Retro".to_string();
        update.description = None;
        update.event_type = None;
        update.recurrence = "every friday".to_string();
        update.anchor_date = dt(2_000_000);
        update.item_type = ItemKind::Task;
        update.basis = Some("COMPLETION".to_string());
        update.assigned_to_user_id = Some("user-1".to_string());
        update.points = Some(15);
        repo.update_series(&id, &update).await.unwrap();

        let series = repo.get_series(&id).await.unwrap();
        assert_eq!(series.project_id, "p1");
        assert_eq!(series.name, "Retro");
        assert_eq!(series.description, None);
        assert_eq!(series.event_type, None);
        assert_eq!(series.recurrence, "every friday");
        assert_eq!(series.anchor_date, dt(2_000_000));
        assert_eq!(series.item_type, ItemKind::Task);
        assert_eq!(series.basis, Some("COMPLETION".to_string()));
        assert_eq!(series.assigned_to_user_id, Some("user-1".to_string()));
        assert_eq!(series.points, Some(15));
    }

    #[tokio::test]
    async fn update_series_missing_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);

        let err = repo
            .update_series("missing", &sample_series("p1"))
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn get_series_missing_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);

        let err = repo.get_series("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn list_series_for_project_orders_by_name_and_scopes_to_project() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let mut zebra = sample_series("p1");
        zebra.name = "Zebra".to_string();
        let mut apple = sample_series("p1");
        apple.name = "Apple".to_string();
        let mut other = sample_series("p2");
        other.name = "Other".to_string();
        repo.create_series(&zebra).await.unwrap();
        repo.create_series(&apple).await.unwrap();
        repo.create_series(&other).await.unwrap();

        let series = repo.list_series_for_project("p1").await.unwrap();
        let names: Vec<_> = series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "Zebra"]);
    }

    #[tokio::test]
    async fn get_occurrence_returns_none_for_purely_virtual_date() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();

        let occurrence = repo.get_occurrence(&id, dt(2_000_000)).await.unwrap();
        assert!(occurrence.is_none());
    }

    #[tokio::test]
    async fn record_materialized_occurrence_then_get_occurrence_round_trips() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();

        repo.record_materialized_occurrence(&id, dt(2_000_000), "item-1")
            .await
            .unwrap();

        let occurrence = repo
            .get_occurrence(&id, dt(2_000_000))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(occurrence.item_id, Some("item-1".to_string()));
        assert!(!occurrence.is_exdate);
    }

    #[tokio::test]
    async fn record_materialized_occurrence_is_upsert_and_clears_exdate() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.mark_exdate(&id, dt(2_000_000)).await.unwrap();

        repo.record_materialized_occurrence(&id, dt(2_000_000), "item-1")
            .await
            .unwrap();

        let occurrence = repo
            .get_occurrence(&id, dt(2_000_000))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(occurrence.item_id, Some("item-1".to_string()));
        assert!(!occurrence.is_exdate);
    }

    #[tokio::test]
    async fn mark_exdate_sets_flag_without_an_item_id() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();

        repo.mark_exdate(&id, dt(2_000_000)).await.unwrap();

        let occurrence = repo
            .get_occurrence(&id, dt(2_000_000))
            .await
            .unwrap()
            .unwrap();
        assert!(occurrence.is_exdate);
        assert_eq!(occurrence.item_id, None);
    }

    #[tokio::test]
    async fn mark_exdate_on_materialized_occurrence_clears_item_id() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.record_materialized_occurrence(&id, dt(2_000_000), "item-1")
            .await
            .unwrap();

        repo.mark_exdate(&id, dt(2_000_000)).await.unwrap();

        let occurrence = repo
            .get_occurrence(&id, dt(2_000_000))
            .await
            .unwrap()
            .unwrap();
        assert!(occurrence.is_exdate);
        assert_eq!(occurrence.item_id, None);
    }

    #[tokio::test]
    async fn delete_occurrence_removes_a_materialized_row_entirely() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.record_materialized_occurrence(&id, dt(2_000_000), "item-1")
            .await
            .unwrap();

        repo.delete_occurrence(&id, dt(2_000_000)).await.unwrap();

        let occurrence = repo.get_occurrence(&id, dt(2_000_000)).await.unwrap();
        assert!(occurrence.is_none());
    }

    #[tokio::test]
    async fn delete_occurrence_is_a_no_op_for_a_date_with_no_row() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();

        repo.delete_occurrence(&id, dt(2_000_000)).await.unwrap();

        let occurrence = repo.get_occurrence(&id, dt(2_000_000)).await.unwrap();
        assert!(occurrence.is_none());
    }

    #[tokio::test]
    async fn list_occurrences_between_only_returns_rows_within_range() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.record_materialized_occurrence(&id, dt(1_000_000), "in-range-early")
            .await
            .unwrap();
        repo.record_materialized_occurrence(&id, dt(1_500_000), "in-range-late")
            .await
            .unwrap();
        repo.record_materialized_occurrence(&id, dt(9_000_000), "out-of-range")
            .await
            .unwrap();

        let occurrences = repo
            .list_occurrences_between(&id, dt(1_000_000), dt(2_000_000))
            .await
            .unwrap();

        let item_ids: Vec<_> = occurrences
            .iter()
            .filter_map(|o| o.item_id.clone())
            .collect();
        assert_eq!(item_ids, vec!["in-range-early", "in-range-late"]);
    }

    #[tokio::test]
    async fn list_occurrences_between_excludes_purely_virtual_dates() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.record_materialized_occurrence(&id, dt(1_000_000), "item-1")
            .await
            .unwrap();

        let occurrences = repo
            .list_occurrences_between(&id, dt(0), dt(9_000_000))
            .await
            .unwrap();

        assert_eq!(occurrences.len(), 1);
    }

    #[tokio::test]
    async fn find_occurrence_by_item_id_finds_a_materialized_occurrence() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.record_materialized_occurrence(&id, dt(2_000_000), "item-1")
            .await
            .unwrap();

        let occurrence = repo
            .find_occurrence_by_item_id("item-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(occurrence.series_id, id);
        assert_eq!(occurrence.occurrence_date, dt(2_000_000));
        assert!(!occurrence.is_exdate);
    }

    #[tokio::test]
    async fn find_occurrence_by_item_id_returns_none_for_an_item_from_no_series() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        repo.create_series(&sample_series("p1")).await.unwrap();

        let occurrence = repo
            .find_occurrence_by_item_id("some-unrelated-item")
            .await
            .unwrap();
        assert!(occurrence.is_none());
    }

    #[tokio::test]
    async fn advance_cursor_sets_cursor_from_a_null_starting_point() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        assert_eq!(repo.get_series(&id).await.unwrap().cursor_date, None);

        repo.advance_cursor(&id, dt(2_000_000)).await.unwrap();

        assert_eq!(
            repo.get_series(&id).await.unwrap().cursor_date,
            Some(dt(2_000_000))
        );
    }

    #[tokio::test]
    async fn advance_cursor_moves_forward() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.advance_cursor(&id, dt(2_000_000)).await.unwrap();

        repo.advance_cursor(&id, dt(3_000_000)).await.unwrap();

        assert_eq!(
            repo.get_series(&id).await.unwrap().cursor_date,
            Some(dt(3_000_000))
        );
    }

    #[tokio::test]
    async fn advance_cursor_never_moves_backward() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.advance_cursor(&id, dt(3_000_000)).await.unwrap();

        repo.advance_cursor(&id, dt(2_000_000)).await.unwrap();

        assert_eq!(
            repo.get_series(&id).await.unwrap().cursor_date,
            Some(dt(3_000_000))
        );
    }

    #[tokio::test]
    async fn advance_cursor_missing_series_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);

        let err = repo
            .advance_cursor("missing", dt(2_000_000))
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn retreat_cursor_moves_backward() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.advance_cursor(&id, dt(3_000_000)).await.unwrap();

        repo.retreat_cursor(&id, dt(2_000_000)).await.unwrap();

        assert_eq!(
            repo.get_series(&id).await.unwrap().cursor_date,
            Some(dt(2_000_000))
        );
    }

    #[tokio::test]
    async fn retreat_cursor_never_moves_forward() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.advance_cursor(&id, dt(2_000_000)).await.unwrap();

        repo.retreat_cursor(&id, dt(3_000_000)).await.unwrap();

        assert_eq!(
            repo.get_series(&id).await.unwrap().cursor_date,
            Some(dt(2_000_000))
        );
    }

    #[tokio::test]
    async fn retreat_cursor_missing_series_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);

        let err = repo
            .retreat_cursor("missing", dt(2_000_000))
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn clear_cursor_clears_when_guard_matches() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.advance_cursor(&id, dt(1_000_000)).await.unwrap();

        repo.clear_cursor(&id, dt(1_000_000)).await.unwrap();

        assert_eq!(repo.get_series(&id).await.unwrap().cursor_date, None);
    }

    #[tokio::test]
    async fn clear_cursor_is_a_no_op_when_guard_does_not_match() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.advance_cursor(&id, dt(2_000_000)).await.unwrap();

        // Guard date doesn't match the real cursor_date (2_000_000) — a concurrent
        // settlement moved it since this caller last read it, so the clear must not
        // clobber that newer state.
        repo.clear_cursor(&id, dt(1_000_000)).await.unwrap();

        assert_eq!(
            repo.get_series(&id).await.unwrap().cursor_date,
            Some(dt(2_000_000))
        );
    }

    #[tokio::test]
    async fn delete_series_removes_series_and_its_occurrences() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.record_materialized_occurrence(&id, dt(2_000_000), "item-1")
            .await
            .unwrap();

        repo.delete_series(&id).await.unwrap();

        assert!(matches!(
            repo.get_series(&id).await,
            Err(RepoError::NotFound)
        ));
        let occurrences = repo
            .list_occurrences_between(&id, dt(0), dt(9_999_999))
            .await
            .unwrap();
        assert!(occurrences.is_empty());
    }

    #[tokio::test]
    async fn delete_series_missing_series_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);

        let err = repo.delete_series("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn list_rotation_members_returns_empty_for_a_series_with_none_set() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();

        let members = repo.list_rotation_members(&id).await.unwrap();

        assert!(members.is_empty());
    }

    #[tokio::test]
    async fn set_rotation_members_then_list_round_trips_sorted_by_user_id() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();

        // Deliberately unsorted input — list_rotation_members' sort is what defines
        // the cycle order, not insertion order.
        repo.set_rotation_members(
            &id,
            &["carol".to_string(), "alice".to_string(), "bob".to_string()],
        )
        .await
        .unwrap();

        let members = repo.list_rotation_members(&id).await.unwrap();

        assert_eq!(members, vec!["alice", "bob", "carol"]);
    }

    #[tokio::test]
    async fn set_rotation_members_replaces_the_prior_set() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.set_rotation_members(&id, &["alice".to_string(), "bob".to_string()])
            .await
            .unwrap();

        repo.set_rotation_members(&id, &["carol".to_string()])
            .await
            .unwrap();

        let members = repo.list_rotation_members(&id).await.unwrap();
        assert_eq!(members, vec!["carol"]);
    }

    #[tokio::test]
    async fn set_rotation_members_with_an_empty_slice_clears_the_rotation() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.set_rotation_members(&id, &["alice".to_string()])
            .await
            .unwrap();

        repo.set_rotation_members(&id, &[]).await.unwrap();

        let members = repo.list_rotation_members(&id).await.unwrap();
        assert!(members.is_empty());
    }

    #[tokio::test]
    async fn rotation_members_are_scoped_to_their_own_series() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let id1 = repo.create_series(&sample_series("p1")).await.unwrap();
        let id2 = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.set_rotation_members(&id1, &["alice".to_string()])
            .await
            .unwrap();
        repo.set_rotation_members(&id2, &["bob".to_string()])
            .await
            .unwrap();

        assert_eq!(
            repo.list_rotation_members(&id1).await.unwrap(),
            vec!["alice"]
        );
        assert_eq!(repo.list_rotation_members(&id2).await.unwrap(), vec!["bob"]);
    }

    // --- Series sub-items ---

    #[tokio::test]
    async fn create_and_get_series_child_round_trip() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();

        let mut child = sample_child(&series_id, "Book venue", 30);
        child.description = Some("Call the hall".to_string());
        child.priority = Some(2);
        child.sort_order = 5;
        let child_id = repo.create_series_child(&child).await.unwrap();

        let stored = repo.get_series_child(&child_id).await.unwrap();
        assert_eq!(stored.id, child_id);
        assert_eq!(stored.series_id, series_id);
        assert_eq!(stored.name, "Book venue");
        assert_eq!(stored.description, Some("Call the hall".to_string()));
        assert_eq!(stored.days_before, 30);
        assert_eq!(stored.priority, Some(2));
        assert_eq!(stored.sort_order, 5);
    }

    #[tokio::test]
    async fn create_series_child_ignores_the_supplied_id() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();

        let mut child = sample_child(&series_id, "Book venue", 30);
        child.id = "client-supplied".to_string();
        let child_id = repo.create_series_child(&child).await.unwrap();

        assert_ne!(child_id, "client-supplied");
        assert!(repo.get_series_child("client-supplied").await.is_err());
    }

    /// `sort_order` is the authored order and is what every screen renders, so the repo —
    /// not each caller — is responsible for applying it.
    #[tokio::test]
    async fn list_series_children_is_ordered_by_sort_order() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();

        for (name, sort_order) in [("third", 2), ("first", 0), ("second", 1)] {
            let mut child = sample_child(&series_id, name, 7);
            child.sort_order = sort_order;
            repo.create_series_child(&child).await.unwrap();
        }

        let names: Vec<String> = repo
            .list_series_children(&series_id)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn list_series_children_is_scoped_to_its_own_series() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series1 = repo.create_series(&sample_series("p1")).await.unwrap();
        let series2 = repo.create_series(&sample_series("p1")).await.unwrap();
        repo.create_series_child(&sample_child(&series1, "mine", 7))
            .await
            .unwrap();
        repo.create_series_child(&sample_child(&series2, "theirs", 7))
            .await
            .unwrap();

        let children = repo.list_series_children(&series1).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "mine");
    }

    #[tokio::test]
    async fn list_series_children_is_empty_for_a_series_with_none() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();

        assert!(
            repo.list_series_children(&series_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// `series_id` is immutable, mirroring `update_series`' treatment of `project_id`.
    #[tokio::test]
    async fn update_series_child_replaces_fields_but_not_the_owning_series() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let other_series = repo.create_series(&sample_series("p1")).await.unwrap();
        let child_id = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();

        let mut updated = sample_child(&other_series, "Book the venue", 21);
        updated.priority = Some(1);
        updated.sort_order = 3;
        repo.update_series_child(&child_id, &updated).await.unwrap();

        let stored = repo.get_series_child(&child_id).await.unwrap();
        assert_eq!(stored.name, "Book the venue");
        assert_eq!(stored.days_before, 21);
        assert_eq!(stored.priority, Some(1));
        assert_eq!(stored.sort_order, 3);
        assert_eq!(stored.series_id, series_id);
    }

    #[tokio::test]
    async fn update_and_delete_series_child_report_not_found() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);

        let child = sample_child("s1", "nope", 1);
        assert!(repo.update_series_child("missing", &child).await.is_err());
        assert!(repo.delete_series_child("missing").await.is_err());
    }

    #[tokio::test]
    async fn delete_series_child_also_removes_its_occurrence_rows() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let child_id = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&child_id, dt(1_000_000), "item-1")
            .await
            .unwrap();

        repo.delete_series_child(&child_id).await.unwrap();

        assert!(repo.get_series_child(&child_id).await.is_err());
        assert!(
            repo.get_child_occurrence(&child_id, dt(1_000_000))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_series_children_for_series_clears_definitions_and_occurrences() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let other_series = repo.create_series(&sample_series("p1")).await.unwrap();
        let child_id = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();
        let other_child = repo
            .create_series_child(&sample_child(&other_series, "Untouched", 7))
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&child_id, dt(1_000_000), "item-1")
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&other_child, dt(1_000_000), "item-2")
            .await
            .unwrap();

        repo.delete_series_children_for_series(&series_id)
            .await
            .unwrap();

        assert!(
            repo.list_series_children(&series_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.get_child_occurrence(&child_id, dt(1_000_000))
                .await
                .unwrap()
                .is_none()
        );
        // The other series' sub-item and its occurrence row are untouched.
        assert_eq!(
            repo.list_series_children(&other_series)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            repo.get_child_occurrence(&other_child, dt(1_000_000))
                .await
                .unwrap()
                .is_some()
        );
    }

    /// A series with no sub-items is the norm, so the cascade must not report not-found.
    #[tokio::test]
    async fn delete_series_children_for_series_is_a_noop_when_there_are_none() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();

        repo.delete_series_children_for_series(&series_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn child_occurrence_round_trips_and_is_none_while_virtual() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let child_id = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();

        assert!(
            repo.get_child_occurrence(&child_id, dt(1_000_000))
                .await
                .unwrap()
                .is_none()
        );

        repo.record_materialized_child_occurrence(&child_id, dt(1_000_000), "item-1")
            .await
            .unwrap();

        let occurrence = repo
            .get_child_occurrence(&child_id, dt(1_000_000))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(occurrence.child_id, child_id);
        assert_eq!(occurrence.occurrence_date, dt(1_000_000));
        assert_eq!(occurrence.item_id, "item-1");
    }

    /// Re-materializing the same cycle must overwrite rather than fail — the upsert
    /// mirrors `record_materialized_occurrence`'s own conflict handling.
    #[tokio::test]
    async fn recording_a_child_occurrence_twice_overwrites_the_item_id() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let child_id = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();

        repo.record_materialized_child_occurrence(&child_id, dt(1_000_000), "item-1")
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&child_id, dt(1_000_000), "item-2")
            .await
            .unwrap();

        let occurrence = repo
            .get_child_occurrence(&child_id, dt(1_000_000))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(occurrence.item_id, "item-2");
    }

    /// One definition materializes independently per parent cycle.
    #[tokio::test]
    async fn list_child_occurrences_for_series_spans_definitions_and_filters_by_range() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let other_series = repo.create_series(&sample_series("p1")).await.unwrap();
        let venue = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();
        let invites = repo
            .create_series_child(&sample_child(&series_id, "Send invites", 14))
            .await
            .unwrap();
        let elsewhere = repo
            .create_series_child(&sample_child(&other_series, "Elsewhere", 7))
            .await
            .unwrap();

        repo.record_materialized_child_occurrence(&venue, dt(1_000), "i1")
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&venue, dt(2_000), "i2")
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&invites, dt(2_000), "i3")
            .await
            .unwrap();
        // Out of range, and belongs to another series — neither should surface.
        repo.record_materialized_child_occurrence(&venue, dt(9_000), "i4")
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&elsewhere, dt(2_000), "i5")
            .await
            .unwrap();

        let found = repo
            .list_child_occurrences_for_series(&series_id, dt(1_000), dt(3_000))
            .await
            .unwrap();

        let mut item_ids: Vec<String> = found.into_iter().map(|o| o.item_id).collect();
        item_ids.sort();
        assert_eq!(item_ids, vec!["i1", "i2", "i3"]);
    }

    #[tokio::test]
    async fn delete_child_occurrence_returns_the_cycle_to_virtual_and_tolerates_a_missing_row() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let child_id = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&child_id, dt(1_000_000), "item-1")
            .await
            .unwrap();

        repo.delete_child_occurrence(&child_id, dt(1_000_000))
            .await
            .unwrap();
        assert!(
            repo.get_child_occurrence(&child_id, dt(1_000_000))
                .await
                .unwrap()
                .is_none()
        );

        // The definition itself survives — deleting a materialized sub-item only
        // un-materializes that cycle, it does not remove the sub-item from the series.
        assert!(repo.get_series_child(&child_id).await.is_ok());

        // A second delete is a no-op, not an error.
        repo.delete_child_occurrence(&child_id, dt(1_000_000))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn find_child_occurrence_by_item_id_resolves_the_reverse_lookup() {
        let pool = test_pool().await;
        let repo = SqliteItemSeriesRepo(pool);
        let series_id = repo.create_series(&sample_series("p1")).await.unwrap();
        let child_id = repo
            .create_series_child(&sample_child(&series_id, "Book venue", 30))
            .await
            .unwrap();
        repo.record_materialized_child_occurrence(&child_id, dt(1_000_000), "item-1")
            .await
            .unwrap();

        let found = repo
            .find_child_occurrence_by_item_id("item-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.child_id, child_id);
        assert_eq!(found.occurrence_date, dt(1_000_000));

        // An ordinary item that never came from a series sub-item is a plain None.
        assert!(
            repo.find_child_occurrence_by_item_id("unrelated")
                .await
                .unwrap()
                .is_none()
        );
    }
}
