use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::domain::push_subscription::PushSubscription;
use crate::storage::sqlite::{PushSubscriptionRepo, RepoError, db_err};

pub struct SqlitePushSubscriptionRepo(pub SqlitePool);

const PUSH_SUBSCRIPTION_SELECT: &str =
    "SELECT id, user_id, endpoint, p256dh_key, auth_key, created_at FROM push_subscriptions";

fn row_to_push_subscription(row: &sqlx::sqlite::SqliteRow) -> PushSubscription {
    let created_at: i64 = row.get("created_at");
    PushSubscription {
        id: row.get("id"),
        user_id: row.get("user_id"),
        endpoint: row.get("endpoint"),
        p256dh_key: row.get("p256dh_key"),
        auth_key: row.get("auth_key"),
        created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_default(),
    }
}

#[async_trait]
impl PushSubscriptionRepo for SqlitePushSubscriptionRepo {
    async fn create_or_update(
        &self,
        user_id: &str,
        endpoint: &str,
        p256dh_key: &str,
        auth_key: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO push_subscriptions \
             (id, user_id, endpoint, p256dh_key, auth_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(endpoint) DO UPDATE SET \
             user_id = excluded.user_id, p256dh_key = excluded.p256dh_key, \
             auth_key = excluded.auth_key",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(endpoint)
        .bind(p256dh_key)
        .bind(auth_key)
        .bind(Utc::now().timestamp())
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_by_endpoint(&self, endpoint: &str) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = ?")
            .bind(endpoint)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_for_user(&self, user_id: &str) -> Result<Vec<PushSubscription>, RepoError> {
        let q = format!("{PUSH_SUBSCRIPTION_SELECT} WHERE user_id = ? ORDER BY created_at ASC");
        sqlx::query(&q)
            .bind(user_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_push_subscription).collect())
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
            "CREATE TABLE push_subscriptions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                endpoint TEXT NOT NULL UNIQUE,
                p256dh_key TEXT NOT NULL,
                auth_key TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn create_or_update_upserts_on_endpoint_conflict() {
        let pool = test_pool().await;
        let repo = SqlitePushSubscriptionRepo(pool);

        repo.create_or_update("user1", "https://push.example/e1", "p256dh-a", "auth-a")
            .await
            .unwrap();
        repo.create_or_update("user1", "https://push.example/e1", "p256dh-b", "auth-b")
            .await
            .unwrap();

        let subs = repo.list_for_user("user1").await.unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].p256dh_key, "p256dh-b");
        assert_eq!(subs[0].auth_key, "auth-b");
    }

    #[tokio::test]
    async fn list_for_user_only_returns_that_users_subscriptions() {
        let pool = test_pool().await;
        let repo = SqlitePushSubscriptionRepo(pool);

        repo.create_or_update("user1", "https://push.example/e1", "p1", "a1")
            .await
            .unwrap();
        repo.create_or_update("user2", "https://push.example/e2", "p2", "a2")
            .await
            .unwrap();

        let subs = repo.list_for_user("user1").await.unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].endpoint, "https://push.example/e1");
    }

    #[tokio::test]
    async fn delete_by_endpoint_removes_it() {
        let pool = test_pool().await;
        let repo = SqlitePushSubscriptionRepo(pool);

        repo.create_or_update("user1", "https://push.example/e1", "p1", "a1")
            .await
            .unwrap();
        repo.delete_by_endpoint("https://push.example/e1")
            .await
            .unwrap();

        assert!(repo.list_for_user("user1").await.unwrap().is_empty());
    }
}
