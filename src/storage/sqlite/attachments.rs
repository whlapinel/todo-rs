use async_trait::async_trait;
use chrono::DateTime;
use sqlx::{Row, SqlitePool};

use crate::domain::attachment::Attachment;
use crate::storage::sqlite::{AttachmentRepo, RepoError, db_err};

pub struct SqliteAttachmentRepo(pub SqlitePool);

const ATTACHMENT_SELECT: &str = "SELECT id, comment_id, item_id, project_id, uploaded_by_user_id, \
     filename, content_type, size_bytes, storage_key, created_at FROM attachments";

fn row_to_attachment(row: &sqlx::sqlite::SqliteRow) -> Attachment {
    let created_at: i64 = row.get("created_at");
    Attachment {
        id: row.get("id"),
        comment_id: row.get("comment_id"),
        item_id: row.get("item_id"),
        project_id: row.get("project_id"),
        uploaded_by_user_id: row.get("uploaded_by_user_id"),
        filename: row.get("filename"),
        content_type: row.get("content_type"),
        size_bytes: row.get("size_bytes"),
        storage_key: row.get("storage_key"),
        created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_default(),
    }
}

#[async_trait]
impl AttachmentRepo for SqliteAttachmentRepo {
    async fn create(&self, attachment: &Attachment) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO attachments (id, comment_id, item_id, project_id, uploaded_by_user_id, \
             filename, content_type, size_bytes, storage_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&attachment.id)
        .bind(&attachment.comment_id)
        .bind(&attachment.item_id)
        .bind(&attachment.project_id)
        .bind(&attachment.uploaded_by_user_id)
        .bind(&attachment.filename)
        .bind(&attachment.content_type)
        .bind(attachment.size_bytes)
        .bind(&attachment.storage_key)
        .bind(attachment.created_at.timestamp())
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Attachment, RepoError> {
        let q = format!("{ATTACHMENT_SELECT} WHERE id = ?");
        sqlx::query(&q)
            .bind(id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| row_to_attachment(&row))
            .ok_or(RepoError::NotFound)
    }

    async fn list_for_item(&self, item_id: &str) -> Result<Vec<Attachment>, RepoError> {
        let q = format!("{ATTACHMENT_SELECT} WHERE item_id = ? ORDER BY created_at DESC");
        sqlx::query(&q)
            .bind(item_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_attachment).collect())
    }

    async fn delete(&self, id: &str) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM attachments WHERE id = ?")
            .bind(id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
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
            "CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                comment_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                uploaded_by_user_id TEXT NOT NULL,
                filename TEXT NOT NULL,
                content_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                storage_key TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn attachment(id: &str, item_id: &str, created_at_secs: i64) -> Attachment {
        Attachment {
            id: id.to_string(),
            comment_id: "c1".to_string(),
            item_id: item_id.to_string(),
            project_id: "p1".to_string(),
            uploaded_by_user_id: "u1".to_string(),
            filename: "photo.jpg".to_string(),
            content_type: "image/jpeg".to_string(),
            size_bytes: 1234,
            storage_key: format!("p1/{item_id}/{id}_photo.jpg"),
            created_at: DateTime::from_timestamp(created_at_secs, 0).unwrap_or_else(Utc::now),
        }
    }

    #[tokio::test]
    async fn create_then_get_roundtrips() {
        let pool = test_pool().await;
        let repo = SqliteAttachmentRepo(pool);

        repo.create(&attachment("a1", "i1", 1_000)).await.unwrap();

        let got = repo.get("a1").await.unwrap();
        assert_eq!(got.filename, "photo.jpg");
        assert_eq!(got.size_bytes, 1234);
    }

    #[tokio::test]
    async fn get_missing_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteAttachmentRepo(pool);

        let err = repo.get("nope").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn list_for_item_orders_newest_first_and_scopes_to_item() {
        let pool = test_pool().await;
        let repo = SqliteAttachmentRepo(pool);

        repo.create(&attachment("a1", "i1", 1_000)).await.unwrap();
        repo.create(&attachment("a2", "i1", 2_000)).await.unwrap();
        repo.create(&attachment("a3", "other-item", 3_000))
            .await
            .unwrap();

        let rows = repo.list_for_item("i1").await.unwrap();
        assert_eq!(
            rows.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec!["a2", "a1"]
        );
    }

    #[tokio::test]
    async fn delete_removes_the_row() {
        let pool = test_pool().await;
        let repo = SqliteAttachmentRepo(pool);

        repo.create(&attachment("a1", "i1", 1_000)).await.unwrap();
        repo.delete("a1").await.unwrap();

        assert!(matches!(
            repo.get("a1").await.unwrap_err(),
            RepoError::NotFound
        ));
    }
}
