use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use crate::domain::{item::Item};
use crate::storage::sqlite::{DueItem, ItemRepo, RepoError, db_err, not_found, row_to_item};

pub struct SqliteItemRepo(pub SqlitePool);

const ITEM_SELECT: &str =
    "SELECT id, user_id, team_id, parent_item_id, name, due_date, scheduled_date, complete, recurrence, recurrence_basis, has_due_time, has_tasks,
            is_template, due_offset_days, assigned_to_user_id,
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
            "{ITEM_SELECT} FROM items WHERE user_id = ? AND parent_item_id IS NULL AND is_template = 0 \
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
        let due_date: Option<i64> = item.due_date.map(|dt| dt.timestamp());
        let scheduled_date: Option<i64> = item.scheduled_date.map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time as i64;
        let has_tasks: i64 = item.has_tasks as i64;
        let is_template: i64 = item.is_template as i64;
        sqlx::query(
            "INSERT INTO items (id, user_id, team_id, parent_item_id, name, due_date, scheduled_date, complete, recurrence, recurrence_basis, has_due_time, has_tasks, is_template, due_offset_days, assigned_to_user_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&item.user_id)
        .bind(&item.team_id)
        .bind(&item.parent_item_id)
        .bind(&item.name)
        .bind(due_date)
        .bind(scheduled_date)
        .bind(complete)
        .bind(&item.recurrence)
        .bind(&item.recurrence_basis)
        .bind(has_due_time)
        .bind(has_tasks)
        .bind(is_template)
        .bind(item.due_offset_days)
        .bind(&item.assigned_to_user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, item: &Item) -> Result<(), RepoError> {
        let due_date: Option<i64> = item.due_date.map(|dt| dt.timestamp());
        let scheduled_date: Option<i64> = item.scheduled_date.map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time as i64;
        let has_tasks: i64 = item.has_tasks as i64;
        let is_template: i64 = item.is_template as i64;
        let rows = sqlx::query(
            "UPDATE items SET name = ?, due_date = ?, scheduled_date = ?, complete = ?, recurrence = ?, recurrence_basis = ?, \
             has_due_time = ?, has_tasks = ?, parent_item_id = ?, is_template = ?, due_offset_days = ?, assigned_to_user_id = ? \
             WHERE id = ? AND user_id = ?",
        )
        .bind(&item.name)
        .bind(due_date)
        .bind(scheduled_date)
        .bind(complete)
        .bind(&item.recurrence)
        .bind(&item.recurrence_basis)
        .bind(has_due_time)
        .bind(has_tasks)
        .bind(&item.parent_item_id)
        .bind(is_template)
        .bind(item.due_offset_days)
        .bind(&item.assigned_to_user_id)
        .bind(&item.id)
        .bind(&item.user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn update_team_item(&self, item: &Item) -> Result<(), RepoError> {
        let due_date: Option<i64> = item.due_date.map(|dt| dt.timestamp());
        let scheduled_date: Option<i64> = item.scheduled_date.map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time as i64;
        let has_tasks: i64 = item.has_tasks as i64;
        let is_template: i64 = item.is_template as i64;
        let rows = sqlx::query(
            "UPDATE items SET name = ?, due_date = ?, scheduled_date = ?, complete = ?, recurrence = ?, recurrence_basis = ?, \
             has_due_time = ?, has_tasks = ?, parent_item_id = ?, is_template = ?, due_offset_days = ?, assigned_to_user_id = ? \
             WHERE id = ? AND team_id = ?",
        )
        .bind(&item.name)
        .bind(due_date)
        .bind(scheduled_date)
        .bind(complete)
        .bind(&item.recurrence)
        .bind(&item.recurrence_basis)
        .bind(has_due_time)
        .bind(has_tasks)
        .bind(&item.parent_item_id)
        .bind(is_template)
        .bind(item.due_offset_days)
        .bind(&item.assigned_to_user_id)
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
            "SELECT items.id, items.user_id, items.team_id, items.parent_item_id, items.name, items.due_date, items.scheduled_date,
                    items.complete, items.recurrence, items.recurrence_basis, items.has_due_time, items.has_tasks,
                    items.is_template, items.due_offset_days, items.assigned_to_user_id,
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

    async fn list_templates(&self, user_id: &str) -> Result<Vec<Item>, RepoError> {
        let q = format!(
            "{ITEM_SELECT} FROM items WHERE user_id = ? AND is_template = 1 AND parent_item_id IS NULL \
             ORDER BY name ASC"
        );
        sqlx::query(&q)
            .bind(user_id)
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
