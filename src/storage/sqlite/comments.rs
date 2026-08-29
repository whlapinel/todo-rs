use async_trait::async_trait;
use chrono::DateTime;
use sqlx::{Row, SqlitePool};

use crate::domain::comment::Comment;
use crate::storage::sqlite::{CommentRepo, RepoError, db_err};

pub struct SqliteCommentRepo(pub SqlitePool);

const COMMENT_SELECT: &str =
    "SELECT id, item_id, project_id, author_user_id, body, created_at FROM comments";

fn row_to_comment(row: &sqlx::sqlite::SqliteRow) -> Comment {
    let created_at: i64 = row.get("created_at");
    Comment {
        id: row.get("id"),
        item_id: row.get("item_id"),
        project_id: row.get("project_id"),
        author_user_id: row.get("author_user_id"),
        body: row.get("body"),
        created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_default(),
    }
}

#[async_trait]
impl CommentRepo for SqliteCommentRepo {
    async fn create(&self, comment: &Comment) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO comments (id, item_id, project_id, author_user_id, body, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&comment.id)
        .bind(&comment.item_id)
        .bind(&comment.project_id)
        .bind(&comment.author_user_id)
        .bind(&comment.body)
        .bind(comment.created_at.timestamp())
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_for_item(&self, item_id: &str) -> Result<Vec<Comment>, RepoError> {
        let q = format!("{COMMENT_SELECT} WHERE item_id = ? ORDER BY created_at ASC");
        sqlx::query(&q)
            .bind(item_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_comment).collect())
    }

    async fn get(&self, id: &str) -> Result<Comment, RepoError> {
        let q = format!("{COMMENT_SELECT} WHERE id = ?");
        let row = sqlx::query(&q)
            .bind(id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .ok_or(RepoError::NotFound)?;
        Ok(row_to_comment(&row))
    }

    async fn update(&self, id: &str, body: &str) -> Result<(), RepoError> {
        let result = sqlx::query("UPDATE comments SET body = ? WHERE id = ?")
            .bind(body)
            .bind(id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), RepoError> {
        let result = sqlx::query("DELETE FROM comments WHERE id = ?")
            .bind(id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE comments (
                id TEXT PRIMARY KEY,
                item_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                author_user_id TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn comment(id: &str, item_id: &str, created_at_secs: i64) -> Comment {
        Comment {
            id: id.to_string(),
            item_id: item_id.to_string(),
            project_id: "p1".to_string(),
            author_user_id: "u1".to_string(),
            body: "hello".to_string(),
            created_at: DateTime::from_timestamp(created_at_secs, 0).unwrap_or_else(Utc::now),
        }
    }

    #[tokio::test]
    async fn create_then_list_roundtrips() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        repo.create(&comment("c1", "i1", 1_000)).await.unwrap();

        let rows = repo.list_for_item("i1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "c1");
        assert_eq!(rows[0].body, "hello");
    }

    #[tokio::test]
    async fn list_for_item_orders_oldest_first_and_scopes_to_item() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        repo.create(&comment("c2", "i1", 2_000)).await.unwrap();
        repo.create(&comment("c1", "i1", 1_000)).await.unwrap();
        repo.create(&comment("c3", "other-item", 500))
            .await
            .unwrap();

        let rows = repo.list_for_item("i1").await.unwrap();
        assert_eq!(
            rows.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["c1", "c2"]
        );
    }

    #[tokio::test]
    async fn get_returns_the_stored_comment() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        repo.create(&comment("c1", "i1", 1_000)).await.unwrap();

        let row = repo.get("c1").await.unwrap();
        assert_eq!(row.body, "hello");
    }

    #[tokio::test]
    async fn get_missing_comment_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        let err = repo.get("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn update_changes_the_body() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        repo.create(&comment("c1", "i1", 1_000)).await.unwrap();
        repo.update("c1", "edited").await.unwrap();

        let row = repo.get("c1").await.unwrap();
        assert_eq!(row.body, "edited");
    }

    #[tokio::test]
    async fn update_missing_comment_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        let err = repo.update("missing", "edited").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn delete_removes_the_comment() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        repo.create(&comment("c1", "i1", 1_000)).await.unwrap();
        repo.delete("c1").await.unwrap();

        let err = repo.get("c1").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn delete_missing_comment_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteCommentRepo(pool);

        let err = repo.delete("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }
}
