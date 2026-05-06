use async_trait::async_trait;
use sqlx::{SqlitePool, Row};

use crate::domain::{item::Item, user::User};
use crate::storage::{DueItem, ItemRepo, RepoError, UserRepo};

fn db_err(e: sqlx::Error) -> RepoError {
    RepoError::Internal(e.to_string())
}

fn not_found() -> RepoError {
    RepoError::NotFound
}

pub struct SqliteUserRepo(pub SqlitePool);
pub struct SqliteItemRepo(pub SqlitePool);

pub async fn create_pool(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(url).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            email TEXT,
            google_id TEXT UNIQUE
        )",
    )
    .execute(&pool)
    .await?;
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN email TEXT")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN google_id TEXT UNIQUE")
        .execute(&pool)
        .await;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            parent_item_id TEXT,
            name TEXT NOT NULL,
            deadline INTEGER,
            complete INTEGER DEFAULT 0,
            recurrence TEXT,
            recurrence_basis TEXT,
            has_due_time INTEGER NOT NULL DEFAULT 0,
            has_tasks INTEGER NOT NULL DEFAULT 1
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_user_id ON items (user_id)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_parent_id ON items (parent_item_id)")
        .execute(&pool)
        .await?;
    Ok(pool)
}

#[async_trait]
impl UserRepo for SqliteUserRepo {
    async fn get(&self, user_id: &str) -> Result<User, RepoError> {
        sqlx::query("SELECT id, first_name, last_name, email, google_id FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| row_to_user(&row))
            .ok_or_else(not_found)
    }

    async fn list(&self) -> Result<Vec<User>, RepoError> {
        sqlx::query("SELECT id, first_name, last_name, email, google_id FROM users")
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.into_iter().map(|row| row_to_user(&row)).collect())
    }

    async fn create(&self, user: &User) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO users (id, first_name, last_name) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(&user.first_name)
            .bind(&user.last_name)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, user: &User) -> Result<(), RepoError> {
        let rows = sqlx::query(
            "UPDATE users SET first_name = ?, last_name = ? WHERE id = ?",
        )
        .bind(&user.first_name)
        .bind(&user.last_name)
        .bind(&user.id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn delete(&self, user_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(user_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn get_or_create_by_google_id(
        &self,
        google_id: &str,
        email: &str,
        first_name: &str,
        last_name: &str,
    ) -> Result<User, RepoError> {
        if let Some(row) = sqlx::query(
            "SELECT id, first_name, last_name, email, google_id FROM users WHERE google_id = ?",
        )
        .bind(google_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        {
            return Ok(row_to_user(&row));
        }

        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO users (id, first_name, last_name, email, google_id) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(first_name)
        .bind(last_name)
        .bind(email)
        .bind(google_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;

        Ok(User {
            id,
            first_name: first_name.to_string(),
            last_name: last_name.to_string(),
            email: Some(email.to_string()),
            google_id: Some(google_id.to_string()),
        })
    }
}

fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("id"),
        first_name: row.get("first_name"),
        last_name: row.get("last_name"),
        email: row.get("email"),
        google_id: row.get("google_id"),
    }
}

const ITEM_SELECT: &str =
    "SELECT id, user_id, parent_item_id, name, deadline, complete, recurrence, recurrence_basis, has_due_time, has_tasks,
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
            "{ITEM_SELECT} FROM items WHERE user_id = ? AND parent_item_id IS NULL \
             ORDER BY COALESCE(deadline, 9999999999999) ASC"
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
             ORDER BY COALESCE(deadline, 9999999999999) ASC"
        );
        sqlx::query(&q)
            .bind(parent_item_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_item).collect())
    }

    async fn create(&self, item: &Item) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        let deadline: Option<i64> = item.deadline.map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time as i64;
        let has_tasks: i64 = item.has_tasks as i64;
        sqlx::query(
            "INSERT INTO items (id, user_id, parent_item_id, name, deadline, complete, recurrence, recurrence_basis, has_due_time, has_tasks)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&item.user_id)
        .bind(&item.parent_item_id)
        .bind(&item.name)
        .bind(deadline)
        .bind(complete)
        .bind(&item.recurrence)
        .bind(&item.recurrence_basis)
        .bind(has_due_time)
        .bind(has_tasks)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, item: &Item) -> Result<(), RepoError> {
        let deadline: Option<i64> = item.deadline.map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time as i64;
        let has_tasks: i64 = item.has_tasks as i64;
        let rows = sqlx::query(
            "UPDATE items SET name = ?, deadline = ?, complete = ?, recurrence = ?, recurrence_basis = ?, \
             has_due_time = ?, has_tasks = ?, parent_item_id = ? WHERE id = ? AND user_id = ?",
        )
        .bind(&item.name)
        .bind(deadline)
        .bind(complete)
        .bind(&item.recurrence)
        .bind(&item.recurrence_basis)
        .bind(has_due_time)
        .bind(has_tasks)
        .bind(&item.parent_item_id)
        .bind(&item.id)
        .bind(&item.user_id)
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
        deadline_after: Option<i64>,
        deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError> {
        sqlx::query(
            "SELECT items.id, items.user_id, items.parent_item_id, items.name, items.deadline,
                    items.complete, items.recurrence, items.recurrence_basis, items.has_due_time, items.has_tasks,
                    COALESCE(parent.name, '') AS parent_name,
                    EXISTS(SELECT 1 FROM items c WHERE c.parent_item_id = items.id) AS has_children
             FROM items
             LEFT JOIN items parent ON items.parent_item_id = parent.id
             WHERE items.user_id = ?
               AND (? IS NULL OR items.deadline >= ?)
               AND (? IS NULL OR items.deadline <= ?)
             ORDER BY COALESCE(items.deadline, 9999999999999) ASC",
        )
        .bind(user_id)
        .bind(deadline_after)
        .bind(deadline_after)
        .bind(deadline_before)
        .bind(deadline_before)
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
}

fn row_to_item(row: &sqlx::sqlite::SqliteRow) -> Item {
    let deadline_secs: Option<i64> = row.get("deadline");
    let complete: Option<i64> = row.get("complete");
    Item {
        id: row.get("id"),
        user_id: row.get("user_id"),
        parent_item_id: row.get("parent_item_id"),
        name: row.get("name"),
        deadline: deadline_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        complete: complete.unwrap_or(0) != 0,
        recurrence: row.get("recurrence"),
        recurrence_basis: row.get("recurrence_basis"),
        has_due_time: row.get::<Option<i64>, _>("has_due_time").unwrap_or(0) != 0,
        has_tasks: row.get::<Option<i64>, _>("has_tasks").unwrap_or(1) != 0,
        has_children: row.get::<Option<i64>, _>("has_children").unwrap_or(0) != 0,
    }
}
