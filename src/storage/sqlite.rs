use async_trait::async_trait;
use sqlx::{SqlitePool, Row};

use crate::domain::{item::Item, list::List, user::User};
use crate::storage::{ItemRepo, ListRepo, RepoError, UserRepo};

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
            name TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            list_id TEXT NOT NULL,
            name TEXT NOT NULL,
            deadline INTEGER
        )",
    )
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
        sqlx::query("SELECT id, user_id, name FROM lists WHERE id = ? AND user_id = ?")
            .bind(list_id)
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| List {
                id: row.get("id"),
                user_id: row.get("user_id"),
                name: row.get("name"),
            })
            .ok_or_else(not_found)
    }

    async fn list(&self, user_id: &str) -> Result<Vec<List>, RepoError> {
        sqlx::query("SELECT id, user_id, name FROM lists WHERE user_id = ?")
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
                    })
                    .collect()
            })
    }

    async fn create(&self, list: &List) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO lists (id, user_id, name) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(&list.user_id)
            .bind(&list.name)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, list: &List) -> Result<(), RepoError> {
        let rows = sqlx::query("UPDATE lists SET name = ? WHERE id = ? AND user_id = ?")
            .bind(&list.name)
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
            "SELECT id, user_id, list_id, name, deadline
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
            "SELECT id, user_id, list_id, name, deadline
             FROM items WHERE user_id = ? AND list_id = ?",
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
        sqlx::query(
            "INSERT INTO items (id, user_id, list_id, name, deadline) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&item.user_id)
        .bind(&item.list_id)
        .bind(&item.name)
        .bind(deadline)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn update(&self, item: &Item) -> Result<(), RepoError> {
        let deadline: Option<i64> = item.deadline.map(|dt| dt.timestamp());
        let rows = sqlx::query(
            "UPDATE items SET name = ?, deadline = ? WHERE id = ? AND user_id = ? AND list_id = ?",
        )
        .bind(&item.name)
        .bind(deadline)
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
}

fn row_to_item(row: &sqlx::sqlite::SqliteRow) -> Item {
    let deadline_secs: Option<i64> = row.get("deadline");
    Item {
        id: row.get("id"),
        user_id: row.get("user_id"),
        list_id: row.get("list_id"),
        name: row.get("name"),
        deadline: deadline_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        ..Item::default()
    }
}
