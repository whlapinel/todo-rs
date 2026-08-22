use crate::domain::user::User;
use crate::storage::sqlite::{RepoError, UserRepo, db_err, not_found, row_to_user};
use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

pub struct SqliteUserRepo(pub SqlitePool);
#[async_trait]
impl UserRepo for SqliteUserRepo {
    async fn get(&self, user_id: &str) -> Result<User, RepoError> {
        sqlx::query(
            "SELECT id, first_name, last_name, email, google_id, timezone FROM users WHERE id = ?",
        )
        .bind(user_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        .map(|row| row_to_user(&row))
        .ok_or_else(not_found)
    }

    async fn list(&self) -> Result<Vec<User>, RepoError> {
        sqlx::query("SELECT id, first_name, last_name, email, google_id, timezone FROM users")
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
            "UPDATE users SET first_name = ?, last_name = ?, timezone = ? WHERE id = ?",
        )
        .bind(&user.first_name)
        .bind(&user.last_name)
        .bind(&user.timezone)
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
            "SELECT id, first_name, last_name, email, google_id, timezone FROM users WHERE google_id = ?",
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
            timezone: None,
        })
    }

    async fn get_or_create_by_email<'a>(
        &'a self,
        email: &'a str,
        name: Option<&'a str>,
    ) -> Result<User, RepoError> {
        if let Some(row) = sqlx::query(
            "SELECT id, first_name, last_name, email, google_id, timezone FROM users WHERE email = ?",
        )
        .bind(email)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        {
            return Ok(row_to_user(&row));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let (first_name, last_name) = name
            .and_then(crate::domain::user::split_display_name)
            .unwrap_or_else(|| {
                (
                    email.split('@').next().unwrap_or(email).to_string(),
                    String::new(),
                )
            });
        sqlx::query("INSERT INTO users (id, first_name, last_name, email) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(&first_name)
            .bind(&last_name)
            .bind(email)
            .execute(&self.0)
            .await
            .map_err(db_err)?;

        Ok(User {
            id,
            first_name,
            last_name,
            email: Some(email.to_string()),
            google_id: None,
            timezone: None,
        })
    }
}
