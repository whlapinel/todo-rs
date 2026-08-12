pub mod activity_log;
pub mod projects;
pub mod teams;
pub mod users;
pub mod items;
use async_trait::async_trait;
use sqlx::{Row, SqlitePool};
use crate::domain::{
    activity_log::ActivityLogEntry,
    item::{Item, ItemKind, ItemType, Recurrence, Schedule, TeamAssignment},
    project::Project,
    team::{Team, TeamRole},
    user::User,
};

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
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ItemRepo: Send + Sync {
    async fn get(&self, user_id: &str, item_id: &str) -> Result<Item, RepoError>;
    async fn get_team_item(&self, team_id: &str, item_id: &str) -> Result<Item, RepoError>;
    async fn list(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn list_team_items(
        &self,
        team_id: &str,
        parent_item_id: Option<String>,
    ) -> Result<Vec<Item>, RepoError>;
    async fn list_children(&self, parent_item_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn list_by_source_event(&self, source_event_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn create(&self, item: &Item) -> Result<String, RepoError>;
    async fn update(&self, item: &Item) -> Result<(), RepoError>;
    async fn update_team_item(&self, item: &Item) -> Result<(), RepoError>;
    async fn delete(&self, item_id: &str) -> Result<(), RepoError>;
    async fn list_due(
        &self,
        user_id: &str,
        deadline_after: Option<i64>,
        deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError>;
    async fn list_due_team_items(
        &self,
        team_id: &str,
        deadline_after: Option<i64>,
        deadline_before: Option<i64>,
    ) -> Result<Vec<DueItem>, RepoError>;
    async fn list_templates(&self, user_id: &str) -> Result<Vec<Item>, RepoError>;
    async fn list_team_templates(&self, team_id: &str) -> Result<Vec<Item>, RepoError>;
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
    /// Adds `delta` (negative to claw back) to `user_id`'s point balance on `team_id`,
    /// returning the resulting balance. See CLAUDE.md's Points plan, Stage 6 —
    /// completion awards a positive delta, reversal (automatic on un-complete, or via
    /// the manual undo endpoint for recurring items) applies the negation of whatever
    /// the originating `activity_log` entry recorded.
    async fn add_team_points(
        &self,
        team_id: &str,
        user_id: &str,
        delta: i32,
    ) -> Result<i64, RepoError>;
    async fn invite(
        &self,
        team_id: &str,
        invitee_user_id: &str,
        invited_by: &str,
    ) -> Result<(), RepoError>;
    async fn accept(&self, team_id: &str, user_id: &str) -> Result<(), RepoError>;
    async fn remove_member(&self, team_id: &str, user_id: &str) -> Result<(), RepoError>;
    async fn share_active_team(&self, user_a: &str, user_b: &str) -> Result<bool, RepoError>;
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
    /// Adds `delta` (negative to claw back) to `user_id`'s point balance on
    /// `project_id`, returning the resulting balance — mirrors `add_team_points`.
    async fn add_project_points(
        &self,
        project_id: &str,
        user_id: &str,
        delta: i32,
    ) -> Result<i64, RepoError>;
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
        team_id: &'a str,
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
        team_id: row.get("team_id"),
        project_id: row.get("project_id"),
        parent_item_id: row.get("parent_item_id"),
        name: row.get("name"),
        description: row.get("description"),
        complete: complete.unwrap_or(0) != 0,
        has_children: row.get::<Option<i64>, _>("has_children").unwrap_or(0) != 0,
        item_type,
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

pub async fn create_pool(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(url).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            email TEXT,
            google_id TEXT UNIQUE
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY,
            user_id TEXT,
            team_id TEXT,
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
            project_id TEXT
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
            points INTEGER NOT NULL DEFAULT 0,
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
            team_id TEXT NOT NULL,
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
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_project_members_user_id ON project_members (user_id)")
        .execute(&pool)
        .await?;
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
