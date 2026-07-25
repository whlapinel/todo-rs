pub mod teams;
pub mod users;
pub mod items;
use async_trait::async_trait;
use sqlx::{Row, SqlitePool};
use crate::domain::{item::Item, team::Team, user::User};

pub struct DueItem {
    pub item: Item,
    pub parent_name: String,
}

pub struct TeamWithStatus {
    pub team: Team,
    pub status: String,
    pub invited_by_name: Option<String>,
}

pub struct TeamMemberInfo {
    pub user: User,
    pub status: String,
}

#[derive(Debug)]
pub enum RepoError {
    NotFound,
    Internal(String),
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn get(&self, user_id: &str) -> Result<User, RepoError>;
    async fn list(&self) -> Result<Vec<User>, RepoError>;
    async fn create(&self, user: &User) -> Result<String, RepoError>;
    async fn update(&self, user: &User) -> Result<(), RepoError>;
    async fn delete(&self, user_id: &str) -> Result<(), RepoError>;
    async fn get_or_create_by_google_id(
        &self,
        google_id: &str,
        email: &str,
        first_name: &str,
        last_name: &str,
    ) -> Result<User, RepoError>;
    async fn get_or_create_by_email<'a>(
        &'a self,
        email: &'a str,
        name: Option<&'a str>,
    ) -> Result<User, RepoError>;
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ItemRepo: Send + Sync {
    async fn get(&self, user_id: &str, item_id: &str) -> Result<Item, RepoError>;
    async fn get_team_item(&self, team_id: &str, item_id: &str) -> Result<Item, RepoError>;
    async fn list(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn list_team_items(
        &self,
        team_id: &str,
        parent_item_id: Option<String>,
    ) -> Result<Vec<Item>, RepoError>;
    async fn list_children(&self, parent_item_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn create(&self, item: &Item) -> Result<String, RepoError>;
    async fn update(&self, item: &Item) -> Result<(), RepoError>;
    async fn update_team_item(&self, item: &Item) -> Result<(), RepoError>;
    async fn delete(&self, item_id: &str) -> Result<(), RepoError>;
    async fn list_due(
        &self,
        user_id: &str,
        deadline_after: Option<i64>,
        deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError>;
    async fn list_templates(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn list_assigned(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TeamRepo: Send + Sync {
    async fn create(&self, name: &str, creator_user_id: &str) -> Result<String, RepoError>;
    async fn get(&self, team_id: &str) -> Result<Team, RepoError>;
    async fn list_for_user(&self, user_id: &str) -> Result<Vec<TeamWithStatus>, RepoError>;
    async fn list_members(&self, team_id: &str) -> Result<Vec<TeamMemberInfo>, RepoError>;
    async fn member_status(
        &self,
        team_id: &str,
        user_id: &str,
    ) -> Result<Option<String>, RepoError>;
    async fn invite(
        &self,
        team_id: &str,
        invitee_user_id: &str,
        invited_by: &str,
    ) -> Result<(), RepoError>;
    async fn accept(&self, team_id: &str, user_id: &str) -> Result<(), RepoError>;
    async fn remove_member(&self, team_id: &str, user_id: &str) -> Result<(), RepoError>;
    async fn share_active_team(&self, user_a: &str, user_b: &str) -> Result<bool, RepoError>;
}

fn db_err(e: sqlx::Error) -> RepoError {
    RepoError::Internal(e.to_string())
}

fn not_found() -> RepoError {
    RepoError::NotFound
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

fn row_to_item(row: &sqlx::sqlite::SqliteRow) -> Item {
    let due_date_secs: Option<i64> = row.get("due_date");
    let scheduled_secs: Option<i64> = row.get("scheduled_date");
    let complete: Option<i64> = row.get("complete");
    Item {
        id: row.get("id"),
        user_id: row.get("user_id"),
        team_id: row.get("team_id"),
        parent_item_id: row.get("parent_item_id"),
        name: row.get("name"),
        due_date: due_date_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        scheduled_date: scheduled_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        complete: complete.unwrap_or(0) != 0,
        recurrence: row.get("recurrence"),
        recurrence_basis: row.get("recurrence_basis"),
        has_due_time: row.get::<Option<i64>, _>("has_due_time").unwrap_or(0) != 0,
        has_tasks: row.get::<Option<i64>, _>("has_tasks").unwrap_or(1) != 0,
        has_children: row.get::<Option<i64>, _>("has_children").unwrap_or(0) != 0,
        is_template: row.get::<Option<i64>, _>("is_template").unwrap_or(0) != 0,
        due_offset_days: row.get("due_offset_days"),
        assigned_to_user_id: row.get("assigned_to_user_id"),
    }
}


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
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY,
            user_id TEXT,
            team_id TEXT,
            parent_item_id TEXT,
            name TEXT NOT NULL,
            due_date INTEGER,
            scheduled_date INTEGER,
            complete INTEGER DEFAULT 0,
            recurrence TEXT,
            recurrence_basis TEXT,
            has_due_time INTEGER NOT NULL DEFAULT 0,
            has_tasks INTEGER NOT NULL DEFAULT 1,
            is_template INTEGER NOT NULL DEFAULT 0,
            due_offset_days INTEGER,
            assigned_to_user_id TEXT
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
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_assigned_to ON items (assigned_to_user_id)")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS teams (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS team_members (
            team_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'PENDING',
            invited_by TEXT,
            PRIMARY KEY (team_id, user_id)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_team_members_user_id ON team_members (user_id)")
        .execute(&pool)
        .await?;
    Ok(pool)
}


