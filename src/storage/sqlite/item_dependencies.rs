use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use crate::storage::sqlite::{ItemDependencyRepo, RepoError, db_err};

pub struct SqliteItemDependencyRepo(pub SqlitePool);

#[async_trait]
impl ItemDependencyRepo for SqliteItemDependencyRepo {
    async fn set_dependencies(
        &self,
        item_id: &str,
        depends_on_item_ids: &[String],
    ) -> Result<(), RepoError> {
        let mut tx = self.0.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM item_dependencies WHERE item_id = ?")
            .bind(item_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        for depends_on_item_id in depends_on_item_ids {
            sqlx::query(
                "INSERT INTO item_dependencies (item_id, depends_on_item_id) VALUES (?, ?)",
            )
            .bind(item_id)
            .bind(depends_on_item_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn list_for_item(&self, item_id: &str) -> Result<Vec<String>, RepoError> {
        sqlx::query("SELECT depends_on_item_id FROM item_dependencies WHERE item_id = ?")
            .bind(item_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(|r| r.get("depends_on_item_id")).collect())
    }

    async fn list_dependents(&self, item_id: &str) -> Result<Vec<String>, RepoError> {
        sqlx::query("SELECT item_id FROM item_dependencies WHERE depends_on_item_id = ?")
            .bind(item_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(|r| r.get("item_id")).collect())
    }

    async fn delete_for_item(&self, item_id: &str) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM item_dependencies WHERE item_id = ? OR depends_on_item_id = ?")
            .bind(item_id)
            .bind(item_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE item_dependencies (
                item_id TEXT NOT NULL,
                depends_on_item_id TEXT NOT NULL,
                PRIMARY KEY (item_id, depends_on_item_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn set_dependencies_replaces_the_full_set() {
        let pool = test_pool().await;
        let repo = SqliteItemDependencyRepo(pool);

        repo.set_dependencies("i1", &["i2".to_string(), "i3".to_string()])
            .await
            .unwrap();
        assert_eq!(repo.list_for_item("i1").await.unwrap().len(), 2);

        repo.set_dependencies("i1", &["i4".to_string()])
            .await
            .unwrap();
        assert_eq!(repo.list_for_item("i1").await.unwrap(), vec!["i4"]);

        repo.set_dependencies("i1", &[]).await.unwrap();
        assert!(repo.list_for_item("i1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_dependents_returns_items_that_depend_on_the_given_item() {
        let pool = test_pool().await;
        let repo = SqliteItemDependencyRepo(pool);

        repo.set_dependencies("i2", &["i1".to_string()])
            .await
            .unwrap();
        repo.set_dependencies("i3", &["i1".to_string()])
            .await
            .unwrap();

        let mut dependents = repo.list_dependents("i1").await.unwrap();
        dependents.sort();
        assert_eq!(dependents, vec!["i2".to_string(), "i3".to_string()]);
        assert!(repo.list_dependents("i2").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_for_item_clears_both_sides() {
        let pool = test_pool().await;
        let repo = SqliteItemDependencyRepo(pool);

        // i1 depends on i2, and i3 depends on i1 — deleting i1 should clear both rows.
        repo.set_dependencies("i1", &["i2".to_string()])
            .await
            .unwrap();
        repo.set_dependencies("i3", &["i1".to_string()])
            .await
            .unwrap();

        repo.delete_for_item("i1").await.unwrap();

        assert!(repo.list_for_item("i1").await.unwrap().is_empty());
        assert!(repo.list_for_item("i3").await.unwrap().is_empty());
    }
}
