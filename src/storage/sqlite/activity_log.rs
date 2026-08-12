use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::domain::activity_log::ActivityLogEntry;
use crate::storage::sqlite::{ActivityLogRepo, RepoError, db_err, not_found, row_to_activity_log_entry};

pub struct SqliteActivityLogRepo(pub SqlitePool);

#[async_trait]
impl ActivityLogRepo for SqliteActivityLogRepo {
    async fn log_activity<'a>(
        &'a self,
        team_id: &'a str,
        project_id: Option<&'a str>,
        user_id: &'a str,
        item_id: &'a str,
        item_name: &'a str,
        points_delta: i32,
    ) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, project_id, user_id, item_id, item_name, points_delta, reversed, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&id)
        .bind(team_id)
        .bind(project_id)
        .bind(user_id)
        .bind(item_id)
        .bind(item_name)
        .bind(points_delta)
        .bind(created_at)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn list_activity_for_team(
        &self,
        team_id: &str,
        limit: i64,
    ) -> Result<Vec<ActivityLogEntry>, RepoError> {
        sqlx::query(
            "SELECT id, team_id, project_id, user_id, item_id, item_name, points_delta, reversed, created_at \
             FROM activity_log \
             WHERE team_id = ? \
             ORDER BY created_at DESC \
             LIMIT ?",
        )
        .bind(team_id)
        .bind(limit)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| rows.iter().map(row_to_activity_log_entry).collect())
    }

    async fn list_activity_for_project(
        &self,
        project_id: &str,
        limit: i64,
    ) -> Result<Vec<ActivityLogEntry>, RepoError> {
        sqlx::query(
            "SELECT id, team_id, project_id, user_id, item_id, item_name, points_delta, reversed, created_at \
             FROM activity_log \
             WHERE project_id = ? \
             ORDER BY created_at DESC \
             LIMIT ?",
        )
        .bind(project_id)
        .bind(limit)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| rows.iter().map(row_to_activity_log_entry).collect())
    }

    async fn most_recent_unreversed(
        &self,
        item_id: &str,
        user_id: &str,
    ) -> Result<Option<ActivityLogEntry>, RepoError> {
        sqlx::query(
            "SELECT id, team_id, project_id, user_id, item_id, item_name, points_delta, reversed, created_at \
             FROM activity_log \
             WHERE item_id = ? AND user_id = ? AND reversed = 0 \
             ORDER BY created_at DESC \
             LIMIT 1",
        )
        .bind(item_id)
        .bind(user_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)
        .map(|row| row.as_ref().map(row_to_activity_log_entry))
    }

    async fn get_entry(&self, entry_id: &str) -> Result<ActivityLogEntry, RepoError> {
        sqlx::query(
            "SELECT id, team_id, project_id, user_id, item_id, item_name, points_delta, reversed, created_at \
             FROM activity_log \
             WHERE id = ?",
        )
        .bind(entry_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        .as_ref()
        .map(row_to_activity_log_entry)
        .ok_or_else(not_found)
    }

    async fn mark_reversed(&self, entry_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("UPDATE activity_log SET reversed = 1 WHERE id = ? AND reversed = 0")
            .bind(entry_id)
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
            "CREATE TABLE activity_log (
                id TEXT PRIMARY KEY,
                team_id TEXT NOT NULL,
                project_id TEXT,
                user_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                item_name TEXT NOT NULL,
                points_delta INTEGER NOT NULL,
                reversed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn list_activity_for_team_orders_newest_first_and_respects_limit() {
        let pool = test_pool().await;
        let repo = SqliteActivityLogRepo(pool.clone());

        for (i, delta) in [1, 2, 3].into_iter().enumerate() {
            sqlx::query(
                "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, created_at) \
                 VALUES (?, 't1', 'u1', 'i1', 'Item', ?, ?)",
            )
            .bind(format!("e{i}"))
            .bind(delta)
            .bind(100 + i as i64)
            .execute(&pool)
            .await
            .unwrap();
        }

        let entries = repo.list_activity_for_team("t1", 2).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].points_delta, 3);
        assert_eq!(entries[1].points_delta, 2);
    }

    #[tokio::test]
    async fn list_activity_for_team_scopes_to_team() {
        let pool = test_pool().await;
        let repo = SqliteActivityLogRepo(pool.clone());
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, created_at) \
             VALUES ('e1', 't1', 'u1', 'i1', 'Item', 5, 100), \
                    ('e2', 't2', 'u1', 'i2', 'Other', 5, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let entries = repo.list_activity_for_team("t1", 100).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "e1");
    }

    #[tokio::test]
    async fn most_recent_unreversed_skips_already_reversed_rows() {
        let pool = test_pool().await;
        let repo = SqliteActivityLogRepo(pool.clone());
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, reversed, created_at) \
             VALUES ('e1', 't1', 'u1', 'i1', 'Item', 5, 1, 200), \
                    ('e2', 't1', 'u1', 'i1', 'Item', 5, 0, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let entry = repo.most_recent_unreversed("i1", "u1").await.unwrap();
        assert_eq!(entry.unwrap().id, "e2");
    }

    #[tokio::test]
    async fn most_recent_unreversed_returns_none_when_all_reversed() {
        let pool = test_pool().await;
        let repo = SqliteActivityLogRepo(pool.clone());
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, reversed, created_at) \
             VALUES ('e1', 't1', 'u1', 'i1', 'Item', 5, 1, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let entry = repo.most_recent_unreversed("i1", "u1").await.unwrap();
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn mark_reversed_flips_flag_and_rejects_double_reversal() {
        let pool = test_pool().await;
        let repo = SqliteActivityLogRepo(pool.clone());
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, created_at) \
             VALUES ('e1', 't1', 'u1', 'i1', 'Item', 5, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();

        repo.mark_reversed("e1").await.unwrap();
        let err = repo.mark_reversed("e1").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn log_activity_round_trips_through_list() {
        let pool = test_pool().await;
        let repo = SqliteActivityLogRepo(pool.clone());

        let id = repo
            .log_activity("t1", Some("p1"), "u1", "i1", "Wash dishes", 5)
            .await
            .unwrap();

        let entries = repo.list_activity_for_team("t1", 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].project_id.as_deref(), Some("p1"));
        assert_eq!(entries[0].item_name, "Wash dishes");
        assert_eq!(entries[0].points_delta, 5);
        assert!(!entries[0].reversed);
    }

    #[tokio::test]
    async fn list_activity_for_project_scopes_to_project() {
        let pool = test_pool().await;
        let repo = SqliteActivityLogRepo(pool.clone());
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, project_id, user_id, item_id, item_name, points_delta, created_at) \
             VALUES ('e1', 't1', 'p1', 'u1', 'i1', 'Item', 5, 100), \
                    ('e2', 't2', 'p2', 'u1', 'i2', 'Other', 5, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let entries = repo.list_activity_for_project("p1", 100).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "e1");
    }
}
