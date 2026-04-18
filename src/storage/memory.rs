use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::{item::Item, list::List, user::User};
use crate::storage::RepoError;
use crate::storage::{ItemRepo, ListRepo, UserRepo};

pub struct InMemoryUserRepo {
    users: RwLock<HashMap<String, User>>,
}

pub struct InMemoryListRepo {
    lists: RwLock<HashMap<String, List>>,
}

pub struct InMemoryItemRepo {
    items: RwLock<HashMap<String, Item>>,
}

impl InMemoryUserRepo {
    pub fn new() -> Self {
        Self {
            users: RwLock::new(HashMap::new()),
        }
    }
}

impl InMemoryListRepo {
    pub fn new() -> Self {
        Self {
            lists: RwLock::new(HashMap::new()),
        }
    }
}

impl InMemoryItemRepo {
    pub fn new() -> Self {
        Self {
            items: RwLock::new(HashMap::new()),
        }
    }
}

fn lock_err(e: impl std::fmt::Display) -> RepoError {
    RepoError::Internal(format!("lock poisoned: {e}"))
}

#[async_trait]
impl UserRepo for InMemoryUserRepo {
    async fn get(&self, user_id: &str) -> Result<User, RepoError> {
        self.users
            .read()
            .map_err(lock_err)?
            .get(user_id)
            .cloned()
            .ok_or(RepoError::NotFound)
    }

    async fn list(&self) -> Result<Vec<User>, RepoError> {
        Ok(self
            .users
            .read()
            .map_err(lock_err)?
            .values()
            .cloned()
            .collect())
    }

    async fn create(&self, user: &User) -> Result<String, RepoError> {
        let id = Uuid::new_v4().to_string();
        let mut stored = user.clone();
        stored.id = id.clone();
        self.users
            .write()
            .map_err(lock_err)?
            .insert(id.clone(), stored);
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
        self.users
            .write()
            .map_err(lock_err)?
            .remove(user_id)
            .map(|_| ())
            .ok_or(RepoError::NotFound)
    }
}

#[async_trait]
impl ListRepo for InMemoryListRepo {
    async fn get(&self, user_id: &str, list_id: &str) -> Result<List, RepoError> {
        self.lists
            .read()
            .map_err(lock_err)?
            .get(list_id)
            .filter(|l| l.user_id == user_id)
            .cloned()
            .ok_or(RepoError::NotFound)
    }

    async fn list(&self, user_id: &str) -> Result<Vec<List>, RepoError> {
        Ok(self
            .lists
            .read()
            .map_err(lock_err)?
            .values()
            .filter(|l| l.user_id == user_id)
            .cloned()
            .collect())
    }

    async fn create(&self, list: &List) -> Result<String, RepoError> {
        let id = Uuid::new_v4().to_string();
        let mut stored = list.clone();
        stored.id = id.clone();
        self.lists
            .write()
            .map_err(lock_err)?
            .insert(id.clone(), stored);
        Ok(id)
    }

    async fn update(&self, list: &List) -> Result<(), RepoError> {
        let mut map = self.lists.write().map_err(lock_err)?;
        let entry = map.get_mut(&list.id).ok_or(RepoError::NotFound)?;
        entry.name = list.name.clone();
        Ok(())
    }

    async fn delete(&self, list_id: &str) -> Result<(), RepoError> {
        self.lists
            .write()
            .map_err(lock_err)?
            .remove(list_id)
            .map(|_| ())
            .ok_or(RepoError::NotFound)
    }
}

#[async_trait]
impl ItemRepo for InMemoryItemRepo {
    async fn get(&self, user_id: &str, list_id: &str, item_id: &str) -> Result<Item, RepoError> {
        self.items
            .read()
            .map_err(lock_err)?
            .get(item_id)
            .filter(|i| i.user_id == user_id && i.list_id == list_id)
            .cloned()
            .ok_or(RepoError::NotFound)
    }

    async fn list(&self, user_id: &str, list_id: &str) -> Result<Vec<Item>, RepoError> {
        Ok(self
            .items
            .read()
            .map_err(lock_err)?
            .values()
            .filter(|i| i.user_id == user_id && i.list_id == list_id)
            .cloned()
            .collect())
    }

    async fn create(&self, item: &Item) -> Result<String, RepoError> {
        let id = Uuid::new_v4().to_string();
        let mut stored = item.clone();
        stored.id = id.clone();
        self.items
            .write()
            .map_err(lock_err)?
            .insert(id.clone(), stored);
        Ok(id)
    }

    async fn update(&self, item: &Item) -> Result<(), RepoError> {
        let mut map = self.items.write().map_err(lock_err)?;
        let entry = map.get_mut(&item.id).ok_or(RepoError::NotFound)?;
        entry.name = item.name.clone();
        entry.deadline = item.deadline;
        Ok(())
    }

    async fn delete(&self, item_id: &str) -> Result<(), RepoError> {
        self.items
            .write()
            .map_err(lock_err)?
            .remove(item_id)
            .map(|_| ())
            .ok_or(RepoError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        domain::{item::Item, list::List, user::User},
        storage::{
            ItemRepo, ListRepo, UserRepo,
            memory::{InMemoryItemRepo, InMemoryListRepo, InMemoryUserRepo},
        },
    };

    async fn test_one() {
        let user_repo = InMemoryUserRepo::new();
        let list_repo = InMemoryListRepo::new();
        let item_repo = InMemoryItemRepo::new();

        let user = User::new("Hannah", "Barbara");
        let user_id = user_repo.create(&user).await.unwrap();

        let list = List::new(&user_id, "Groceries");
        let list_id = list_repo.create(&list).await.unwrap();

        let item = Item::new(&user_id, &list_id, "milk");
        let item_id = item_repo.create(&item).await.unwrap();
    }
}
