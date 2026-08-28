use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

use crate::domain::reminder::{Reminder, ReminderKind};
use crate::storage::sqlite::{ReminderRepo, RepoError, db_err};

pub struct SqliteReminderRepo(pub SqlitePool);

const REMINDER_SELECT: &str = "SELECT id, item_id, project_id, user_id, kind, source, \
     remind_at, sent_at, created_at FROM reminders";

fn row_to_reminder(row: &sqlx::sqlite::SqliteRow) -> Reminder {
    let kind: String = row.get("kind");
    let remind_at: i64 = row.get("remind_at");
    let sent_at: Option<i64> = row.get("sent_at");
    let created_at: i64 = row.get("created_at");
    Reminder {
        id: row.get("id"),
        item_id: row.get("item_id"),
        project_id: row.get("project_id"),
        user_id: row.get("user_id"),
        kind: ReminderKind::from_str(&kind).unwrap_or(ReminderKind::Due),
        source: row.get("source"),
        remind_at: DateTime::from_timestamp(remind_at, 0).unwrap_or_default(),
        sent_at: sent_at.and_then(|ts| DateTime::from_timestamp(ts, 0)),
        created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_default(),
    }
}

#[async_trait]
impl ReminderRepo for SqliteReminderRepo {
    async fn sync_auto_reminders(
        &self,
        item_id: &str,
        project_id: &str,
        user_id: &str,
        reminders: &[(ReminderKind, DateTime<Utc>)],
    ) -> Result<(), RepoError> {
        let mut tx = self.0.begin().await.map_err(db_err)?;

        let existing: Vec<(String, String, i64)> = sqlx::query(
            "SELECT id, kind, remind_at FROM reminders WHERE item_id = ? AND source = 'AUTO'",
        )
        .bind(item_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?
        .iter()
        .map(|row| (row.get("id"), row.get("kind"), row.get("remind_at")))
        .collect();

        let created_at = Utc::now().timestamp();
        let mut seen_kinds = std::collections::HashSet::new();

        for (kind, remind_at) in reminders {
            seen_kinds.insert(kind.as_str());
            let remind_at_ts = remind_at.timestamp();
            match existing.iter().find(|(_, k, _)| k == kind.as_str()) {
                // Same remind_at: leave the row (and its sent_at) untouched, so an
                // unrelated edit to the item doesn't silently un-dismiss a reminder.
                Some((_, _, existing_remind_at)) if *existing_remind_at == remind_at_ts => {}
                // remind_at moved: it's effectively a new reminder, so clear sent_at.
                Some((id, _, _)) => {
                    sqlx::query("UPDATE reminders SET remind_at = ?, sent_at = NULL WHERE id = ?")
                        .bind(remind_at_ts)
                        .bind(id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                }
                None => {
                    let id = uuid::Uuid::new_v4().to_string();
                    sqlx::query(
                        "INSERT INTO reminders \
                         (id, item_id, project_id, user_id, kind, source, remind_at, sent_at, created_at) \
                         VALUES (?, ?, ?, ?, ?, 'AUTO', ?, NULL, ?)",
                    )
                    .bind(&id)
                    .bind(item_id)
                    .bind(project_id)
                    .bind(user_id)
                    .bind(kind.as_str())
                    .bind(remind_at_ts)
                    .bind(created_at)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                }
            }
        }

        for (id, kind, _) in &existing {
            if !seen_kinds.contains(kind.as_str()) {
                sqlx::query("DELETE FROM reminders WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
            }
        }

        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn delete_for_item(&self, item_id: &str) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM reminders WHERE item_id = ?")
            .bind(item_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_for_item(&self, item_id: &str) -> Result<Vec<Reminder>, RepoError> {
        let q = format!("{REMINDER_SELECT} WHERE item_id = ? ORDER BY remind_at ASC");
        sqlx::query(&q)
            .bind(item_id)
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_reminder).collect())
    }

    async fn list_due_for_user(
        &self,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<Reminder>, RepoError> {
        let q = format!(
            "{REMINDER_SELECT} WHERE user_id = ? AND remind_at <= ? AND sent_at IS NULL \
             ORDER BY remind_at ASC"
        );
        sqlx::query(&q)
            .bind(user_id)
            .bind(now.timestamp())
            .fetch_all(&self.0)
            .await
            .map_err(db_err)
            .map(|rows| rows.iter().map(row_to_reminder).collect())
    }

    async fn dismiss(&self, id: &str, user_id: &str) -> Result<(), RepoError> {
        sqlx::query("UPDATE reminders SET sent_at = ? WHERE id = ? AND user_id = ?")
            .bind(Utc::now().timestamp())
            .bind(id)
            .bind(user_id)
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

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE reminders (
                id TEXT PRIMARY KEY,
                item_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'AUTO',
                remind_at INTEGER NOT NULL,
                sent_at INTEGER,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn ts(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[tokio::test]
    async fn sync_auto_reminders_creates_one_row_per_kind() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool);

        repo.sync_auto_reminders(
            "item1",
            "proj1",
            "user1",
            &[
                (ReminderKind::Due, ts(1_000)),
                (ReminderKind::ScheduledStart, ts(2_000)),
                (ReminderKind::ScheduledEnd, ts(3_000)),
            ],
        )
        .await
        .unwrap();

        let rows = repo.list_for_item("item1").await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, ReminderKind::Due);
        assert_eq!(rows[0].remind_at, ts(1_000));
        assert_eq!(rows[0].user_id, "user1");
        assert_eq!(rows[0].project_id, "proj1");
        assert_eq!(rows[0].source, "AUTO");
        assert_eq!(rows[0].sent_at, None);
    }

    #[tokio::test]
    async fn sync_auto_reminders_replaces_prior_auto_rows() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool);

        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(9_999))])
            .await
            .unwrap();

