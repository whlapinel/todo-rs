use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::{item::Item, user::User};
use crate::storage::RepoError;
use crate::storage::{DueItem, ItemRepo, UserRepo};

pub struct InMemoryUserRepo {
    users: RwLock<HashMap<String, User>>,
}

pub struct InMemoryItemRepo {
    items: RwLock<HashMap<String, Item>>,
}

impl InMemoryUserRepo {
    pub fn new() -> Self {
        Self { users: RwLock::new(HashMap::new()) }
    }
}

impl InMemoryItemRepo {
    pub fn new() -> Self {
        Self { items: RwLock::new(HashMap::new()) }
    }
}

fn lock_err(e: impl std::fmt::Display) -> RepoError {
    RepoError::Internal(format!("lock poisoned: {e}"))
}

#[async_trait]
impl UserRepo for InMemoryUserRepo {
    async fn get(&self, user_id: &str) -> Result<User, RepoError> {
        self.users.read().map_err(lock_err)?.get(user_id).cloned().ok_or(RepoError::NotFound)
    }

    async fn list(&self) -> Result<Vec<User>, RepoError> {
        Ok(self.users.read().map_err(lock_err)?.values().cloned().collect())
    }

    async fn create(&self, user: &User) -> Result<String, RepoError> {
        let id = Uuid::new_v4().to_string();
        let mut stored = user.clone();
        stored.id = id.clone();
        self.users.write().map_err(lock_err)?.insert(id.clone(), stored);
        Ok(id)
    }

    async fn update(&self, user: &User) -> Result<(), RepoError> {
        let mut map = self.users.write().map_err(lock_err)?;
        let entry = map.get_mut(&user.id).ok_or(RepoError::NotFound)?;
        entry.first_name = user.first_name.clone();
        entry.last_name = user.last_name.clone();
        Ok(())
    }

    async fn delete(&self, user_id: &str) -> Result<(), RepoError> {
        self.users.write().map_err(lock_err)?.remove(user_id).map(|_| ()).ok_or(RepoError::NotFound)
    }

    async fn get_or_create_by_google_id(
        &self,
        google_id: &str,
        email: &str,
        first_name: &str,
        last_name: &str,
    ) -> Result<User, RepoError> {
        let existing = {
            let map = self.users.read().map_err(lock_err)?;
            map.values().find(|u| u.google_id.as_deref() == Some(google_id)).cloned()
        };
        if let Some(user) = existing {
            return Ok(user);
        }
        let id = Uuid::new_v4().to_string();
        let user = User {
            id: id.clone(),
            first_name: first_name.to_string(),
            last_name: last_name.to_string(),
            email: Some(email.to_string()),
            google_id: Some(google_id.to_string()),
        };
        self.users.write().map_err(lock_err)?.insert(id, user.clone());
        Ok(user)
    }
}

#[async_trait]
impl ItemRepo for InMemoryItemRepo {
    async fn get(&self, user_id: &str, item_id: &str) -> Result<Item, RepoError> {
        self.items.read().map_err(lock_err)?.get(item_id)
            .filter(|i| i.user_id == user_id)
            .cloned()
            .ok_or(RepoError::NotFound)
    }

    async fn list(&self, user_id: &str) -> Result<Vec<Item>, RepoError> {
        Ok(self.items.read().map_err(lock_err)?.values()
            .filter(|i| i.user_id == user_id && i.parent_item_id.is_none())
            .cloned()
            .collect())
    }

    async fn list_children(&self, parent_item_id: &str) -> Result<Vec<Item>, RepoError> {
        Ok(self.items.read().map_err(lock_err)?.values()
            .filter(|i| i.parent_item_id.as_deref() == Some(parent_item_id))
            .cloned()
            .collect())
    }

    async fn create(&self, item: &Item) -> Result<String, RepoError> {
        let id = Uuid::new_v4().to_string();
        let mut stored = item.clone();
        stored.id = id.clone();
        self.items.write().map_err(lock_err)?.insert(id.clone(), stored);
        Ok(id)
    }

    async fn update(&self, item: &Item) -> Result<(), RepoError> {
        let mut map = self.items.write().map_err(lock_err)?;
        let entry = map.get_mut(&item.id).ok_or(RepoError::NotFound)?;
        entry.name = item.name.clone();
        entry.deadline = item.deadline;
        entry.complete = item.complete;
        entry.recurrence = item.recurrence.clone();
        entry.recurrence_basis = item.recurrence_basis.clone();
        entry.has_due_time = item.has_due_time;
        entry.has_tasks = item.has_tasks;
        entry.parent_item_id = item.parent_item_id.clone();
        Ok(())
    }

    async fn delete(&self, item_id: &str) -> Result<(), RepoError> {
        self.items.write().map_err(lock_err)?.remove(item_id).map(|_| ()).ok_or(RepoError::NotFound)
    }

    async fn list_due(
        &self,
        _user_id: &str,
        _deadline_after: Option<i64>,
        _deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError> {
        Ok(vec![])
    }
}
