use super::{Migration, MigrationError};
use async_trait::async_trait;
use sqlx::{Row, SqliteConnection};
use std::future::Future;
use std::pin::Pin;

/// Stage 10 core of docs/recurring-events-virtual-occurrences-rough-plan.md: the one-time
/// data migration that moves every existing top-level recurring `Item` onto an
/// `item_series`/`item_occurrences` pair, so the legacy single-row recurrence mechanism
/// (`Item::next_recurrence`/`clone_children`, retired in the same commit as this migration)
/// can be deleted with no functional loss for live data.
///
/// Design forks resolved via `AskUserQuestion` before this was written (see the doc's own
/// "Stage 10 core" planning-notes section for full reasoning):
/// - `DUE_DATE`-basis items are migrated too (not left on the legacy mechanism); their
///   future occurrences become `scheduled_date`-primary, since `item_series` has no
///   due-vs-scheduled distinction (`get_or_materialize_occurrence` always writes
///   `scheduled_date`).
/// - A migrated item's children (if any — Task-only, Events can never have children) are
///   preserved by auto-synthesizing a `Template` item from the current subtree and linking
///   it via the new series' `template_item_id`, mirroring what `clone_children` used to do
///   on every recurrence.
/// - `COMPLETION_DATE`-basis items that don't fit `item_series`'s narrower `COMPLETION`
///   basis rules (Task-only, "every N days/weeks/months/years" patterns only — see
///   `service::item_series::validate_series_basis`) downgrade silently to schedule-basis
///   rather than being rejected or left unmigrated.
/// - `event_type` is always dropped (inherits item 5's already-committed "unsupported on
///   any series" decision).
///
/// A migration can't call service-layer code, so this works directly against raw SQL and
/// the (pure, no I/O) `domain::recurrence::parse` — never `service::items`/`item_series`.
///
/// Idempotent for free: the last step of processing each row clears `items.recurrence`/
/// `recurrence_basis`, so a second run finds zero rows matching this migration's own
/// `WHERE recurrence IS NOT NULL` guard — no separate `NOT EXISTS` check needed (unlike
/// `backfill_projects.rs`, whose guard wasn't self-clearing). The whole `up()` runs inside
/// one transaction (`run_migrations`'s existing per-migration-transaction behavior), so a
/// mid-run failure rolls back cleanly.
pub struct MigrateLegacyRecurringItems;

#[async_trait]
impl Migration for MigrateLegacyRecurringItems {
    fn version(&self) -> i64 {
        20
    }

    fn name(&self) -> &str {
        "migrate legacy recurring items onto item_series"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        let now = chrono::Utc::now().timestamp();

        // `item_type = 'TEMPLATE'` rows are deliberately excluded — a Template's own
        // `recurrence` is only ever an inert copy from a source item at "Save as template"
        // time (`service::templates::create_template`/`create_team_template`), never live.
        // `parent_item_id IS NULL`/`project_id IS NOT NULL` are defensive — every live
        // recurring item is already top-level (enforced at every create/update call site)
        // and project-scoped (Stage C3), but cost nothing to double-check here.
        let rows = sqlx::query(
            "SELECT id, project_id, user_id, name, description, item_type,
                    due_date, scheduled_date, recurrence, recurrence_basis
             FROM items
             WHERE recurrence IS NOT NULL
               AND item_type IN ('TASK', 'EVENT')
               AND parent_item_id IS NULL
               AND project_id IS NOT NULL",
        )
        .fetch_all(&mut *conn)
        .await?;

        for row in rows {
            let id: String = row.get("id");
            let project_id: String = row.get("project_id");
            let user_id: Option<String> = row.get("user_id");
            let name: String = row.get("name");
            let description: Option<String> = row.get("description");
            let item_type: String = row.get("item_type");
            let due_date: Option<i64> = row.get("due_date");
            let scheduled_date: Option<i64> = row.get("scheduled_date");
            let recurrence: String = row.get("recurrence");
            let recurrence_basis: Option<String> = row.get("recurrence_basis");

            // Every live row's `recurrence` was already validated parseable at write time
            // (`create_item`/`update_item`/etc.), so this should never fail in practice —
            // defensive `continue` rather than failing the whole migration over one bad row.
            let Ok(rule) = crate::domain::recurrence::parse(&recurrence) else {
                continue;
            };

            let (anchor, series_basis): (i64, Option<&'static str>) =
                match recurrence_basis.as_deref() {
                    Some("SCHEDULED_DATE") => (scheduled_date.or(due_date).unwrap_or(now), None),
                    Some("COMPLETION_DATE") => {
                        let anchor = scheduled_date.or(due_date).unwrap_or(now);
                        // Mirrors `validate_series_basis`'s exact eligibility check
                        // (`src/service/item_series.rs`): Task-only, and only the four
                        // fixed-interval units (not `MonthlyDay`/`WeeklyDay`).
                        let eligible = item_type == "TASK"
                            && matches!(
                                rule.unit,
                                crate::domain::recurrence::RecurrenceUnit::Days
                                    | crate::domain::recurrence::RecurrenceUnit::Weeks
                                    | crate::domain::recurrence::RecurrenceUnit::Months
                                    | crate::domain::recurrence::RecurrenceUnit::Years
                            );
                        (anchor, if eligible { Some("COMPLETION") } else { None })
                    }
                    // NULL (legacy default) or "DUE_DATE".
                    _ => (due_date.or(scheduled_date).unwrap_or(now), None),
                };

            // Children carry-forward: Task-only (Events can never have children). Synthesize
            // a Template from the current subtree and link it, so future materializations
            // keep getting children the same way `clone_children` used to.
            let template_item_id: Option<String> = if item_type == "TASK" {
                let has_children: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE parent_item_id = ?")
                        .bind(&id)
                        .fetch_one(&mut *conn)
                        .await?;
                if has_children > 0 {
                    Some(synthesize_template(&mut *conn, &id, &project_id, &user_id, &name).await?)
                } else {
                    None
                }
            } else {
                None
            };

            let series_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO item_series (id, project_id, name, description, event_type, recurrence, anchor_date, item_type, basis, template_item_id) \
                 VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, ?)",
            )
            .bind(&series_id)
            .bind(&project_id)
            .bind(&name)
            .bind(&description)
            .bind(&recurrence)
            .bind(anchor)
            .bind(&item_type)
            .bind(series_basis)
            .bind(&template_item_id)
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "INSERT INTO item_occurrences (series_id, occurrence_date, item_id, is_exdate) \
                 VALUES (?, ?, ?, 0)",
            )
            .bind(&series_id)
            .bind(anchor)
            .bind(&id)
            .execute(&mut *conn)
            .await?;