        let rows = repo.list_for_item("item1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].remind_at, ts(9_999));
    }

    #[tokio::test]
    async fn sync_auto_reminders_leaves_custom_rows_untouched() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO reminders \
             (id, item_id, project_id, user_id, kind, source, remind_at, created_at) \
             VALUES ('custom1', 'item1', 'proj1', 'user1', 'DUE', 'CUSTOM', 5000, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let repo = SqliteReminderRepo(pool);

        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();

        let rows = repo.list_for_item("item1").await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|r| r.source == "CUSTOM" && r.id == "custom1")
        );
        assert!(rows.iter().any(|r| r.source == "AUTO"));
    }

    #[tokio::test]
    async fn sync_auto_reminders_with_empty_slice_clears_existing_auto_rows() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool);
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();

        repo.sync_auto_reminders("item1", "proj1", "user1", &[])
            .await
            .unwrap();

        assert!(repo.list_for_item("item1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sync_auto_reminders_preserves_sent_at_when_remind_at_unchanged() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool.clone());
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();
        let row = repo.list_for_item("item1").await.unwrap().remove(0);
        sqlx::query("UPDATE reminders SET sent_at = ? WHERE id = ?")
            .bind(5_000_i64)
            .bind(&row.id)
            .execute(&pool)
            .await
            .unwrap();

        // An unrelated edit that recomputes the same due-based remind_at must not
        // clobber the dismissal recorded above.
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();

        let rows = repo.list_for_item("item1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, row.id);
        assert_eq!(rows[0].sent_at, Some(ts(5_000)));
    }

    #[tokio::test]
    async fn sync_auto_reminders_clears_sent_at_when_remind_at_changes() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool.clone());
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();
        let row = repo.list_for_item("item1").await.unwrap().remove(0);
        sqlx::query("UPDATE reminders SET sent_at = ? WHERE id = ?")
            .bind(5_000_i64)
            .bind(&row.id)
            .execute(&pool)
            .await
            .unwrap();

        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(9_999))])
            .await
            .unwrap();

        let rows = repo.list_for_item("item1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, row.id);
        assert_eq!(rows[0].remind_at, ts(9_999));
        assert_eq!(rows[0].sent_at, None);
    }

    #[tokio::test]
    async fn sync_auto_reminders_drops_kinds_no_longer_present() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool.clone());
        repo.sync_auto_reminders(
            "item1",
            "proj1",
            "user1",
            &[
                (ReminderKind::Due, ts(1_000)),
                (ReminderKind::ScheduledStart, ts(2_000)),
            ],
        )
        .await
        .unwrap();
        let due_row = repo
            .list_for_item("item1")
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.kind == ReminderKind::Due)
            .unwrap();
        sqlx::query("UPDATE reminders SET sent_at = ? WHERE id = ?")
            .bind(5_000_i64)
            .bind(&due_row.id)
            .execute(&pool)
            .await
            .unwrap();

        // The item no longer has a scheduled start; the Due reminder is unchanged.
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();

        let rows = repo.list_for_item("item1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, ReminderKind::Due);
        assert_eq!(rows[0].sent_at, Some(ts(5_000)));
    }

    #[tokio::test]
    async fn list_due_for_user_excludes_future_and_dismissed_and_other_users() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool.clone());
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();
        repo.sync_auto_reminders("item2", "proj1", "user1", &[(ReminderKind::Due, ts(9_999))])
            .await
            .unwrap();
        repo.sync_auto_reminders("item3", "proj1", "user2", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();
        let dismissed = repo.list_for_item("item1").await.unwrap().remove(0);
        repo.dismiss(&dismissed.id, "user1").await.unwrap();
        repo.sync_auto_reminders(
            "item4",
            "proj1",
            "user1",
            &[
                (ReminderKind::Due, ts(1_000)),
                (ReminderKind::ScheduledStart, ts(500)),
            ],
        )
        .await
        .unwrap();

        let due = repo.list_due_for_user("user1", ts(2_000)).await.unwrap();

        assert_eq!(due.len(), 2);
        assert!(due.iter().all(|r| r.item_id == "item4"));
        assert!(due.iter().any(|r| r.kind == ReminderKind::Due));
        assert!(due.iter().any(|r| r.kind == ReminderKind::ScheduledStart));
    }

    #[tokio::test]
    async fn dismiss_is_scoped_to_the_owning_user() {
        let pool = test_pool().await;
        let repo = SqliteReminderRepo(pool.clone());
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();
        let row = repo.list_for_item("item1").await.unwrap().remove(0);

        repo.dismiss(&row.id, "user2").await.unwrap();
        let still_due = repo.list_due_for_user("user1", ts(2_000)).await.unwrap();
        assert_eq!(still_due.len(), 1);

        repo.dismiss(&row.id, "user1").await.unwrap();
        let now_dismissed = repo.list_due_for_user("user1", ts(2_000)).await.unwrap();
        assert!(now_dismissed.is_empty());
    }

    #[tokio::test]
    async fn delete_for_item_removes_auto_and_custom_rows() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO reminders \
             (id, item_id, project_id, user_id, kind, source, remind_at, created_at) \
             VALUES ('custom1', 'item1', 'proj1', 'user1', 'DUE', 'CUSTOM', 5000, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let repo = SqliteReminderRepo(pool);
        repo.sync_auto_reminders("item1", "proj1", "user1", &[(ReminderKind::Due, ts(1_000))])
            .await
            .unwrap();

        repo.delete_for_item("item1").await.unwrap();

        assert!(repo.list_for_item("item1").await.unwrap().is_empty());
    }
}
