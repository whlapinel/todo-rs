pub mod memory;
pub mod sqlite;
use async_trait::async_trait;
pub mod dynamo;

use crate::domain::{item::Item, list::List, user::User};

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
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ListRepo: Send + Sync {
    async fn get(&self, user_id: &str, list_id: &str) -> Result<List, RepoError>;
    async fn list(&self, user_id: &str) -> Result<Vec<List>, RepoError>;
    async fn create(&self, list: &List) -> Result<String, RepoError>;
    async fn update(&self, list: &List) -> Result<(), RepoError>;
    async fn delete(&self, list_id: &str) -> Result<(), RepoError>;
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ItemRepo: Send + Sync {
    async fn get(&self, user_id: &str, list_id: &str, item_id: &str) -> Result<Item, RepoError>;
    async fn list(&self, user_id: &str, list_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn create(&self, item: &Item) -> Result<String, RepoError>;
    async fn update(&self, item: &Item) -> Result<(), RepoError>;
    async fn delete(&self, item_id: &str) -> Result<(), RepoError>;
}
