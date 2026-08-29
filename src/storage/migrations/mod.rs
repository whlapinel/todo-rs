mod activity_log;
mod activity_log_team_id_nullable;
mod add_attachments;
mod add_calendar_subscriptions;
mod add_comments;
mod add_event_occurrences_item_id_index;
mod add_event_series;
mod add_item_dependencies;
mod add_item_description;
mod add_item_points;
mod add_item_priority;
mod add_item_series;
mod add_item_series_assignment;
mod add_item_series_basis;
mod add_item_series_cursor_date;
mod add_item_series_id;
mod add_item_series_rotation_members;
mod add_item_series_template_item_id;
mod add_item_source_event_id;
mod add_projects;
mod add_push_subscriptions;
mod add_reminders;
mod add_team_member_role;
mod add_user_personal_project_id;
mod add_user_timezone;
mod backfill_projects;
mod drop_items_team_id;
mod drop_team_member_points;
mod ensure_attachments_comment_id;
mod has_tasks_to_simple;
mod item_type_event_type;
mod migrate_legacy_recurring_items;
mod scheduled_end_date;
mod team_member_points;

use activity_log::ActivityLog;
use activity_log_team_id_nullable::ActivityLogTeamIdNullable;
use add_attachments::AddAttachments;
use add_calendar_subscriptions::AddCalendarSubscriptions;
use add_comments::AddComments;
use add_event_occurrences_item_id_index::AddEventOccurrencesItemIdIndex;
use add_event_series::AddEventSeries;
use add_item_dependencies::AddItemDependencies;
use add_item_description::AddItemDescription;
use add_item_points::AddItemPoints;
use add_item_priority::AddItemPriority;
use add_item_series::AddItemSeries;
use add_item_series_assignment::AddItemSeriesAssignment;
use add_item_series_basis::AddItemSeriesBasis;
use add_item_series_cursor_date::AddItemSeriesCursorDate;
use add_item_series_id::AddItemSeriesId;
use add_item_series_rotation_members::AddItemSeriesRotationMembers;
use add_item_series_template_item_id::AddItemSeriesTemplateItemId;
use add_item_source_event_id::AddItemSourceEventId;
use add_projects::AddProjects;
use add_push_subscriptions::AddPushSubscriptions;
use add_reminders::AddReminders;
use add_team_member_role::AddTeamMemberRole;
use add_user_personal_project_id::AddUserPersonalProjectId;
use add_user_timezone::AddUserTimezone;
use async_trait::async_trait;
use backfill_projects::BackfillProjects;
use drop_items_team_id::DropItemsTeamId;
use drop_team_member_points::DropTeamMemberPoints;
use ensure_attachments_comment_id::EnsureAttachmentsCommentId;
use has_tasks_to_simple::HasTasksToSimple;
use item_type_event_type::ItemTypeEventType;
use migrate_legacy_recurring_items::MigrateLegacyRecurringItems;
use scheduled_end_date::ScheduledEndDate;
use sqlx::{Row, SqliteConnection, SqlitePool};
use team_member_points::TeamMemberPoints;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait]
trait Migration: Send + Sync {
    fn version(&self) -> i64;
    fn name(&self) -> &str;
    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError>;
}

/// Whether `table` already has `column` — SQLite's `ALTER TABLE ... ADD
/// COLUMN`/`DROP COLUMN` have no `IF NOT EXISTS` form, so migrations that
/// alter an existing table must check first. `table`/`column` are always
/// internal literals from call sites, never user input.
async fn column_exists(
    conn: &mut SqliteConnection,
    table: &str,
    column: &str,
) -> Result<bool, MigrationError> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column))
}

