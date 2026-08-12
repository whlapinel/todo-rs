use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use crate::domain::{item::Item};
use crate::storage::sqlite::{DueItem, ItemRepo, RepoError, db_err, not_found, row_to_item};

pub struct SqliteItemRepo(pub SqlitePool);

const ITEM_SELECT: &str =
    "SELECT id, user_id, team_id, project_id, parent_item_id, name, description, due_date, scheduled_date, scheduled_end_date, complete, recurrence, recurrence_basis,
            has_due_time, has_scheduled_time, has_end_time,
            item_type, event_type, due_offset_days, assigned_to_user_id, points, source_event_id,
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

    async fn get_team_item(&self, team_id: &str, item_id: &str) -> Result<Item, RepoError> {
        let q = format!("{ITEM_SELECT} FROM items WHERE id = ? AND team_id = ?");
        sqlx::query(&q)
            .bind(item_id)
            .bind(team_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| row_to_item(&row))
            .ok_or_else(not_found)
    }

    async fn list_team_items(
        &self,
        team_id: &str,
        parent_item_id: Option<String>,
    ) -> Result<Vec<Item>, RepoError> {
        let q = if parent_item_id.is_some() {
            format!(
                "{ITEM_SELECT} FROM items WHERE team_id = ? AND parent_item_id = ? \
                 ORDER BY COALESCE(due_date, 9999999999999) ASC"
            )
        } else {
            format!(
                "{ITEM_SELECT} FROM items WHERE team_id = ? AND parent_item_id IS NULL \
                 ORDER BY COALESCE(due_date, 9999999999999) ASC"
            )
        };
        let query = sqlx::query(&q).bind(team_id);
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
            "INSERT INTO items (id, user_id, team_id, project_id, parent_item_id, name, description, due_date, scheduled_date, scheduled_end_date, complete, recurrence, recurrence_basis, has_due_time, has_scheduled_time, has_end_time, item_type, event_type, due_offset_days, assigned_to_user_id, points, source_event_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&item.user_id)
        .bind(&item.team_id)
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
             has_due_time = ?, has_scheduled_time = ?, has_end_time = ?, parent_item_id = ?, item_type = ?, event_type = ?, due_offset_days = ?, assigned_to_user_id = ?, source_event_id = ?, project_id = ? \
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
        .bind(&item.id)
        .bind(&item.user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn update_team_item(&self, item: &Item) -> Result<(), RepoError> {
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
             has_due_time = ?, has_scheduled_time = ?, has_end_time = ?, parent_item_id = ?, item_type = ?, event_type = ?, due_offset_days = ?, assigned_to_user_id = ?, points = ?, source_event_id = ?, project_id = ? \
             WHERE id = ? AND team_id = ?",
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
        .bind(&item.project_id)
        .bind(&item.id)
        .bind(&item.team_id)
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
            "SELECT items.id, items.user_id, items.team_id, items.project_id, items.parent_item_id, items.name, items.description, items.due_date, items.scheduled_date, items.scheduled_end_date,
                    items.complete, items.recurrence, items.recurrence_basis, items.has_due_time, items.has_scheduled_time, items.has_end_time,
                    items.item_type, items.event_type, items.due_offset_days, items.assigned_to_user_id, items.points, items.source_event_id,
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

    async fn list_due_team_items(
        &self,
        team_id: &str,
        due_date_after: Option<i64>,
        due_date_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError> {
        sqlx::query(
            "SELECT items.id, items.user_id, items.team_id, items.project_id, items.parent_item_id, items.name, items.description, items.due_date, items.scheduled_date, items.scheduled_end_date,
                    items.complete, items.recurrence, items.recurrence_basis, items.has_due_time, items.has_scheduled_time, items.has_end_time,
                    items.item_type, items.event_type, items.due_offset_days, items.assigned_to_user_id, items.points, items.source_event_id,
                    COALESCE(parent.name, '') AS parent_name,
                    EXISTS(SELECT 1 FROM items c WHERE c.parent_item_id = items.id) AS has_children
             FROM items
             LEFT JOIN items parent ON items.parent_item_id = parent.id
             WHERE items.team_id = ?
               AND (? IS NULL OR items.due_date >= ?)
               AND (? IS NULL OR items.due_date <= ?)
             ORDER BY COALESCE(items.due_date, 9999999999999) ASC",
        )
        .bind(team_id)
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

    async fn list_team_templates(&self, team_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE team_id = ? AND item_type = 'TEMPLATE' AND parent_item_id IS NULL \
             ORDER BY name ASC"
        );
        sqlx::query(&q)
            .bind(team_id)
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