            sqlx::query("UPDATE items SET recurrence = NULL, recurrence_basis = NULL WHERE id = ?")
                .bind(&id)
                .execute(&mut *conn)
                .await?;
        }

        Ok(())
    }
}

/// Creates a new `TEMPLATE`-kind root item (copying `user_id`/`project_id`/`name` from
/// `source_id`) and recursively copies `source_id`'s entire child subtree onto it as
/// `TEMPLATE`-kind children — the migration-time equivalent of
/// `service::items::copy_children_as_template` (used by "Save as template"), reimplemented
/// against raw SQL since a migration can't call service-layer code. Returns the new
/// template root's id.
fn synthesize_template<'a>(
    conn: &'a mut SqliteConnection,
    source_id: &'a str,
    project_id: &'a str,
    user_id: &'a Option<String>,
    name: &'a str,
) -> Pin<Box<dyn Future<Output = Result<String, MigrationError>> + Send + 'a>> {
    Box::pin(async move {
        let template_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO items (id, user_id, project_id, parent_item_id, name, description, \
             due_date, scheduled_date, scheduled_end_date, complete, recurrence, recurrence_basis, \
             has_due_time, has_scheduled_time, has_end_time, item_type, event_type, due_offset_days, \
             assigned_to_user_id, points, source_event_id) \
             VALUES (?, ?, ?, NULL, ?, NULL, NULL, NULL, NULL, 0, NULL, NULL, 0, 0, 0, 'TEMPLATE', NULL, NULL, NULL, NULL, NULL)",
        )
        .bind(&template_id)
        .bind(user_id)
        .bind(project_id)
        .bind(name)
        .execute(&mut *conn)
        .await?;

        copy_children_as_template(&mut *conn, source_id, &template_id).await?;

        Ok(template_id)
    })
}

