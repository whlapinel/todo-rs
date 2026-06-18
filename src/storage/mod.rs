pub mod memory;
pub mod sqlite;
use async_trait::async_trait;
pub mod dynamo;

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
    async fn get_or_create_by_email(&self, email: &str) -> Result<User, RepoError>;
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