fn all_migrations() -> Vec<Box<dyn Migration>> {
    vec![
        Box::new(ItemTypeEventType),
        Box::new(ScheduledEndDate),
        Box::new(HasTasksToSimple),
        Box::new(AddTeamMemberRole),
        Box::new(AddItemPoints),
        Box::new(ActivityLog),
        Box::new(TeamMemberPoints),
        Box::new(AddItemDescription),
        Box::new(AddItemSourceEventId),
        Box::new(AddProjects),
        Box::new(BackfillProjects),
        Box::new(DropTeamMemberPoints),
        Box::new(AddEventSeries),
        Box::new(DropItemsTeamId),
        Box::new(AddEventOccurrencesItemIdIndex),
        Box::new(AddItemSeries),
        Box::new(AddItemSeriesCursorDate),
        Box::new(AddItemSeriesBasis),
        Box::new(AddItemSeriesTemplateItemId),
        Box::new(MigrateLegacyRecurringItems),
        Box::new(AddItemSeriesAssignment),
        Box::new(ActivityLogTeamIdNullable),
        Box::new(AddItemSeriesId),
        Box::new(AddItemSeriesRotationMembers),
        Box::new(AddCalendarSubscriptions),
        Box::new(AddUserTimezone),
        Box::new(AddUserPersonalProjectId),
        Box::new(AddReminders),
        Box::new(AddItemDependencies),
        Box::new(AddPushSubscriptions),
        Box::new(AddItemPriority),
        Box::new(AddComments),
        Box::new(AddAttachments),
        Box::new(EnsureAttachmentsCommentId),
    ]
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), MigrationError> {
    let mut conn = pool.acquire().await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(&mut *conn)
    .await?;

    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _migrations")
        .fetch_all(&mut *conn)
        .await?;

    let mut migrations = all_migrations();
    migrations.sort_by_key(|m| m.version());

    for m in migrations {
        if applied.contains(&m.version()) {
            continue;
        }

        let mut tx = pool.begin().await?;
        m.up(&mut tx).await?;
        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?, ?)")
            .bind(m.version())
            .bind(m.name())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        tracing::info!(version = m.version(), name = m.name(), "applied migration");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    // `run_migrations` holds an acquired connection for its own setup while
    // also opening a second one per migration transaction, so the pool
    // needs room for >1 connection at once. Shared-cache mode keeps every
    // pooled connection pointed at the same in-memory database rather than
    // each getting its own isolated one.
    async fn memory_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    async fn old_team_members_table(pool: &SqlitePool) {
        sqlx::query(
            "CREATE TABLE team_members (
                team_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'PENDING',
                invited_by TEXT,
                PRIMARY KEY (team_id, user_id)
            )",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// `users`/`teams` predate the migration system (part of the original hand-written
    /// base schema, like `items.user_id`/`items.team_id` below) — every schema-pool
    /// fixture that exercises the full `run_migrations()` pipeline needs them present,
    /// since stage B1's `BackfillProjects` (see `backfill_projects.rs`) reads both.
    async fn users_and_teams_tables(pool: &SqlitePool) {
        sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY, first_name TEXT NOT NULL)")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE teams (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn old_schema_pool() -> SqlitePool {
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT,
                team_id TEXT,
                due_date INTEGER,
                scheduled_date INTEGER,
                is_template INTEGER NOT NULL DEFAULT 0,
                parent_item_id TEXT,
                recurrence TEXT,
                recurrence_basis TEXT,
                due_offset_days INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        old_team_members_table(&pool).await;
        users_and_teams_tables(&pool).await;
        pool
    }

    async fn current_schema_pool() -> SqlitePool {
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT,
                description TEXT,
                due_date INTEGER,
                scheduled_date INTEGER,
                scheduled_end_date INTEGER,
                has_scheduled_time INTEGER NOT NULL DEFAULT 0,
                has_end_time INTEGER NOT NULL DEFAULT 0,
                item_type TEXT NOT NULL DEFAULT 'TASK',
                event_type TEXT,
                parent_item_id TEXT,
                recurrence TEXT,
                recurrence_basis TEXT,
                due_offset_days INTEGER,
                points INTEGER,
                project_id TEXT,
                google_event_id TEXT,
                calendar_subscription_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        users_and_teams_tables(&pool).await;
        sqlx::query(
            "CREATE TABLE team_members (
                team_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'PENDING',
                invited_by TEXT,
                role TEXT NOT NULL DEFAULT 'member',
                PRIMARY KEY (team_id, user_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE activity_log (
                id TEXT PRIMARY KEY,
                team_id TEXT,
                user_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                item_name TEXT NOT NULL,
                points_delta INTEGER NOT NULL,
                reversed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                project_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                owner_user_id TEXT NOT NULL,
                team_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE project_members (
                project_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'member',
                points INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (project_id, user_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE event_series (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                event_type TEXT,
                recurrence TEXT NOT NULL,
                anchor_date INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE event_occurrences (
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
                cursor_date INTEGER
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

    async fn pre_simple_schema_pool() -> SqlitePool {
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT,
                team_id TEXT,
                due_date INTEGER,
                scheduled_date INTEGER,
                scheduled_end_date INTEGER,
                has_scheduled_time INTEGER NOT NULL DEFAULT 0,
                has_end_time INTEGER NOT NULL DEFAULT 0,
                has_tasks INTEGER NOT NULL DEFAULT 1,
                item_type TEXT NOT NULL DEFAULT 'TASK',
                event_type TEXT,
                parent_item_id TEXT,
                recurrence TEXT,
                recurrence_basis TEXT,
                due_offset_days INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        old_team_members_table(&pool).await;
        users_and_teams_tables(&pool).await;
        pool
    }

    async fn pre_source_event_id_schema_pool() -> SqlitePool {
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT,
                team_id TEXT,
                due_date INTEGER,
                scheduled_date INTEGER,
                parent_item_id TEXT,
                item_type TEXT NOT NULL DEFAULT 'TASK',
                recurrence TEXT,
                recurrence_basis TEXT,
                due_offset_days INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        old_team_members_table(&pool).await;
        users_and_teams_tables(&pool).await;
        pool
    }

    #[tokio::test]
    async fn backfills_event_nested_children_into_source_event_id_references() {
        let pool = pre_source_event_id_schema_pool().await;
        sqlx::query(
            "INSERT INTO items (id, name, item_type, parent_item_id) VALUES \
             ('event1', 'Birthday party', 'EVENT', NULL), \
             ('child1', 'Buy cake', 'TASK', 'event1'), \
             ('other1', 'Unrelated task', 'TASK', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_migrations(&pool).await.unwrap();

        let (source_event_id, parent_item_id): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT source_event_id, parent_item_id FROM items WHERE id = 'child1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(source_event_id.as_deref(), Some("event1"));
        assert_eq!(parent_item_id, None);

        let (other_source_event_id, other_parent_item_id): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT source_event_id, parent_item_id FROM items WHERE id = 'other1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(other_source_event_id, None);
        assert_eq!(other_parent_item_id, None);
    }

    #[tokio::test]
    async fn backfills_simple_item_type_and_drops_has_tasks() {
        let pool = pre_simple_schema_pool().await;
        sqlx::query(
            "INSERT INTO items (id, name, has_tasks, item_type) VALUES \
             ('1', 'Milk', 0, 'TASK'), \
             ('2', 'Water plants', 1, 'TASK'), \
             ('3', 'checklist child', 0, 'TEMPLATE')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_migrations(&pool).await.unwrap();

        let item_type_1: String = sqlx::query_scalar("SELECT item_type FROM items WHERE id = '1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(item_type_1, "SIMPLE");

        let item_type_2: String = sqlx::query_scalar("SELECT item_type FROM items WHERE id = '2'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(item_type_2, "TASK");

        // TEMPLATE rows keep their own type — has_tasks = 0 there is for unrelated
        // reasons (checklist children never expose due-date fields regardless).
        let item_type_3: String = sqlx::query_scalar("SELECT item_type FROM items WHERE id = '3'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(item_type_3, "TEMPLATE");

        let mut conn = pool.acquire().await.unwrap();
        assert!(
            !column_exists(&mut conn, "items", "has_tasks")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn migrates_old_schema_and_backfills_item_type() {
        let pool = old_schema_pool().await;
        sqlx::query("INSERT INTO items (id, name, is_template) VALUES ('1', 'tmpl', 1)")
            .execute(&pool)
            .await
            .unwrap();

        run_migrations(&pool).await.unwrap();

        let item_type: String = sqlx::query_scalar("SELECT item_type FROM items WHERE id = '1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(item_type, "TEMPLATE");

        let mut conn = pool.acquire().await.unwrap();
        assert!(
            !column_exists(&mut conn, "items", "is_template")
                .await
                .unwrap()
        );
        assert!(
            column_exists(&mut conn, "items", "scheduled_end_date")
                .await
                .unwrap()
        );
        assert!(
            column_exists(&mut conn, "items", "has_scheduled_time")
                .await
                .unwrap()
        );
        assert!(
            column_exists(&mut conn, "items", "has_end_time")
                .await
                .unwrap()
        );

        let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied_count, 34);
    }

    #[tokio::test]
    async fn is_a_noop_against_a_db_already_on_the_current_schema() {
        let pool = current_schema_pool().await;

        run_migrations(&pool).await.unwrap();

        let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied_count, 34);
    }

    #[tokio::test]
    async fn running_twice_is_idempotent() {
        let pool = old_schema_pool().await;

        run_migrations(&pool).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied_count, 34);
    }
}
