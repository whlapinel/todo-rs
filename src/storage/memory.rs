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

    async fn get_or_create_by_email<'a>(&'a self, email: &'a str, name: Option<&'a str>) -> Result<User, RepoError> {
        let existing = {
            let map = self.users.read().map_err(lock_err)?;
            map.values().find(|u| u.email.as_deref() == Some(email)).cloned()
        };
        if let Some(user) = existing {
            return Ok(user);
        }
        let id = Uuid::new_v4().to_string();
        let (first_name, last_name) = name
            .and_then(crate::domain::user::split_display_name)
            .unwrap_or_else(|| (email.split('@').next().unwrap_or(email).to_string(), String::new()));
        let user = User {
            id: id.clone(),
            first_name,
            last_name,
            email: Some(email.to_string()),
            google_id: None,
        };
        self.users.write().map_err(lock_err)?.insert(id, user.clone());
        Ok(user)
    }
}

#[async_trait]
impl ItemRepo for InMemoryItemRepo {
    async fn get(&self, user_id: &str, item_id: &str) -> Result<Item, RepoError> {
        self.items.read().map_err(lock_err)?.get(item_id)
            .filter(|i| i.user_id.as_deref() == Some(user_id))
            .cloned()
            .ok_or(RepoError::NotFound)
    }

    async fn get_team_item(&self, team_id: &str, item_id: &str) -> Result<Item, RepoError> {
        self.items.read().map_err(lock_err)?.get(item_id)
            .filter(|i| i.team_id.as_deref() == Some(team_id))
            .cloned()
            .ok_or(RepoError::NotFound)
    }

    async fn list(&self, user_id: &str) -> Result<Vec<Item>, RepoError> {
        Ok(self.items.read().map_err(lock_err)?.values()
            .filter(|i| i.user_id.as_deref() == Some(user_id) && i.parent_item_id.is_none())
            .cloned()
            .collect())
    }

    async fn list_team_items(
        &self,
        team_id: &str,
        parent_item_id: Option<String>,
    ) -> Result<Vec<Item>, RepoError> {
        Ok(self.items.read().map_err(lock_err)?.values()
            .filter(|i| {
                i.team_id.as_deref() == Some(team_id)
                    && i.parent_item_id.as_deref() == parent_item_id.as_deref()
            })
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
        entry.assigned_to_user_id = item.assigned_to_user_id.clone();
        Ok(())
    }

    async fn update_team_item(&self, item: &Item) -> Result<(), RepoError> {
        self.update(item).await
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

    async fn list_templates(&self, _user_id: &str) -> Result<Vec<Item>, RepoError> {
        Ok(vec![])
    }

    async fn list_assigned(&self, _user_id: &str) -> Result<Vec<Item>, RepoError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::Item;

    #[tokio::test]
    async fn create_and_get_user_item() {
        let repo = InMemoryItemRepo::new();
        let item = Item::new_user_item("u1", "Buy milk");
        let id = repo.create(&item).await.unwrap();
        let fetched = repo.get("u1", &id).await.unwrap();
        assert_eq!(fetched.name, "Buy milk");
        assert_eq!(fetched.user_id, Some("u1".to_string()));
    }

    #[tokio::test]
    async fn get_user_item_wrong_user_returns_not_found() {
        let repo = InMemoryItemRepo::new();
        let item = Item::new_user_item("u1", "Secret task");
        let id = repo.create(&item).await.unwrap();
        let result = repo.get("u2", &id).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn create_and_get_team_item() {
        let repo = InMemoryItemRepo::new();
        let item = Item::new_team_item("team1", "Deploy server");
        let id = repo.create(&item).await.unwrap();
        let fetched = repo.get_team_item("team1", &id).await.unwrap();
        assert_eq!(fetched.name, "Deploy server");
        assert_eq!(fetched.team_id, Some("team1".to_string()));
    }

    #[tokio::test]
    async fn get_team_item_wrong_team_returns_not_found() {
        let repo = InMemoryItemRepo::new();
        let item = Item::new_team_item("team1", "task");
        let id = repo.create(&item).await.unwrap();
        let result = repo.get_team_item("team2", &id).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn list_team_items_filters_by_team() {
        let repo = InMemoryItemRepo::new();
        repo.create(&Item::new_team_item("team1", "A")).await.unwrap();
        repo.create(&Item::new_team_item("team1", "B")).await.unwrap();
        repo.create(&Item::new_team_item("team2", "C")).await.unwrap();

        let team1_items = repo.list_team_items("team1", None).await.unwrap();
        assert_eq!(team1_items.len(), 2);
        assert!(team1_items.iter().all(|i| i.team_id.as_deref() == Some("team1")));
    }

    #[tokio::test]
    async fn delete_removes_item() {
        let repo = InMemoryItemRepo::new();
        let item = Item::new_user_item("u1", "task");
        let id = repo.create(&item).await.unwrap();
        repo.delete(&id).await.unwrap();
        assert!(matches!(repo.get("u1", &id).await, Err(RepoError::NotFound)));
    }
}
