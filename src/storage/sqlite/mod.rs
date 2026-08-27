pub mod activity_log;
pub mod calendar_subscriptions;
pub mod item_dependencies;
pub mod item_series;
pub mod items;
pub mod projects;
pub mod reminders;
pub mod teams;
pub mod users;
use crate::domain::{
    activity_log::ActivityLogEntry,
    calendar_subscription::CalendarSubscription,
    item::{Item, ItemKind, ItemType, Recurrence, Schedule, TeamAssignment},
    item_series::{ItemOccurrence, ItemSeries},
    project::Project,
    reminder::{Reminder, ReminderKind},
    team::{Team, TeamRole},
    user::User,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

pub struct DueItem {
    pub item: Item,
    pub parent_name: String,
}

pub struct TeamWithStatus {
    pub team: Team,
    pub status: String,
    pub invited_by_name: Option<String>,
}

pub struct TeamMemberInfo {
    pub user: User,
    pub status: String,
    pub role: TeamRole,
    pub points: i32,
}

/// No `status` field, unlike `TeamMemberInfo` — a project has no independent
/// invite flow at this stage; every row is either the owner (seeded at `create`)
/// or synced in eagerly from an attached team's ACTIVE members (stage A4).
pub struct ProjectMemberInfo {
    pub user: User,
    pub role: TeamRole,
    pub points: i32,
}

#[derive(Debug)]
pub enum RepoError {
    NotFound,
    Internal(String),
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn get(&self, user_id: &str) -> Result<User, RepoError>;
    async fn list(&self) -> Result<Vec<User>, RepoError>;
    async fn create(&self, user: &User) -> Result<String, RepoError>;
    async fn update(&self, user: &User) -> Result<(), RepoError>;
    async fn delete(&self, user_id: &str) -> Result<(), RepoError>;
    async fn get_or_create_by_google_id(
        &self,
        google_id: &str,
        email: &str,
        first_name: &str,
        last_name: &str,
    ) -> Result<User, RepoError>;
    async fn get_or_create_by_email<'a>(
        &'a self,
        email: &'a str,
        name: Option<&'a str>,
    ) -> Result<User, RepoError>;
    /// Sets `users.personal_project_id` — see `service::projects::ensure_default_project`,
    /// the sole caller, and `docs/dialog-item-forms-plan.md`'s Stage 0.
    async fn set_personal_project_id(
        &self,
        user_id: &str,
        project_id: &str,
    ) -> Result<(), RepoError>;
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ItemRepo: Send + Sync {
    async fn get(&self, user_id: &str, item_id: &str) -> Result<Item, RepoError>;
    /// Stage B3's unified read: an item scoped to `project_id` regardless of whether
    /// the underlying row is personal- or team-owned — see
    /// docs/project-abstraction-plan.md.
    async fn get_by_project(&self, project_id: &str, item_id: &str) -> Result<Item, RepoError>;
    async fn list(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
    /// Stage B3's unified list, keyed on `project_id` instead of `team_id`/`user_id`.
    async fn list_by_project(
        &self,
        project_id: &str,
        parent_item_id: Option<String>,
    ) -> Result<Vec<Item>, RepoError>;
    async fn list_children(&self, parent_item_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn list_by_source_event(&self, source_event_id: &str) -> Result<Vec<Item>, RepoError>;
    /// The full current set of imported items for one calendar subscription — what
    /// the Stage 3 sync diff (docs/google-calendar-import-plan.md) compares a
    /// freshly-parsed iCal feed against. `calendar_subscription_id`/`google_event_id`
    /// aren't domain fields yet (that's Stage 4) — this method exists now so Stage 3
    /// isn't blocked on it, but its `WHERE` clause is the only place either raw column
    /// is touched until Stage 4 lands.
    async fn list_by_calendar_subscription(
        &self,
        calendar_subscription_id: &str,
    ) -> Result<Vec<Item>, RepoError>;
    async fn create(&self, item: &Item) -> Result<String, RepoError>;
    async fn update(&self, item: &Item) -> Result<(), RepoError>;
    /// Stage B3's unified write primitive, keyed on `project_id` — carries the full
    /// column set (including `points`) so it's usable for both personal- and
    /// team-backed projects.
    async fn update_by_project(&self, item: &Item) -> Result<(), RepoError>;
    async fn delete(&self, item_id: &str) -> Result<(), RepoError>;
    async fn list_due(
        &self,
        user_id: &str,
        deadline_after: Option<i64>,
        deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError>;
    /// Stage B5e's project-scoped counterpart to `list_due` — keyed on `project_id`
    /// instead of `team_id`, so `project_calendar.rs` doesn't have to special-case
    /// personal vs. team-backed projects the way `dashboard.rs`/`team_dashboard.rs`
    /// used to be two separate screens.
    async fn list_due_by_project(
        &self,
        project_id: &str,
        deadline_after: Option<i64>,
        deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError>;
    async fn list_templates(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
    /// Project-scoped counterpart to `list_templates` — see
    /// docs/team-id-removal-plan.md's Stage 1.
    async fn list_templates_by_project(&self, project_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn list_assigned(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TeamRepo: Send + Sync {
    async fn create(&self, name: &str, creator_user_id: &str) -> Result<String, RepoError>;
    async fn get(&self, team_id: &str) -> Result<Team, RepoError>;
    async fn update_name(&self, team_id: &str, name: &str) -> Result<(), RepoError>;
    async fn list_for_user(&self, user_id: &str) -> Result<Vec<TeamWithStatus>, RepoError>;
    async fn list_members(&self, team_id: &str) -> Result<Vec<TeamMemberInfo>, RepoError>;
    async fn member_status(
        &self,
        team_id: &str,
        user_id: &str,
    ) -> Result<Option<String>, RepoError>;
    async fn member_role(
        &self,
        team_id: &str,
        user_id: &str,
    ) -> Result<Option<TeamRole>, RepoError>;
    /// Count of `ACTIVE` members with `role = 'admin'` on this team — used to guard
    /// against demoting a team's last remaining admin.
    async fn count_active_admins(&self, team_id: &str) -> Result<i64, RepoError>;
    async fn set_member_role(
        &self,
        team_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<(), RepoError>;
    async fn invite(
        &self,
        team_id: &str,
        invitee_user_id: &str,
        invited_by: &str,
    ) -> Result<(), RepoError>;
    async fn accept(&self, team_id: &str, user_id: &str) -> Result<(), RepoError>;
    async fn remove_member(&self, team_id: &str, user_id: &str) -> Result<(), RepoError>;
}

/// See docs/project-abstraction-plan.md, stage A2. Not yet called from anywhere in
/// the running app — no service layer, no HTTP surface (that's A3/A5).
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ProjectRepo: Send + Sync {
    /// Creates the project row and seeds `owner_user_id` as an admin
    /// `project_members` row (points 0) — same shape as `TeamRepo::create` seeding
    /// the creator as admin.
    async fn create<'a>(
        &'a self,
        name: &'a str,
        owner_user_id: &'a str,
        team_id: Option<&'a str>,
    ) -> Result<String, RepoError>;
    async fn get(&self, project_id: &str) -> Result<Project, RepoError>;
    /// Stage B2's personal-item resolution: the caller's own team-less project, if
    /// any. Arbitrary pick if a user somehow has more than one (same accepted gap as
    /// stage B1's backfill migration — see docs/project-abstraction-plan.md).
    async fn find_personal_project(&self, user_id: &str) -> Result<Option<Project>, RepoError>;
    /// Stage B2's team-item resolution: the (at most one) project a team currently
    /// backs.
    async fn get_by_team(&self, team_id: &str) -> Result<Option<Project>, RepoError>;
    async fn update_name(&self, project_id: &str, name: &str) -> Result<(), RepoError>;
    /// Plain column write, no member-sync cascade — that's stage A4.
    async fn attach_team(&self, project_id: &str, team_id: &str) -> Result<(), RepoError>;
    /// Plain column write (`team_id` → NULL), no member-sync cascade — stage A4.
    async fn detach_team(&self, project_id: &str) -> Result<(), RepoError>;
    async fn delete(&self, project_id: &str) -> Result<(), RepoError>;
    async fn list_for_user(&self, user_id: &str) -> Result<Vec<Project>, RepoError>;
    async fn list_members(&self, project_id: &str) -> Result<Vec<ProjectMemberInfo>, RepoError>;
    async fn member_role(
        &self,
        project_id: &str,
        user_id: &str,
    ) -> Result<Option<TeamRole>, RepoError>;
    async fn set_member_role(
        &self,
        project_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<(), RepoError>;
    /// Mirrors `TeamRepo::count_active_admins` — `project_members` has no `status`
    /// column (row presence itself means active membership, see A4's sync/attach
    /// design), so this counts `role = 'admin'` rows only, no status filter. Added in
    /// stage C2 for the bootstrap-admin gate; see
    /// docs/project-abstraction-plan.md.
    async fn count_active_admins(&self, project_id: &str) -> Result<i64, RepoError>;
    /// Adds `delta` (negative to claw back) to `user_id`'s point balance on
    /// `project_id`, returning the resulting balance — the sole point-balance write
    /// path since stage C4 removed `TeamRepo::add_team_points`/`team_members.points`.
    async fn add_project_points(
        &self,
        project_id: &str,
        user_id: &str,
        delta: i32,
    ) -> Result<i64, RepoError>;
}

/// A project's subscriptions to external (Google Calendar) iCal feeds — see
/// docs/google-calendar-import-plan.md. Not yet called from anywhere in the running
/// app (Stage 2) — no service layer, no HTTP surface, no background sync (that's
/// Stages 3/5).
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait CalendarSubscriptionRepo: Send + Sync {
    async fn create(
        &self,
        project_id: &str,
        ical_url: &str,
        created_by_user_id: &str,
    ) -> Result<CalendarSubscription, RepoError>;
    async fn get(&self, id: &str) -> Result<CalendarSubscription, RepoError>;
    async fn list_by_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<CalendarSubscription>, RepoError>;
    async fn delete(&self, id: &str) -> Result<(), RepoError>;
    /// Every subscription across every project — the Stage 5 background sweep's
    /// entry point.
    async fn list_all(&self) -> Result<Vec<CalendarSubscription>, RepoError>;
    async fn record_sync_result(
        &self,
        id: &str,
        synced_at: DateTime<Utc>,
        error: Option<String>,
    ) -> Result<(), RepoError>;
}

/// Auto-generated "notify at the instant a date occurs" reminders for Task/Event items —
/// see `service::reminders::sync_item_reminders`, the sole writer. Stage 1 of the
/// reminders feature (`docs/issues_and_features.md`): schema + auto-population only, no
/// mutation UI/API and no delivery mechanism yet, so nothing else reads this table.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ReminderRepo: Send + Sync {
    /// Replaces every `source = 'AUTO'` reminder for `item_id` with `reminders` in one
    /// transaction (delete-then-insert, not a diff — nothing sets `sent_at` yet, so
    /// there's no in-flight state to preserve across an edit). Any `source = 'CUSTOM'`
    /// row (a future mutation-UI feature) is left untouched. An empty `reminders` slice
    /// still clears existing auto rows — e.g. a due date getting removed, or an item
    /// becoming unassigned on a team project.
    async fn sync_auto_reminders(
        &self,
        item_id: &str,
        project_id: &str,
        user_id: &str,
        reminders: &[(ReminderKind, DateTime<Utc>)],
    ) -> Result<(), RepoError>;
    /// Deletes every reminder (auto or custom) for `item_id` — called on item delete.
    async fn delete_for_item(&self, item_id: &str) -> Result<(), RepoError>;
    async fn list_for_item(&self, item_id: &str) -> Result<Vec<Reminder>, RepoError>;
}

/// "Depends on" (docs/issues_and_features.md) — a many-to-many `item_id -> depends_on_item_id`
/// relation, kept in its own table rather than a column on `items` since an item can depend on
/// more than one sibling. See `service::item_dependencies` for the validation this sits behind
/// (Task-only, same project, sibling-only) — this trait itself enforces nothing.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ItemDependencyRepo: Send + Sync {
    /// Replaces the full set of items `item_id` depends on with `depends_on_item_ids`
    /// (delete-then-insert, not a diff) — mirrors `ReminderRepo::sync_auto_reminders`'s
    /// full-replace shape. An empty slice clears every dependency.
    async fn set_dependencies(
        &self,
        item_id: &str,
        depends_on_item_ids: &[String],
    ) -> Result<(), RepoError>;
    /// The ids of every item `item_id` currently depends on.
    async fn list_for_item(&self, item_id: &str) -> Result<Vec<String>, RepoError>;
    /// Deletes every dependency row referencing `item_id`, on either side (as the dependent
    /// item or as the thing depended on) — called on item delete, so neither a deleted item's
    /// own dependency rows nor another item's now-dangling reference to it survive.
    async fn delete_for_item(&self, item_id: &str) -> Result<(), RepoError>;
}

/// Append-mostly completion/points log, kept separate from `ItemRepo`/`TeamRepo`
/// since it's not a CRUD resource — see CLAUDE.md's per-team roles/points design.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ActivityLogRepo: Send + Sync {
    /// Explicit `<'a>` lifetime on the trait method (same fix `ProjectRepo::create`
    /// needed — see docs/project-abstraction-plan.md stage A2's implementation notes):
    /// `#[async_trait]`'s desugaring can't elide a lifetime buried inside `Option<&str>`.
    async fn log_activity<'a>(
        &'a self,
        team_id: Option<&'a str>,
        project_id: Option<&'a str>,
        user_id: &'a str,
        item_id: &'a str,
        item_name: &'a str,
        points_delta: i32,
    ) -> Result<String, RepoError>;
    /// Server-capped by the caller — this trait has no pagination concept (the whole
    /// Smithy model has none; see CLAUDE.md), so `limit` is expected to already be
    /// clamped (e.g. `.min(100)`) before it reaches here.
    async fn list_activity_for_team(
        &self,
        team_id: &str,
        limit: i64,
    ) -> Result<Vec<ActivityLogEntry>, RepoError>;
    /// Stage B2's project_id-keyed read — see docs/project-abstraction-plan.md.
    /// `team_activity.rs` resolves the team's backing project and calls this instead
    /// of `list_activity_for_team`; the team-keyed method stays for the legacy
    /// `ListTeamActivityLog` JSON API operation, untouched until stage B4.
    async fn list_activity_for_project(
        &self,
        project_id: &str,
        limit: i64,
    ) -> Result<Vec<ActivityLogEntry>, RepoError>;
    async fn most_recent_unreversed(
        &self,
        item_id: &str,
        user_id: &str,
    ) -> Result<Option<ActivityLogEntry>, RepoError>;
    /// Fetches a single entry by id, regardless of team/reversed state — the manual
    /// undo endpoint (Stage 6) uses this to look up the entry before checking whether
    /// the caller is actually its own `user_id` and whether it's already reversed.
    async fn get_entry(&self, entry_id: &str) -> Result<ActivityLogEntry, RepoError>;
    async fn mark_reversed(&self, entry_id: &str) -> Result<(), RepoError>;
}

/// See docs/recurring-events-virtual-occurrences-rough-plan.md's staged breakdown.
/// Originally `EventSeriesRepo`/Event-only (stage 2); renamed and generalized to
/// also cover Task series at stage 7a — `item_series`/`item_occurrences` are the
/// renamed `event_series`/`event_occurrences` tables (see `AddItemSeries`, the
/// migration that renamed+migrated the data).
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ItemSeriesRepo: Send + Sync {
    /// Ignores `series.id` and generates a fresh one server-side, same convention
    /// as `ItemRepo::create`/`ProjectRepo::create`.
    async fn create_series(&self, series: &ItemSeries) -> Result<String, RepoError>;
    /// Full-replace update of name/description/event_type/recurrence/anchor_date/item_type.
    /// `series.id`/`series.project_id` are ignored — `series_id` is the target, and
    /// project_id is immutable (mirrors `create_series`'s "ignores `series.id`"
    /// convention, extended to the one other identity-shaped field).
    async fn update_series(&self, series_id: &str, series: &ItemSeries) -> Result<(), RepoError>;
    async fn get_series(&self, series_id: &str) -> Result<ItemSeries, RepoError>;
    async fn list_series_for_project(&self, project_id: &str)
    -> Result<Vec<ItemSeries>, RepoError>;
    async fn get_occurrence(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<Option<ItemOccurrence>, RepoError>;
    /// Upserts the `(series_id, occurrence_date)` row with a materialized
    /// `item_id`, clearing any prior `is_exdate` — the write
    /// `get_or_materialize_occurrence` (stage 3) calls the first time a virtual
    /// occurrence is touched.
    async fn record_materialized_occurrence(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
        item_id: &str,
    ) -> Result<(), RepoError>;
    /// Upserts the `(series_id, occurrence_date)` row with `is_exdate = true` and
    /// `item_id` cleared — the EXDATE-equivalent "skip this date" marker, and the
    /// *only* path that should ever set `is_exdate`. Deleting a materialized
    /// occurrence's item does **not** call this (see `delete_occurrence` below) —
    /// 2026-08-16, second pass: an item delete un-materializes the occurrence
    /// instead, since "I deleted this item" and "I explicitly excluded this date
    /// from the series forever" are different intents, and only Skip means the
    /// latter.
    async fn mark_exdate(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError>;
    /// Deletes the `(series_id, occurrence_date)` row outright, if one exists —
    /// un-materializes the occurrence rather than excluding it, so it goes back to
    /// being a plain virtual occurrence: re-materializable, and (if it's the
    /// series' current occurrence) still current, just itemless again rather than
    /// stuck. Called from `service::item_series::unlink_deleted_item_occurrence`
    /// when a series-linked item is deleted — deliberately distinct from
    /// `mark_exdate`, which stays the one and only way to actually exclude a date
    /// (via the explicit Skip action). A no-op (not an error) if no row exists for
    /// the date, matching `mark_exdate`'s upsert-style tolerance of either state.
    async fn delete_occurrence(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError>;
    /// Only returns rows that exist in `event_occurrences` (materialized or
    /// exdate) — purely virtual dates within the range have no row and aren't
    /// included here; callers combine this with `recurrence::occurrences_between`
    /// to get the full picture.
    async fn list_occurrences_between(
        &self,
        series_id: &str,
        range_start: DateTime<Utc>,
        range_end: DateTime<Utc>,
    ) -> Result<Vec<ItemOccurrence>, RepoError>;
    /// Reverse lookup used when an item is deleted (`service::project_items::delete_project_item`)
    /// to find whether it was a materialized series occurrence, so the occurrence row can be
    /// deleted (un-materializing it) rather than left pointing at a now-deleted `item_id`. `None`
    /// for an item that never came from a series — the overwhelmingly common case, so callers
    /// must treat `None` as a normal, cheap no-op rather than something to log or special-case.
    async fn find_occurrence_by_item_id(
        &self,
        item_id: &str,
    ) -> Result<Option<ItemOccurrence>, RepoError>;
    /// Stage 9: forward-only cursor advance for a Task-typed series — sets
    /// `cursor_date = MAX(cursor_date, occurrence_date)` atomically (a `NULL` cursor
    /// counts as `occurrence_date` itself), so completing/skipping occurrences out of
    /// order can never move the cursor backward. Called from both the completion hook
    /// (`service::item_series::record_task_completion`) and `skip_occurrence` — the two
    /// actions that "settle" an occurrence, per docs/recurring-events-virtual-occurrences-rough-plan.md's
    /// Stage 9 cursor design.
    async fn advance_cursor(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError>;

    /// Un-settles a Task series' cursor back to one step before `occurrence_date` —
    /// the counterpart un-completing an item calls (`service::item_series::record_task_uncompletion`)
    /// after `advance_cursor` moved it forward. `MIN`-guarded like `advance_cursor` is
    /// `MAX`-guarded, so it can only ever move the cursor backward.
    async fn retreat_cursor(
        &self,
        series_id: &str,
        occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError>;

    /// Clears a Task series' cursor back to its pre-anything-settled `None` state —
    /// used only when un-completing the series' very first (anchor) occurrence, since
    /// there's no earlier occurrence for `retreat_cursor` to land on. Guarded on the
    /// cursor still being exactly at `expected_occurrence_date`, so a concurrent
    /// settlement that has since moved the cursor elsewhere is left untouched rather
    /// than clobbered.
    async fn clear_cursor(
        &self,
        series_id: &str,
        expected_occurrence_date: DateTime<Utc>,
    ) -> Result<(), RepoError>;

    /// Deletes the `item_series` row and all its `item_occurrences` rows. Orphan, not
    /// cascade — never touches `items`; every already-materialized occurrence survives
    /// as a plain standalone item. See item_series.smithy's `DeleteItemSeries` doc
    /// comment for the rationale.
    async fn delete_series(&self, series_id: &str) -> Result<(), RepoError>;

    /// The series' rotation membership, sorted by `user_id` — this sort order *is* the
    /// cycle order (docs/assignment-rotation-plan.md's "unordered set, stable derived
    /// order" decision), not just a display nicety, so callers computing
    /// `rotation[index % len]` must use this method rather than re-sorting themselves.
    /// Empty `Vec` (not an error) for a series with no rotation configured, same
    /// not-found-is-empty convention `list_occurrences_between` already follows.
    async fn list_rotation_members(&self, series_id: &str) -> Result<Vec<String>, RepoError>;
    /// Full-replace of `series_id`'s rotation membership — deletes every existing row
    /// for this series and reinserts `user_ids`, in one transaction so a reader never
    /// observes a partial set. Passing an empty slice clears the rotation entirely
    /// (the service layer, not this method, is what rejects an ambiguous "empty but
    /// explicitly rotating" request — see resolve_series_assignment). Does not validate
    /// `user_ids` against project membership; that's the service layer's job, same
    /// division `create_series`/`update_series` already follow for `assigned_to_user_id`.
    async fn set_rotation_members(
        &self,
        series_id: &str,
        user_ids: &[String],
    ) -> Result<(), RepoError>;
}

fn db_err(e: sqlx::Error) -> RepoError {
    RepoError::Internal(e.to_string())
}

fn not_found() -> RepoError {
    RepoError::NotFound
}

fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("id"),
        first_name: row.get("first_name"),
        last_name: row.get("last_name"),
        email: row.get("email"),
        google_id: row.get("google_id"),
        timezone: row.get("timezone"),
        personal_project_id: row.get("personal_project_id"),
    }
}

/// Reconstructs whichever `ItemType` variant matches the stored `item_type` column,
/// folding the flat DB columns (unchanged schema — see CLAUDE.md's storage section)
/// into that variant's payload. Columns that don't apply to the resolved variant
/// (e.g. `points` on a row that turns out to be an `Event`) are simply dropped here;
/// the write side (`items.rs`'s INSERT/UPDATE) is what keeps them from being written
/// in the first place for the wrong kind.
fn row_to_item(row: &sqlx::sqlite::SqliteRow) -> Item {
    let due_date_secs: Option<i64> = row.get("due_date");
    let scheduled_secs: Option<i64> = row.get("scheduled_date");
    let scheduled_end_secs: Option<i64> = row.get("scheduled_end_date");
    let complete: Option<i64> = row.get("complete");

    let schedule = Schedule {
        due_date: due_date_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        has_due_time: row.get::<Option<i64>, _>("has_due_time").unwrap_or(0) != 0,
        scheduled_date: scheduled_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        has_scheduled_time: row.get::<Option<i64>, _>("has_scheduled_time").unwrap_or(0) != 0,
        scheduled_end_date: scheduled_end_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        has_end_time: row.get::<Option<i64>, _>("has_end_time").unwrap_or(0) != 0,
    };
    let recurrence = Recurrence {
        pattern: row.get("recurrence"),
        basis: row.get("recurrence_basis"),
        due_offset_days: row.get("due_offset_days"),
    };
    let event_type: Option<String> = row.get("event_type");
    let assigned_to_user_id: Option<String> = row.get("assigned_to_user_id");
    let points: Option<i32> = row.get("points");
    let source_event_id: Option<String> = row.get("source_event_id");

    let kind: ItemKind = row
        .get::<Option<String>, _>("item_type")
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();

    let item_type = match kind {
        ItemKind::Task => ItemType::Task {
            schedule,
            recurrence,
            team_assignment: if assigned_to_user_id.is_some() || points.is_some() {
                Some(TeamAssignment {
                    assigned_to_user_id,
                    points,
                })
            } else {
                None
            },
            source_event_id,
        },
        ItemKind::Event => ItemType::Event {
            schedule,
            recurrence,
            event_type,
        },
        ItemKind::Template => ItemType::Template {
            schedule,
            recurrence,
            event_type,
        },
        ItemKind::Simple => ItemType::Simple,
    };

    Item {
        id: row.get("id"),
        user_id: row.get("user_id"),
        project_id: row.get("project_id"),
        parent_item_id: row.get("parent_item_id"),
        name: row.get("name"),
        description: row.get("description"),
        complete: complete.unwrap_or(0) != 0,
        has_children: row.get::<Option<i64>, _>("has_children").unwrap_or(0) != 0,
        item_type,
        series_id: row.get("series_id"),
        google_event_id: row.get("google_event_id"),
        calendar_subscription_id: row.get("calendar_subscription_id"),
    }
}

fn row_to_activity_log_entry(row: &sqlx::sqlite::SqliteRow) -> ActivityLogEntry {
    let created_at_secs: i64 = row.get("created_at");
    let reversed: i64 = row.get("reversed");
    ActivityLogEntry {
        id: row.get("id"),
        team_id: row.get("team_id"),
        project_id: row.get("project_id"),
        user_id: row.get("user_id"),
        item_id: row.get("item_id"),
        item_name: row.get("item_name"),
        points_delta: row.get("points_delta"),
        reversed: reversed != 0,
        created_at: chrono::DateTime::from_timestamp(created_at_secs, 0)
            .unwrap_or_default()
            .with_timezone(&chrono::Utc),
    }
}

fn row_to_calendar_subscription(row: &sqlx::sqlite::SqliteRow) -> CalendarSubscription {
    let created_at_secs: i64 = row.get("created_at");
    let last_synced_at_secs: Option<i64> = row.get("last_synced_at");
    CalendarSubscription {
        id: row.get("id"),
        project_id: row.get("project_id"),
        ical_url: row.get("ical_url"),
        created_by_user_id: row.get("created_by_user_id"),
        created_at: chrono::DateTime::from_timestamp(created_at_secs, 0)
            .unwrap_or_default()
            .with_timezone(&chrono::Utc),
        last_synced_at: last_synced_at_secs
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        last_sync_error: row.get("last_sync_error"),
    }
}

pub async fn create_pool(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(url).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            email TEXT,
            google_id TEXT UNIQUE,
            timezone TEXT,
            personal_project_id TEXT
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY,
            user_id TEXT,
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
            source_event_id TEXT,
            project_id TEXT,
            series_id TEXT,
            google_event_id TEXT,
            calendar_subscription_id TEXT
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_user_id ON items (user_id)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_parent_id ON items (parent_item_id)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_items_assigned_to ON items (assigned_to_user_id)")
        .execute(&pool)
        .await?;
    // idx_items_series_id is deliberately NOT created here — same index-ordering reason as
    // idx_items_project_id below: `series_id` is in this baseline `CREATE TABLE` for a fresh
    // DB, but an existing DB that predates it hits this statement with `CREATE TABLE IF NOT
    // EXISTS` as a no-op (table already exists, column doesn't), and this index creation ran
    // unconditionally, before `run_migrations()` ever got a chance to add the column via
    // `AddItemSeriesId` — "no such column: series_id" on every startup against such a DB.
    // `AddItemSeriesId` (migration version 23) creates this index itself, after adding the
    // column if missing, exactly like `idx_items_project_id`'s own migration does.

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS teams (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS team_members (
            team_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'PENDING',
            invited_by TEXT,
            role TEXT NOT NULL DEFAULT 'member',
            PRIMARY KEY (team_id, user_id)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_team_members_user_id ON team_members (user_id)")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS activity_log (
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
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_activity_log_team_created ON activity_log (team_id, created_at DESC)",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_activity_log_item_id ON activity_log (item_id)")
        .execute(&pool)
        .await?;
    // idx_activity_log_project_id is deliberately NOT created here — same
    // index-ordering reason as idx_items_project_id above: it lives in
    // backfill_projects.rs, the migration that added the column, not the baseline.

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            owner_user_id TEXT NOT NULL,
            team_id TEXT
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_projects_team_id ON projects (team_id)")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS project_members (
            project_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'member',
            points INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (project_id, user_id)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_project_members_user_id ON project_members (user_id)",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS event_series (
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
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_event_series_project_id ON event_series (project_id)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS event_occurrences (
            series_id TEXT NOT NULL,
            occurrence_date INTEGER NOT NULL,
            item_id TEXT,
            is_exdate INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (series_id, occurrence_date)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_event_occurrences_item_id ON event_occurrences (item_id)",
    )
    .execute(&pool)
    .await?;

    // Renamed/generalized from event_series/event_occurrences above at stage 7a of
    // docs/recurring-events-virtual-occurrences-rough-plan.md — the old tables are left
    // in place, unread, matching this codebase's precedent of not force-dropping
    // superseded schema (see CLAUDE.md's Storage Layer section on `items.user_id`).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS item_series (
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
            template_item_id TEXT,
            assigned_to_user_id TEXT,
            points INTEGER
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_item_series_project_id ON item_series (project_id)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS item_occurrences (
            series_id TEXT NOT NULL,
            occurrence_date INTEGER NOT NULL,
            item_id TEXT,
            is_exdate INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (series_id, occurrence_date)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_item_occurrences_item_id ON item_occurrences (item_id)",
    )
    .execute(&pool)
    .await?;
    // Assignment rotation (docs/assignment-rotation-plan.md) — an unordered set of
    // project-member user ids a Task-typed series rotates its materialized occurrences'
    // assignee across. No `position` column: cycle order is derived at read time via
    // `ORDER BY user_id ASC` (see ItemSeriesRepo::list_rotation_members), not
    // separately authored — deliberately simpler than a position-tracked ordered list
    // per that plan's decision. Mutually exclusive with `item_series.assigned_to_user_id`
    // (enforced at the service layer, not here).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS item_series_rotation_members (
            series_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            PRIMARY KEY (series_id, user_id)
        )",
    )
    .execute(&pool)
    .await?;

    // See docs/google-calendar-import-plan.md, Stage 1. Brand-new table, so (like
    // `projects`/`project_members` above) it's safe to create — table and index both —
    // directly in the baseline; `AddCalendarSubscriptions` creates the identical pair
    // again for a DB that predates this migration.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS calendar_subscriptions (
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
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_calendar_subscriptions_project_id \
         ON calendar_subscriptions (project_id)",
    )
    .execute(&pool)
    .await?;

    // Stage 1 of the reminders feature (docs/issues_and_features.md) — schema +
    // auto-population only, see service::reminders::sync_item_reminders, the sole
    // writer. Brand-new table, same "safe directly in the baseline" precedent as
    // calendar_subscriptions above; AddReminders creates the identical pair for a DB
    // that predates this migration. idx_reminders_item_id is exercised immediately by
    // sync_auto_reminders/delete_for_item; idx_reminders_user_remind_at isn't read by
    // anything yet but is exactly what a future delivery sweep ("every unsent reminder
    // due now, per user") will need.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS reminders (
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
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_reminders_item_id ON reminders (item_id)")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_reminders_user_remind_at \
         ON reminders (user_id, remind_at)",
    )
    .execute(&pool)
    .await?;

    // "Depends on" (docs/issues_and_features.md) — see `ItemDependencyRepo`. Brand-new
    // table, same "safe directly in the baseline" precedent as reminders above;
    // `AddItemDependencies` creates the identical pair for a DB that predates this
    // migration. Both directions are indexed: `idx_item_dependencies_item_id` for
    // `list_for_item`/`set_dependencies`'s own delete-then-insert, and
    // `idx_item_dependencies_depends_on` for `delete_for_item`'s reverse-side cleanup
    // (a deleted item's dangling references from other items' rows).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS item_dependencies (
            item_id TEXT NOT NULL,
            depends_on_item_id TEXT NOT NULL,
            PRIMARY KEY (item_id, depends_on_item_id)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_item_dependencies_item_id \
         ON item_dependencies (item_id)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_item_dependencies_depends_on \
         ON item_dependencies (depends_on_item_id)",
    )
    .execute(&pool)
    .await?;

    // idx_items_calendar_subscription_id / idx_items_calsub_google_event_id are
    // deliberately NOT created here — same index-ordering reason as idx_items_project_id
    // below: `items.calendar_subscription_id`/`items.google_event_id` are added to an
    // *existing* table by `AddCalendarSubscriptions`, so both indexes live inside that
    // migration, not the baseline.

    // idx_items_project_id is deliberately NOT created here — see add_projects.rs's
    // doc comment: an index on a column added to an *existing* table via a migration
    // must live inside that migration, not the baseline, since baseline indexes run
    // before run_migrations() and would fail against any DB that predates the ALTER
    // TABLE that adds the column (this bit us once already for source_event_id).

    // Every CREATE TABLE/INDEX IF NOT EXISTS baseline statement above must run before
    // this — migrations may target any of those tables (e.g. AddTeamMemberRole alters
    // team_members), and on a brand-new DB they wouldn't exist yet otherwise.
    crate::storage::migrations::run_migrations(&pool)
        .await
        .map_err(|crate::storage::migrations::MigrationError::Database(e)| e)?;
    Ok(pool)
}
