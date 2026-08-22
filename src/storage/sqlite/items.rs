use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use crate::domain::item::Item;
use crate::storage::sqlite::{DueItem, ItemRepo, RepoError, db_err, not_found, row_to_item};

pub struct SqliteItemRepo(pub SqlitePool);

const ITEM_SELECT: &str =
    "SELECT id, user_id, project_id, parent_item_id, name, description, due_date, scheduled_date, scheduled_end_date, complete, recurrence, recurrence_basis,
            has_due_time, has_scheduled_time, has_end_time,
            item_type, event_type, due_offset_days, assigned_to_user_id, points, source_event_id, series_id,
            google_event_id, calendar_subscription_id,
            EXISTS(SELECT 1 FROM items c WHERE c.parent_item_id = items.id) AS has_children";

#[async_trait]
impl ItemRepo for SqliteItemRepo {
    async fn get(&self, user_id: &str, item_id: &str) -> Result<Item, RepoError> {
        let q = format!("{ITEM_SELECT} FROM items WHERE id = ? AND user_id = ?");
        sqlx::query(&q)
            .bind(item_id)
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| row_to_item(&row))
            .ok_or_else(not_found)
    }

    async fn list(&self, user_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE user_id = ? AND parent_item_id IS NULL AND item_type != 'TEMPLATE' \
             ORDER BY COALESCE(due_date, 9999999999999) ASC"
        );
        sqlx::query(&q)
            .bind(user_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn list_children(&self, parent_item_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE parent_item_id = ? \
             ORDER BY COALESCE(due_date, 9999999999999) ASC"
        );
        sqlx::query(&q)
            .bind(parent_item_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn list_by_source_event(&self, source_event_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE source_event_id = ? \
             ORDER BY COALESCE(due_date, 9999999999999) ASC"
        );
        sqlx::query(&q)
            .bind(source_event_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn list_by_calendar_subscription(
        &self,
        calendar_subscription_id: &str,
    ) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE calendar_subscription_id = ? \
             ORDER BY COALESCE(due_date, 9999999999999) ASC"
        );
        sqlx::query(&q)
            .bind(calendar_subscription_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn get_by_project(&self, project_id: &str, item_id: &str) -> Result<Item, RepoError> {
        let q = format!("{ITEM_SELECT} FROM items WHERE id = ? AND project_id = ?");
        sqlx::query(&q)
            .bind(item_id)
            .bind(project_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| row_to_item(&row))
            .ok_or_else(not_found)
    }

    async fn list_by_project(
        &self,
        project_id: &str,
        parent_item_id: Option<String>,
    ) -> Result<Vec<Item>, RepoError> {
        let q = if parent_item_id.is_some() {
            format!(
                "{ITEM_SELECT} FROM items WHERE project_id = ? AND parent_item_id = ? \
                 ORDER BY COALESCE(due_date, 9999999999999) ASC"
            )
        } else {
            format!(
                "{ITEM_SELECT} FROM items WHERE project_id = ? AND parent_item_id IS NULL \
                 ORDER BY COALESCE(due_date, 9999999999999) ASC"
            )
        };
        let query = sqlx::query(&q).bind(project_id);
        let query = if let Some(pid) = parent_item_id {
            query.bind(pid)
        } else {
            query
        };
        query
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn create(&self, item: &Item) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        let due_date: Option<i64> = item.due_date().map(|dt| dt.timestamp());
        let scheduled_date: Option<i64> = item.scheduled_date().map(|dt| dt.timestamp());
        let scheduled_end_date: Option<i64> = item.scheduled_end_date().map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time() as i64;
        let has_scheduled_time: i64 = item.has_scheduled_time() as i64;
        let has_end_time: i64 = item.has_end_time() as i64;
        let item_type: &str = item.kind().as_str();
        sqlx::query(
            "INSERT INTO items (id, user_id, project_id, parent_item_id, name, description, due_date, scheduled_date, scheduled_end_date, complete, recurrence, recurrence_basis, has_due_time, has_scheduled_time, has_end_time, item_type, event_type, due_offset_days, assigned_to_user_id, points, source_event_id, series_id, google_event_id, calendar_subscription_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&item.user_id)
        .bind(&item.project_id)
        .bind(&item.parent_item_id)
        .bind(&item.name)
        .bind(&item.description)
        .bind(due_date)
        .bind(scheduled_date)
        .bind(scheduled_end_date)
        .bind(complete)
        .bind(item.recurrence_pattern())
        .bind(item.recurrence_basis())
        .bind(has_due_time)
        .bind(has_scheduled_time)
        .bind(has_end_time)
        .bind(item_type)
        .bind(item.event_type())
        .bind(item.due_offset_days())
        .bind(item.assigned_to_user_id())
        .bind(item.points())
        .bind(item.source_event_id())
        .bind(&item.series_id)
        .bind(&item.google_event_id)
        .bind(&item.calendar_subscription_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, item: &Item) -> Result<(), RepoError> {
        let due_date: Option<i64> = item.due_date().map(|dt| dt.timestamp());
        let scheduled_date: Option<i64> = item.scheduled_date().map(|dt| dt.timestamp());
        let scheduled_end_date: Option<i64> = item.scheduled_end_date().map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time() as i64;
        let has_scheduled_time: i64 = item.has_scheduled_time() as i64;
        let has_end_time: i64 = item.has_end_time() as i64;
        let item_type: &str = item.kind().as_str();
        let rows = sqlx::query(
            "UPDATE items SET name = ?, description = ?, due_date = ?, scheduled_date = ?, scheduled_end_date = ?, complete = ?, recurrence = ?, recurrence_basis = ?, \
             has_due_time = ?, has_scheduled_time = ?, has_end_time = ?, parent_item_id = ?, item_type = ?, event_type = ?, due_offset_days = ?, assigned_to_user_id = ?, source_event_id = ?, project_id = ?, series_id = ?, google_event_id = ?, calendar_subscription_id = ? \
             WHERE id = ? AND user_id = ?",
        )
        .bind(&item.name)
        .bind(&item.description)
        .bind(due_date)
        .bind(scheduled_date)
        .bind(scheduled_end_date)
        .bind(complete)
        .bind(item.recurrence_pattern())
        .bind(item.recurrence_basis())
        .bind(has_due_time)
        .bind(has_scheduled_time)
        .bind(has_end_time)
        .bind(&item.parent_item_id)
        .bind(item_type)
        .bind(item.event_type())
        .bind(item.due_offset_days())
        .bind(item.assigned_to_user_id())
        .bind(item.source_event_id())
        .bind(&item.project_id)
        .bind(&item.series_id)
        .bind(&item.google_event_id)
        .bind(&item.calendar_subscription_id)
        .bind(&item.id)
        .bind(&item.user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn update_by_project(&self, item: &Item) -> Result<(), RepoError> {
        let due_date: Option<i64> = item.due_date().map(|dt| dt.timestamp());
        let scheduled_date: Option<i64> = item.scheduled_date().map(|dt| dt.timestamp());
        let scheduled_end_date: Option<i64> = item.scheduled_end_date().map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time() as i64;
        let has_scheduled_time: i64 = item.has_scheduled_time() as i64;
        let has_end_time: i64 = item.has_end_time() as i64;
        let item_type: &str = item.kind().as_str();
        let rows = sqlx::query(
            "UPDATE items SET name = ?, description = ?, due_date = ?, scheduled_date = ?, scheduled_end_date = ?, complete = ?, recurrence = ?, recurrence_basis = ?, \
             has_due_time = ?, has_scheduled_time = ?, has_end_time = ?, parent_item_id = ?, item_type = ?, event_type = ?, due_offset_days = ?, assigned_to_user_id = ?, points = ?, source_event_id = ?, series_id = ?, google_event_id = ?, calendar_subscription_id = ? \
             WHERE id = ? AND project_id = ?",
        )
        .bind(&item.name)
        .bind(&item.description)
        .bind(due_date)
        .bind(scheduled_date)
        .bind(scheduled_end_date)
        .bind(complete)
        .bind(item.recurrence_pattern())
        .bind(item.recurrence_basis())
        .bind(has_due_time)
        .bind(has_scheduled_time)
        .bind(has_end_time)
        .bind(&item.parent_item_id)
        .bind(item_type)
        .bind(item.event_type())
        .bind(item.due_offset_days())
        .bind(item.assigned_to_user_id())
        .bind(item.points())
        .bind(item.source_event_id())
        .bind(&item.series_id)
        .bind(&item.google_event_id)
        .bind(&item.calendar_subscription_id)
        .bind(&item.id)
        .bind(&item.project_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn delete(&self, item_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("DELETE FROM items WHERE id = ?")
            .bind(item_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn list_due(
        &self,
        user_id: &str,
        due_date_after: Option<i64>,
        due_date_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError> {
        sqlx::query(
            "SELECT items.id, items.user_id, items.project_id, items.parent_item_id, items.name, items.description, items.due_date, items.scheduled_date, items.scheduled_end_date,
                    items.complete, items.recurrence, items.recurrence_basis, items.has_due_time, items.has_scheduled_time, items.has_end_time,
                    items.item_type, items.event_type, items.due_offset_days, items.assigned_to_user_id, items.points, items.source_event_id, items.series_id,
                    COALESCE(parent.name, '') AS parent_name,
                    EXISTS(SELECT 1 FROM items c WHERE c.parent_item_id = items.id) AS has_children
             FROM items
             LEFT JOIN items parent ON items.parent_item_id = parent.id
             WHERE (items.user_id = ? OR items.assigned_to_user_id = ?)
               AND (? IS NULL OR items.due_date >= ?)
               AND (? IS NULL OR items.due_date <= ?)
             ORDER BY COALESCE(items.due_date, 9999999999999) ASC",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(due_date_after)
        .bind(due_date_after)
        .bind(due_date_before)
        .bind(due_date_before)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| {
            rows.iter()
                .map(|row| DueItem {
                    item: row_to_item(row),
                    parent_name: row.get("parent_name"),
                })
                .collect()
        })
    }

    async fn list_due_by_project(
        &self,
        project_id: &str,
        due_date_after: Option<i64>,
        due_date_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError> {
        sqlx::query(
            "SELECT items.id, items.user_id, items.project_id, items.parent_item_id, items.name, items.description, items.due_date, items.scheduled_date, items.scheduled_end_date,
                    items.complete, items.recurrence, items.recurrence_basis, items.has_due_time, items.has_scheduled_time, items.has_end_time,
                    items.item_type, items.event_type, items.due_offset_days, items.assigned_to_user_id, items.points, items.source_event_id, items.series_id,
                    COALESCE(parent.name, '') AS parent_name,
                    EXISTS(SELECT 1 FROM items c WHERE c.parent_item_id = items.id) AS has_children
             FROM items
             LEFT JOIN items parent ON items.parent_item_id = parent.id
             WHERE items.project_id = ?
               AND (? IS NULL OR items.due_date >= ?)
               AND (? IS NULL OR items.due_date <= ?)
             ORDER BY COALESCE(items.due_date, 9999999999999) ASC",
        )
        .bind(project_id)
        .bind(due_date_after)
        .bind(due_date_after)
        .bind(due_date_before)
        .bind(due_date_before)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| {
            rows.iter()
                .map(|row| DueItem {
                    item: row_to_item(row),
                    parent_name: row.get("parent_name"),
                })
                .collect()
        })
    }

    async fn list_templates(&self, user_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE user_id = ? AND item_type = 'TEMPLATE' AND parent_item_id IS NULL \
             ORDER BY name ASC"
        );
        sqlx::query(&q)
            .bind(user_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn list_templates_by_project(&self, project_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE project_id = ? AND item_type = 'TEMPLATE' AND parent_item_id IS NULL \
             ORDER BY name ASC"
        );
        sqlx::query(&q)
            .bind(project_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn list_assigned(&self, user_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE assigned_to_user_id = ? \
             ORDER BY COALESCE(due_date, 9999999999999) ASC"
        );
        sqlx::query(&q)
            .bind(user_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }
}

/// See docs/project-abstraction-plan.md, stage B3. `items.rs` previously had no
/// sqlite-level tests of its own (A2's implementation notes flagged this) — this
/// module covers only the new project-scoped methods, following the per-file
/// `test_pool()` precedent `projects.rs`/`activity_log.rs` already established.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{ItemType, Recurrence, Schedule, TeamAssignment};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                parent_item_id TEXT,
                name TEXT NOT NULL,
                description TEXT,
                due_date INTEGER,
                scheduled_date INTEGER,
                scheduled_end_date INTEGER,
                complete INTEGER DEFAULT 0,
                recurrence TEXT,
                recurrence_basis TEXT,
                has_due_time INTEGER NOT NULL DEFAULT 0,
                has_scheduled_time INTEGER NOT NULL DEFAULT 0,
                has_end_time INTEGER NOT NULL DEFAULT 0,
                item_type TEXT NOT NULL DEFAULT 'TASK',
                event_type TEXT,
                due_offset_days INTEGER,
                assigned_to_user_id TEXT,
                points INTEGER,
                source_event_id TEXT,
                project_id TEXT,
                series_id TEXT,
                google_event_id TEXT,
                calendar_subscription_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn item_in_project(project_id: &str, name: &str) -> Item {
        let mut item = Item::new_user_item("u1", name);
        item.project_id = Some(project_id.to_string());
        item
    }

    #[tokio::test]
    async fn get_by_project_finds_item_scoped_to_project() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        let id = repo.create(&item_in_project("p1", "Task 1")).await.unwrap();

        let found = repo.get_by_project("p1", &id).await.unwrap();
        assert_eq!(found.name, "Task 1");
        assert_eq!(found.project_id, Some("p1".to_string()));
    }

    #[tokio::test]
    async fn get_by_project_rejects_item_in_a_different_project() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        let id = repo.create(&item_in_project("p1", "Task 1")).await.unwrap();

        let result = repo.get_by_project("p2", &id).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn list_by_project_returns_only_top_level_items_in_that_project() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        repo.create(&item_in_project("p1", "In project"))
            .await
            .unwrap();
        repo.create(&item_in_project("p2", "Other project"))
            .await
            .unwrap();

        let items = repo.list_by_project("p1", None).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "In project");
    }

    #[tokio::test]
    async fn list_by_project_scopes_to_given_parent() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        let parent_id = repo.create(&item_in_project("p1", "Parent")).await.unwrap();
        let mut child = item_in_project("p1", "Child");
        child.parent_item_id = Some(parent_id.clone());
        repo.create(&child).await.unwrap();

        let top_level = repo.list_by_project("p1", None).await.unwrap();
        assert_eq!(top_level.len(), 1);
        assert_eq!(top_level[0].name, "Parent");

        let children = repo.list_by_project("p1", Some(parent_id)).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "Child");
    }

    #[tokio::test]
    async fn list_templates_by_project_returns_only_templates_in_that_project() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);

        let mut template = item_in_project("p1", "Template 1");
        template.item_type = ItemType::Template {
            schedule: Schedule::default(),
            recurrence: Recurrence::default(),
            event_type: None,
        };
        repo.create(&template).await.unwrap();

        repo.create(&item_in_project("p1", "Not a template"))
            .await
            .unwrap();

        let mut other_project_template = item_in_project("p2", "Other project template");
        other_project_template.item_type = ItemType::Template {
            schedule: Schedule::default(),
            recurrence: Recurrence::default(),
            event_type: None,
        };
        repo.create(&other_project_template).await.unwrap();

        let templates = repo.list_templates_by_project("p1").await.unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Template 1");
    }

    #[tokio::test]
    async fn update_by_project_round_trips_fields_including_points() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        let mut item = item_in_project("p1", "Task 1");
        let id = repo.create(&item).await.unwrap();

        item.id = id.clone();
        item.name = "Renamed".to_string();
        item.item_type = ItemType::Task {
            schedule: Schedule::default(),
            recurrence: Recurrence::default(),
            team_assignment: Some(TeamAssignment {
                assigned_to_user_id: Some("assignee1".to_string()),
                points: Some(5),
            }),
            source_event_id: None,
        };
        repo.update_by_project(&item).await.unwrap();

        let updated = repo.get_by_project("p1", &id).await.unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.points(), Some(5));
        assert_eq!(updated.assigned_to_user_id().as_deref(), Some("assignee1"));
    }

    #[tokio::test]
    async fn series_id_round_trips_through_create_and_update() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        let mut item = item_in_project("p1", "Standup");
        item.series_id = Some("series-1".to_string());
        let id = repo.create(&item).await.unwrap();

        let created = repo.get_by_project("p1", &id).await.unwrap();
        assert_eq!(created.series_id.as_deref(), Some("series-1"));

        item.id = id.clone();
        item.name = "Standup (renamed)".to_string();
        repo.update_by_project(&item).await.unwrap();

        let updated = repo.get_by_project("p1", &id).await.unwrap();
        assert_eq!(updated.series_id.as_deref(), Some("series-1"));
    }

    #[tokio::test]
    async fn list_by_calendar_subscription_scopes_to_that_subscription() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool.clone());

        let mut in_sub = item_in_project("p1", "Dentist");
        in_sub.calendar_subscription_id = Some("sub1".to_string());
        in_sub.google_event_id = Some("evt1".to_string());
        repo.create(&in_sub).await.unwrap();

        let mut other_sub = item_in_project("p1", "Other subscription");
        other_sub.calendar_subscription_id = Some("sub2".to_string());
        other_sub.google_event_id = Some("evt2".to_string());
        repo.create(&other_sub).await.unwrap();

        repo.create(&item_in_project("p1", "Not imported"))
            .await
            .unwrap();

        let items = repo.list_by_calendar_subscription("sub1").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Dentist");
    }

    #[tokio::test]
    async fn google_event_id_and_calendar_subscription_id_round_trip_through_create_and_update() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        let mut item = item_in_project("p1", "Imported Event");
        item.google_event_id = Some("evt-1".to_string());
        item.calendar_subscription_id = Some("sub-1".to_string());
        let id = repo.create(&item).await.unwrap();

        let created = repo.get_by_project("p1", &id).await.unwrap();
        assert_eq!(created.google_event_id.as_deref(), Some("evt-1"));
        assert_eq!(created.calendar_subscription_id.as_deref(), Some("sub-1"));

        item.id = id.clone();
        item.name = "Imported Event (renamed)".to_string();
        repo.update_by_project(&item).await.unwrap();

        let updated = repo.get_by_project("p1", &id).await.unwrap();
        assert_eq!(updated.google_event_id.as_deref(), Some("evt-1"));
        assert_eq!(updated.calendar_subscription_id.as_deref(), Some("sub-1"));
    }

    #[tokio::test]
    async fn list_by_calendar_subscription_returns_empty_for_unknown_subscription() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);

        let items = repo.list_by_calendar_subscription("missing").await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn update_by_project_not_found_for_wrong_project() {
        let pool = test_pool().await;
        let repo = SqliteItemRepo(pool);
        let mut item = item_in_project("p1", "Task 1");
        let id = repo.create(&item).await.unwrap();
        item.id = id;
        item.project_id = Some("p2".to_string());

        let result = repo.update_by_project(&item).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }
}
