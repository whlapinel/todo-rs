use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::domain::calendar_subscription::CalendarSubscription;
use crate::storage::sqlite::{
    CalendarSubscriptionRepo, RepoError, db_err, not_found, row_to_calendar_subscription,
};

pub struct SqliteCalendarSubscriptionRepo(pub SqlitePool);

const CALENDAR_SUBSCRIPTION_SELECT: &str = "SELECT id, project_id, ical_url, created_by_user_id, \
     created_at, last_synced_at, last_sync_error FROM calendar_subscriptions";

#[async_trait]
impl CalendarSubscriptionRepo for SqliteCalendarSubscriptionRepo {
    async fn create(
        &self,
        project_id: &str,
        ical_url: &str,
        created_by_user_id: &str,
    ) -> Result<CalendarSubscription, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO calendar_subscriptions \
             (id, project_id, ical_url, created_by_user_id, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(project_id)
        .bind(ical_url)
        .bind(created_by_user_id)
        .bind(created_at)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        self.get(&id).await
    }

    async fn get(&self, id: &str) -> Result<CalendarSubscription, RepoError> {
        let q = format!("{CALENDAR_SUBSCRIPTION_SELECT} WHERE id = ?");
        sqlx::query(&q)
            .bind(id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .as_ref()
            .map(row_to_calendar_subscription)
            .ok_or_else(not_found)
    }

    async fn list_by_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<CalendarSubscription>, RepoError> {
        let q = format!("{CALENDAR_SUBSCRIPTION_SELECT} WHERE project_id = ? ORDER BY created_at ASC");
        sqlx::query(&q)
            .bind(project_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_calendar_subscription).collect())
    }

    async fn delete(&self, id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("DELETE FROM calendar_subscriptions WHERE id = ?")
            .bind(id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn list_all(&self) -> Result<Vec<CalendarSubscription>, RepoError> {
        let q = format!("{CALENDAR_SUBSCRIPTION_SELECT} ORDER BY created_at ASC");
        sqlx::query(&q)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_calendar_subscription).collect())
    }

    async fn record_sync_result(
        &self,
        id: &str,
        synced_at: DateTime<Utc>,
        error: Option<String>,
    ) -> Result<(), RepoError> {
        let rows = sqlx::query(
            "UPDATE calendar_subscriptions SET last_synced_at = ?, last_sync_error = ? \
             WHERE id = ?",
        )
        .bind(synced_at.timestamp())
        .bind(error)
        .bind(id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
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
            "CREATE TABLE calendar_subscriptions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                ical_url TEXT NOT NULL,
                created_by_user_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_synced_at INTEGER,
                last_sync_error TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn create_and_get_round_trip() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);

        let sub = repo
            .create("p1", "https://calendar.google.com/x.ics", "u1")
            .await
            .unwrap();

        assert_eq!(sub.project_id, "p1");
        assert_eq!(sub.ical_url, "https://calendar.google.com/x.ics");
        assert_eq!(sub.created_by_user_id, "u1");
        assert_eq!(sub.last_synced_at, None);
        assert_eq!(sub.last_sync_error, None);

        let fetched = repo.get(&sub.id).await.unwrap();
        assert_eq!(fetched.id, sub.id);
    }

    #[tokio::test]
    async fn get_missing_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);
        let err = repo.get("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn list_by_project_scopes_and_orders_by_created_at() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);
        let a = repo.create("p1", "https://x/a.ics", "u1").await.unwrap();
        let b = repo.create("p1", "https://x/b.ics", "u1").await.unwrap();
        repo.create("p2", "https://x/c.ics", "u1").await.unwrap();

        let subs = repo.list_by_project("p1").await.unwrap();
        let ids: Vec<_> = subs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec![a.id.as_str(), b.id.as_str()]);
    }

    #[tokio::test]
    async fn list_all_returns_every_subscription_across_projects() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);
        repo.create("p1", "https://x/a.ics", "u1").await.unwrap();
        repo.create("p2", "https://x/b.ics", "u1").await.unwrap();

        let subs = repo.list_all().await.unwrap();
        assert_eq!(subs.len(), 2);
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);
        let sub = repo.create("p1", "https://x/a.ics", "u1").await.unwrap();

        repo.delete(&sub.id).await.unwrap();

        assert!(matches!(
            repo.get(&sub.id).await.unwrap_err(),
            RepoError::NotFound
        ));
    }

    #[tokio::test]
    async fn delete_missing_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);
        let err = repo.delete("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn record_sync_result_success_clears_error() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);
        let sub = repo.create("p1", "https://x/a.ics", "u1").await.unwrap();
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        repo.record_sync_result(&sub.id, now, Some("boom".to_string()))
            .await
            .unwrap();
        let fetched = repo.get(&sub.id).await.unwrap();
        assert_eq!(fetched.last_sync_error.as_deref(), Some("boom"));

        repo.record_sync_result(&sub.id, now, None).await.unwrap();
        let fetched = repo.get(&sub.id).await.unwrap();
        assert_eq!(fetched.last_synced_at, Some(now));
        assert_eq!(fetched.last_sync_error, None);
    }

    #[tokio::test]
    async fn record_sync_result_missing_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteCalendarSubscriptionRepo(pool);
        let err = repo
            .record_sync_result("missing", Utc::now(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }
}