fn copy_children_as_template<'a>(
    conn: &'a mut SqliteConnection,
    source_parent_id: &'a str,
    new_template_parent_id: &'a str,
) -> Pin<Box<dyn Future<Output = Result<(), MigrationError>> + Send + 'a>> {
    Box::pin(async move {
        let children = sqlx::query(
            "SELECT id, user_id, project_id, name, description, due_offset_days \
             FROM items WHERE parent_item_id = ?",
        )
        .bind(source_parent_id)
        .fetch_all(&mut *conn)
        .await?;

        for child in children {
            let child_id: String = child.get("id");
            let child_user_id: Option<String> = child.get("user_id");
            let child_project_id: Option<String> = child.get("project_id");
            let child_name: String = child.get("name");
            let child_description: Option<String> = child.get("description");
            let child_due_offset_days: Option<i32> = child.get("due_offset_days");

            let new_child_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO items (id, user_id, project_id, parent_item_id, name, description, \
                 due_date, scheduled_date, scheduled_end_date, complete, recurrence, recurrence_basis, \
                 has_due_time, has_scheduled_time, has_end_time, item_type, event_type, due_offset_days, \
                 assigned_to_user_id, points, source_event_id) \
                 VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, NULL, 0, NULL, NULL, 0, 0, 0, 'TEMPLATE', NULL, ?, NULL, NULL, NULL)",
            )
            .bind(&new_child_id)
            .bind(&child_user_id)
            .bind(&child_project_id)
            .bind(new_template_parent_id)
            .bind(&child_name)
            .bind(&child_description)
            .bind(child_due_offset_days)
            .execute(&mut *conn)
            .await?;

            copy_children_as_template(&mut *conn, &child_id, &new_child_id).await?;
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                project_id TEXT,
                parent_item_id TEXT,
                name TEXT NOT NULL,
                description TEXT,
                due_date INTEGER,
                scheduled_date INTEGER,
                scheduled_end_date INTEGER,
                complete INTEGER DEFAULT 0,
                recurrence TEXT,
                recurrence_basis TEXT,
                has_due_time INTEGER NOT NULL DEFAULT 0,
                has_scheduled_time INTEGER NOT NULL DEFAULT 0,
                has_end_time INTEGER NOT NULL DEFAULT 0,
                item_type TEXT NOT NULL DEFAULT 'TASK',
                event_type TEXT,
                due_offset_days INTEGER,
                assigned_to_user_id TEXT,
                points INTEGER,
                source_event_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE item_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL,
                item_type TEXT NOT NULL DEFAULT 'EVENT',
                cursor_date INTEGER,
                basis TEXT,
                template_item_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE item_occurrences (
                series_id TEXT NOT NULL,
                occurrence_date INTEGER NOT NULL,
                item_id TEXT,
                is_exdate INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (series_id, occurrence_date)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_item(
        pool: &SqlitePool,
        id: &str,
        project_id: &str,
        parent_item_id: Option<&str>,
        name: &str,
        item_type: &str,
        due_date: Option<i64>,
        scheduled_date: Option<i64>,
        recurrence: Option<&str>,
        recurrence_basis: Option<&str>,
        due_offset_days: Option<i32>,
    ) {
        sqlx::query(
            "INSERT INTO items (id, user_id, project_id, parent_item_id, name, item_type, \
             due_date, scheduled_date, recurrence, recurrence_basis, due_offset_days) \
             VALUES (?, 'u1', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(parent_item_id)
        .bind(name)
        .bind(item_type)
        .bind(due_date)
        .bind(scheduled_date)
        .bind(recurrence)
        .bind(recurrence_basis)
        .bind(due_offset_days)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn migrates_a_due_date_basis_task_with_no_basis_set() {
        let pool = test_pool().await;
        insert_item(
            &pool, "t1", "p1", None, "Water plants", "TASK",
            Some(1_000), None, Some("every 2 days"), None, None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT * FROM item_series").fetch_one(&pool).await.unwrap();
        assert_eq!(series.get::<String, _>("project_id"), "p1");
        assert_eq!(series.get::<String, _>("name"), "Water plants");
        assert_eq!(series.get::<i64, _>("anchor_date"), 1_000);
        assert_eq!(series.get::<String, _>("item_type"), "TASK");
        assert_eq!(series.get::<Option<String>, _>("basis"), None);
        assert_eq!(series.get::<Option<String>, _>("template_item_id"), None);

        let series_id: String = series.get("id");
        let occ = sqlx::query("SELECT * FROM item_occurrences WHERE series_id = ?")
            .bind(&series_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(occ.get::<i64, _>("occurrence_date"), 1_000);
        assert_eq!(occ.get::<Option<String>, _>("item_id"), Some("t1".to_string()));
        assert_eq!(occ.get::<i64, _>("is_exdate"), 0);

        let item = sqlx::query("SELECT recurrence, recurrence_basis FROM items WHERE id = 't1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(item.get::<Option<String>, _>("recurrence"), None);
        assert_eq!(item.get::<Option<String>, _>("recurrence_basis"), None);
    }

    #[tokio::test]
    async fn scheduled_date_basis_anchors_at_scheduled_date() {
        let pool = test_pool().await;
        insert_item(
            &pool, "t1", "p1", None, "Standup", "EVENT",
            None, Some(2_000), Some("every week"), Some("SCHEDULED_DATE"), None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT anchor_date, basis FROM item_series").fetch_one(&pool).await.unwrap();
        assert_eq!(series.get::<i64, _>("anchor_date"), 2_000);
        assert_eq!(series.get::<Option<String>, _>("basis"), None);
    }

    #[tokio::test]
    async fn eligible_completion_basis_task_gets_completion_basis() {
        let pool = test_pool().await;
        insert_item(
            &pool, "t1", "p1", None, "Refill water filter", "TASK",
            None, Some(3_000), Some("every 3 days"), Some("COMPLETION_DATE"), None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT basis FROM item_series").fetch_one(&pool).await.unwrap();
        assert_eq!(series.get::<Option<String>, _>("basis"), Some("COMPLETION".to_string()));
    }

    #[tokio::test]
    async fn ineligible_pattern_downgrades_completion_basis() {
        let pool = test_pool().await;
        insert_item(
            &pool, "t1", "p1", None, "Pay rent", "TASK",
            None, Some(3_000), Some("every month on the 3rd"), Some("COMPLETION_DATE"), None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT basis FROM item_series").fetch_one(&pool).await.unwrap();
        assert_eq!(series.get::<Option<String>, _>("basis"), None);
    }

    #[tokio::test]
    async fn completion_basis_event_downgrades_to_schedule_basis() {
        let pool = test_pool().await;
        insert_item(
            &pool, "e1", "p1", None, "Weekly sync", "EVENT",
            None, Some(3_000), Some("every 7 days"), Some("COMPLETION_DATE"), None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT item_type, basis FROM item_series").fetch_one(&pool).await.unwrap();
        assert_eq!(series.get::<String, _>("item_type"), "EVENT");
        assert_eq!(series.get::<Option<String>, _>("basis"), None);
    }

    #[tokio::test]
    async fn synthesizes_a_template_from_a_two_level_child_subtree() {
        let pool = test_pool().await;
        insert_item(
            &pool, "t1", "p1", None, "Move house", "TASK",
            Some(5_000), None, Some("every year"), None, None,
        )
        .await;
        insert_item(
            &pool, "c1", "p1", Some("t1"), "Pack boxes", "TASK",
            None, None, None, None, Some(-3),
        )
        .await;
        insert_item(
            &pool, "g1", "p1", Some("c1"), "Label boxes", "TASK",
            None, None, None, None, Some(-2),
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT template_item_id FROM item_series").fetch_one(&pool).await.unwrap();
        let template_id: String = series.get("template_item_id");

        let template = sqlx::query("SELECT item_type, parent_item_id FROM items WHERE id = ?")
            .bind(&template_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(template.get::<String, _>("item_type"), "TEMPLATE");
        assert_eq!(template.get::<Option<String>, _>("parent_item_id"), None);

        let child = sqlx::query(
            "SELECT item_type, name, due_offset_days FROM items WHERE parent_item_id = ?",
        )
        .bind(&template_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(child.get::<String, _>("item_type"), "TEMPLATE");
        assert_eq!(child.get::<String, _>("name"), "Pack boxes");
        assert_eq!(child.get::<Option<i32>, _>("due_offset_days"), Some(-3));

        let child_id: String = sqlx::query_scalar("SELECT id FROM items WHERE parent_item_id = ?")
            .bind(&template_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let grandchild = sqlx::query(
            "SELECT item_type, name, due_offset_days FROM items WHERE parent_item_id = ?",
        )
        .bind(&child_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(grandchild.get::<String, _>("item_type"), "TEMPLATE");
        assert_eq!(grandchild.get::<String, _>("name"), "Label boxes");
        assert_eq!(grandchild.get::<Option<i32>, _>("due_offset_days"), Some(-2));

        // The original subtree is untouched (non-destructive, unlike legacy `clone_children`).
        let original_children: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE parent_item_id = 't1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(original_children, 1);
    }

    #[tokio::test]
    async fn task_with_no_children_gets_no_template() {
        let pool = test_pool().await;
        insert_item(
            &pool, "t1", "p1", None, "Water plants", "TASK",
            Some(1_000), None, Some("every 2 days"), None, None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT template_item_id FROM item_series").fetch_one(&pool).await.unwrap();
        assert_eq!(series.get::<Option<String>, _>("template_item_id"), None);
    }

    #[tokio::test]
    async fn events_never_attempt_template_synthesis() {
        let pool = test_pool().await;
        insert_item(
            &pool, "e1", "p1", None, "Weekly sync", "EVENT",
            None, Some(3_000), Some("every week"), None, None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series = sqlx::query("SELECT template_item_id FROM item_series").fetch_one(&pool).await.unwrap();
        assert_eq!(series.get::<Option<String>, _>("template_item_id"), None);
    }

    #[tokio::test]
    async fn template_kind_items_with_inert_recurrence_are_skipped() {
        let pool = test_pool().await;
        insert_item(
            &pool, "tpl1", "p1", None, "Move house template", "TEMPLATE",
            None, None, Some("every year"), None, None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn running_twice_is_idempotent() {
        let pool = test_pool().await;
        insert_item(
            &pool, "t1", "p1", None, "Water plants", "TASK",
            Some(1_000), None, Some("every 2 days"), None, None,
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();
        MigrateLegacyRecurringItems.up(&mut conn).await.unwrap();

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series_count, 1);
        let occ_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_occurrences")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(occ_count, 1);
    }
}
