use async_trait::async_trait;
use sqlx::{SqlitePool, Row};

use crate::domain::{item::Item, list::List, user::User};
use crate::storage::{DueItem, ItemRepo, ListRepo, RepoError, UserRepo};

fn db_err(e: sqlx::Error) -> RepoError {
    RepoError::Internal(e.to_string())
}

fn not_found() -> RepoError {
    RepoError::NotFound
}

pub struct SqliteUserRepo(pub SqlitePool);
pub struct SqliteListRepo(pub SqlitePool);
pub struct SqliteItemRepo(pub SqlitePool);

pub async fn create_pool(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(url).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS lists (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT NOT NULL,
            has_tasks INTEGER NOT NULL DEFAULT 1
        )",
    )
    .execute(&pool)
    .await?;
    let _ = sqlx::query("ALTER TABLE lists ADD COLUMN has_tasks INTEGER NOT NULL DEFAULT 1")
        .execute(&pool)
        .await; // ignored if column already exists
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            list_id TEXT NOT NULL,
            name TEXT NOT NULL,
            deadline INTEGER,
            complete INTEGER DEFAULT 0,
            recurrence TEXT,
            recurrence_basis TEXT,
            has_due_time INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(&pool)
    .await?;
    let _ = sqlx::query("ALTER TABLE items ADD COLUMN has_due_time INTEGER NOT NULL DEFAULT 0")
        .execute(&pool)
        .await;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_list_id ON items (list_id)")
        .execute(&pool)
        .await?;
    Ok(pool)
}

#[async_trait]
impl UserRepo for SqliteUserRepo {
    async fn get(&self, user_id: &str) -> Result<User, RepoError> {
        sqlx::query("SELECT id, first_name, last_name FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| User {
                id: row.get("id"),
                first_name: row.get("first_name"),
                last_name: row.get("last_name"),
            })
            .ok_or_else(not_found)
    }

    async fn list(&self) -> Result<Vec<User>, RepoError> {
        sqlx::query("SELECT id, first_name, last_name FROM users")
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| {
                rows.into_iter()
                    .map(|row| User {
                        id: row.get("id"),
                        first_name: row.get("first_name"),
                        last_name: row.get("last_name"),
                    })
                    .collect()
            })
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
}

#[async_trait]
impl ListRepo for SqliteListRepo {
    async fn get(&self, user_id: &str, list_id: &str) -> Result<List, RepoError> {
        sqlx::query("SELECT id, user_id, name, has_tasks FROM lists WHERE id = ? AND user_id = ?")
            .bind(list_id)
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| List {
                id: row.get("id"),
                user_id: row.get("user_id"),
                name: row.get("name"),
                has_tasks: row.get::<Option<i64>, _>("has_tasks").unwrap_or(1) != 0,
            })
            .ok_or_else(not_found)
    }

    async fn list(&self, user_id: &str) -> Result<Vec<List>, RepoError> {
        sqlx::query("SELECT id, user_id, name, has_tasks FROM lists WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| {
                rows.into_iter()
                    .map(|row| List {
                        id: row.get("id"),
                        user_id: row.get("user_id"),
                        name: row.get("name"),
                        has_tasks: row.get::<Option<i64>, _>("has_tasks").unwrap_or(1) != 0,
                    })
                    .collect()
            })
    }

    async fn create(&self, list: &List) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        let has_tasks: i64 = list.has_tasks as i64;
        sqlx::query("INSERT INTO lists (id, user_id, name, has_tasks) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(&list.user_id)
            .bind(&list.name)
            .bind(has_tasks)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, list: &List) -> Result<(), RepoError> {
        let has_tasks: i64 = list.has_tasks as i64;
        let rows = sqlx::query("UPDATE lists SET name = ?, has_tasks = ? WHERE id = ? AND user_id = ?")
            .bind(&list.name)
            .bind(has_tasks)
            .bind(&list.id)
            .bind(&list.user_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn delete(&self, list_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("DELETE FROM lists WHERE id = ?")
            .bind(list_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }
}

#[async_trait]
impl ItemRepo for SqliteItemRepo {
    async fn get(&self, user_id: &str, list_id: &str, item_id: &str) -> Result<Item, RepoError> {
        sqlx::query(
            "SELECT id, user_id, list_id, name, deadline, complete, recurrence, recurrence_basis, has_due_time
             FROM items WHERE id = ? AND list_id = ? AND user_id = ?",
        )
        .bind(item_id)
        .bind(list_id)
        .bind(user_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        .map(|row| row_to_item(&row))
        .ok_or_else(not_found)
    }

    async fn list(&self, user_id: &str, list_id: &str) -> Result<Vec<Item>, RepoError> {
        sqlx::query(
            "SELECT id, user_id, list_id, name, deadline, complete, recurrence, recurrence_basis, has_due_time
             FROM items WHERE user_id = ? AND list_id = ?
             ORDER BY COALESCE(deadline, 9999999999999) ASC",
        )
        .bind(user_id)
        .bind(list_id)
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
        sqlx::query(
            "INSERT INTO items (id, user_id, list_id, name, deadline, complete, recurrence, recurrence_basis, has_due_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&item.user_id)
        .bind(&item.list_id)
        .bind(&item.name)
        .bind(deadline)
        .bind(complete)
        .bind(&item.recurrence)
        .bind(&item.recurrence_basis)
        .bind(has_due_time)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, item: &Item) -> Result<(), RepoError> {
        let deadline: Option<i64> = item.deadline.map(|dt| dt.timestamp());
        let complete: i64 = item.complete as i64;
        let has_due_time: i64 = item.has_due_time as i64;
        let rows = sqlx::query(
            "UPDATE items SET name = ?, deadline = ?, complete = ?, recurrence = ?, recurrence_basis = ?, has_due_time = ? WHERE id = ? AND user_id = ? AND list_id = ?",
        )
        .bind(&item.name)
        .bind(deadline)
        .bind(complete)
        .bind(&item.recurrence)
        .bind(&item.recurrence_basis)
        .bind(has_due_time)
        .bind(&item.id)
        .bind(&item.user_id)
        .bind(&item.list_id)
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

    async fn delete_by_list(&self, list_id: &str) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM items WHERE list_id = ?")
            .bind(list_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_due(
        &self,
        user_id: &str,
        deadline_after: Option<i64>,
        deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError> {
        sqlx::query(
            "SELECT items.id, items.user_id, items.list_id, items.name, items.deadline,
                    items.complete, items.recurrence, items.recurrence_basis, items.has_due_time,
                    lists.name AS list_name
             FROM items JOIN lists ON items.list_id = lists.id
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
                    list_name: row.get("list_name"),
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
        list_id: row.get("list_id"),
        name: row.get("name"),
        deadline: deadline_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        complete: complete.unwrap_or(0) != 0,
        recurrence: row.get("recurrence"),
        recurrence_basis: row.get("recurrence_basis"),
        has_due_time: row.get::<Option<i64>, _>("has_due_time").unwrap_or(0) != 0,
        ..Item::default()
    }
}
