use super::{Migration, MigrationError, column_exists};
use async_trait::async_trait;
use sqlx::{Row, SqliteConnection};

/// Backfills `projects`/`project_members`/`items.project_id`/`activity_log.project_id`
/// against data that predates the Project abstraction (see the Project-abstraction
/// plan's stage B1) — the one migration in this sequence that mutates real rows rather
/// than just adding empty schema. Everything here is skip-existing/`NOT EXISTS`-guarded
/// so it's safe to run against a DB that's already partially backfilled (in particular,
/// `ensure_default_project`, stage A5, has been creating personal projects live in
/// production on every login since A5 shipped — by the time this migration runs, some
/// users already have one).
///
/// **Personal projects:** every user without an existing personal project (a `projects`
/// row with `team_id IS NULL AND owner_user_id = users.id`) gets one named `"Personal"`,
/// matching `ensure_default_project`'s naming — plus the same owner-as-admin
/// `project_members` row `ProjectRepo::create`/`SqliteProjectRepo::create` seed on every
/// create, since this backfill is deliberately mirroring that path rather than going
/// through it (a migration can't call service-layer code).
///
/// **Team projects:** every team without an existing attached project gets one:
/// `name = teams.name`, `team_id = teams.id`. `owner_user_id` is a deterministic pick —
/// the lowest `user_id` among the team's current `ACTIVE` admins, falling back to the
/// lowest `user_id` among any `ACTIVE` member if the team somehow has no active admin
/// (see the plan's "Team-backed project owner_user_id backfill" decision: `owner_user_id`
/// is write-once-and-ignored once a project has a `team_id`, so which admin gets picked
/// here is low-stakes). A team with zero active members at all (shouldn't happen in
/// practice — `TeamRepo::create` always seeds the creator as an ACTIVE admin) has no
/// valid owner to backfill and is skipped entirely, same as the plan's silence on that
/// case — its items simply won't get a `project_id` from this migration.
/// `project_members` is then seeded from the team's current `ACTIVE` members the same
/// way `SqliteProjectRepo::attach_team` does (role `member`, points `0`, `NOT EXISTS`
/// guard so the owner's own already-inserted admin row is never clobbered).
///
/// **`items.project_id` backfill:** personal items resolve via `user_id` against the
/// user's `team_id IS NULL` project; team items resolve via `team_id` against that
/// team's project. Matches the plan's SQL verbatim — including its one known blind
/// spot: if a user has ever created more than one personal (team-less) project of
/// their own (possible since stage A5's `CreateProject` operation shipped), the
/// personal-item subquery has no way to disambiguate which one an item belongs to and
/// SQLite will pick one arbitrarily. Not handled here — flagged in the plan as an
/// accepted risk, not something this migration needs to solve.
///
/// **`activity_log.project_id`:** new nullable column (guarded with `column_exists`,
/// per convention), backfilled from each row's `team_id` via the same `projects.team_id`
/// join. Its index is created here, not in the `create_pool()` baseline — per
/// `add_projects.rs`'s doc comment (and CLAUDE.md's index-ordering note): an index on a
/// column added to an *existing* table by a migration must live inside that migration,
/// not the baseline, since baseline indexes run before `run_migrations()` and would
/// fail against any DB that predates the `ALTER TABLE`. `activity_log.team_id` itself is
/// untouched — it stays populated and readable until stage B2 cuts reads/writes over to
/// `project_id`; actual column drop is stage C.
pub struct BackfillProjects;

#[async_trait]
impl Migration for BackfillProjects {
    fn version(&self) -> i64 {
        11
    }

    fn name(&self) -> &str {
        "backfill projects, project_members, items.project_id, activity_log.project_id"
    }

    async fn up(&self, conn: &mut SqliteConnection) -> Result<(), MigrationError> {
        let user_ids: Vec<String> = sqlx::query(
            "SELECT id FROM users WHERE NOT EXISTS (
                 SELECT 1 FROM projects
                 WHERE projects.owner_user_id = users.id AND projects.team_id IS NULL
             )",
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("id"))
        .collect();

        for user_id in user_ids {
            let project_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO projects (id, name, owner_user_id, team_id) \
                 VALUES (?, 'Personal', ?, NULL)",
            )
            .bind(&project_id)
            .bind(&user_id)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "INSERT INTO project_members (project_id, user_id, role, points) \
                 VALUES (?, ?, 'admin', 0)",
            )
            .bind(&project_id)
            .bind(&user_id)
            .execute(&mut *conn)
            .await?;
        }

        let team_ids: Vec<String> = sqlx::query(
            "SELECT id FROM teams WHERE NOT EXISTS (
                 SELECT 1 FROM projects WHERE projects.team_id = teams.id
             )",
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("id"))
        .collect();

        for team_id in team_ids {
            let team_name: String = sqlx::query_scalar("SELECT name FROM teams WHERE id = ?")
                .bind(&team_id)
                .fetch_one(&mut *conn)
                .await?;

            let admin_owner: Option<String> = sqlx::query_scalar(
                "SELECT user_id FROM team_members \
                 WHERE team_id = ? AND status = 'ACTIVE' AND role = 'admin' \
                 ORDER BY user_id ASC LIMIT 1",
            )
            .bind(&team_id)
            .fetch_optional(&mut *conn)
            .await?;

            let owner_user_id = match admin_owner {
                Some(id) => Some(id),
                None => {
                    sqlx::query_scalar(
                        "SELECT user_id FROM team_members \
                         WHERE team_id = ? AND status = 'ACTIVE' \
                         ORDER BY user_id ASC LIMIT 1",
                    )
                    .bind(&team_id)
                    .fetch_optional(&mut *conn)
                    .await?
                }
            };

            let Some(owner_user_id) = owner_user_id else {
                continue;
            };

            let project_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO projects (id, name, owner_user_id, team_id) VALUES (?, ?, ?, ?)",
            )
            .bind(&project_id)
            .bind(&team_name)
            .bind(&owner_user_id)
            .bind(&team_id)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "INSERT INTO project_members (project_id, user_id, role, points) \
                 VALUES (?, ?, 'admin', 0)",
            )
            .bind(&project_id)
            .bind(&owner_user_id)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "INSERT INTO project_members (project_id, user_id, role, points)
                 SELECT ?, team_members.user_id, 'member', 0
                 FROM team_members
                 WHERE team_members.team_id = ? AND team_members.status = 'ACTIVE'
                   AND NOT EXISTS (
                       SELECT 1 FROM project_members
                       WHERE project_members.project_id = ?
                         AND project_members.user_id = team_members.user_id
                   )",
            )
            .bind(&project_id)
            .bind(&team_id)
            .bind(&project_id)
            .execute(&mut *conn)
            .await?;
        }

        sqlx::query(
            "UPDATE items SET project_id = (
                 SELECT id FROM projects WHERE owner_user_id = items.user_id AND team_id IS NULL
             ) WHERE user_id IS NOT NULL AND project_id IS NULL",
        )
        .execute(&mut *conn)
        .await?;
        // `items.team_id` was dropped in Stage 6 of docs/team-id-removal-plan.md
        // (`drop_items_team_id.rs`, version 14 — this migration is version 11, so on
        // any DB that still needs migrating from scratch, the column is present when
        // this step runs and gets dropped later). A DB already on the current
        // baseline schema (fresh install, or fully migrated already) has no
        // `items.team_id` column at all, so this step has nothing to backfill from —
        // skip it rather than referencing a column that may not exist.
        if column_exists(conn, "items", "team_id").await? {
            sqlx::query(
                "UPDATE items SET project_id = (
                     SELECT id FROM projects WHERE projects.team_id = items.team_id
                 ) WHERE team_id IS NOT NULL AND project_id IS NULL",
            )
            .execute(&mut *conn)
            .await?;
        }

        if !column_exists(conn, "activity_log", "project_id").await? {
            sqlx::query("ALTER TABLE activity_log ADD COLUMN project_id TEXT")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query(
            "UPDATE activity_log SET project_id = (
                 SELECT id FROM projects WHERE projects.team_id = activity_log.team_id
             ) WHERE project_id IS NULL",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_activity_log_project_id ON activity_log (project_id)",
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn empty_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        SqlitePoolOptions::new().connect_with(opts).await.unwrap()
    }

    async fn schema_pool() -> SqlitePool {
        let pool = empty_pool().await;
        sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY, first_name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                team_id TEXT,
                name TEXT NOT NULL,
                project_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE teams (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE team_members (
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
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE activity_log (
                id TEXT PRIMARY KEY,
                team_id TEXT NOT NULL,
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
        pool
    }

    async fn project_row(pool: &SqlitePool, owner: &str, team: Option<&str>) -> Option<String> {
        sqlx::query_scalar(
            "SELECT id FROM projects WHERE owner_user_id = ? AND team_id IS ?",
        )
        .bind(owner)
        .bind(team)
        .fetch_optional(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn creates_a_personal_project_for_every_user_missing_one() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO users (id, first_name) VALUES ('u1', 'Ann')")
            .execute(&pool)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();

        let project_id = project_row(&pool, "u1", None).await.unwrap();
        let name: String = sqlx::query_scalar("SELECT name FROM projects WHERE id = ?")
            .bind(&project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "Personal");
        let role: String = sqlx::query_scalar(
            "SELECT role FROM project_members WHERE project_id = ? AND user_id = 'u1'",
        )
        .bind(&project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(role, "admin");
    }

    #[tokio::test]
    async fn skips_a_user_who_already_has_a_personal_project() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO users (id, first_name) VALUES ('u1', 'Ann')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO projects (id, name, owner_user_id, team_id) VALUES ('p1', 'Personal', 'u1', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM projects WHERE owner_user_id = 'u1' AND team_id IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn creates_a_team_project_owned_by_the_lowest_active_admin() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('t1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role) VALUES \
             ('t1', 'zeta', 'ACTIVE', 'admin'), \
             ('t1', 'alpha', 'ACTIVE', 'admin'), \
             ('t1', 'beta', 'ACTIVE', 'member'), \
             ('t1', 'gamma', 'PENDING', 'member')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();

        let project_id = project_row(&pool, "alpha", Some("t1")).await.unwrap();
        let name: String = sqlx::query_scalar("SELECT name FROM projects WHERE id = ?")
            .bind(&project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "Family");

        let member_roles: Vec<(String, String)> = sqlx::query(
            "SELECT user_id, role FROM project_members WHERE project_id = ? ORDER BY user_id",
        )
        .bind(&project_id)
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get("user_id"), r.get("role")))
        .collect();
        assert_eq!(
            member_roles,
            vec![
                ("alpha".to_string(), "admin".to_string()),
                ("beta".to_string(), "member".to_string()),
                ("zeta".to_string(), "member".to_string()),
            ]
        );
        // gamma is only PENDING, so no project_members row was seeded for them.
        let gamma_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM project_members WHERE project_id = ? AND user_id = 'gamma'",
        )
        .bind(&project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(gamma_count, 0);
    }

    #[tokio::test]
    async fn falls_back_to_any_active_member_when_the_team_has_no_admin() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('t1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role) VALUES \
             ('t1', 'only', 'ACTIVE', 'member')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();

        let project_id = project_row(&pool, "only", Some("t1")).await;
        assert!(project_id.is_some());
    }

    #[tokio::test]
    async fn skips_a_team_that_already_has_an_attached_project() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('t1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO projects (id, name, owner_user_id, team_id) VALUES ('p1', 'Family', 'u1', 't1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE team_id = 't1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn backfills_items_project_id_for_personal_and_team_items() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO users (id, first_name) VALUES ('u1', 'Ann')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO teams (id, name) VALUES ('t1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role) VALUES ('t1', 'u1', 'ACTIVE', 'admin')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO items (id, user_id, team_id, name) VALUES \
             ('i1', 'u1', NULL, 'Personal task'), \
             ('i2', NULL, 't1', 'Team task')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();

        let personal_project_id = project_row(&pool, "u1", None).await.unwrap();
        let team_project_id = project_row(&pool, "u1", Some("t1")).await.unwrap();

        let i1_project: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM items WHERE id = 'i1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(i1_project, Some(personal_project_id));

        let i2_project: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM items WHERE id = 'i2'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(i2_project, Some(team_project_id));
    }

    #[tokio::test]
    async fn adds_and_backfills_activity_log_project_id() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('t1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role) VALUES ('t1', 'u1', 'ACTIVE', 'admin')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO activity_log (id, team_id, user_id, item_id, item_name, points_delta, created_at) \
             VALUES ('a1', 't1', 'u1', 'i1', 'Wash dishes', 5, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();

        assert!(column_exists(&mut conn, "activity_log", "project_id").await.unwrap());
        let team_project_id = project_row(&pool, "u1", Some("t1")).await.unwrap();
        let log_project: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM activity_log WHERE id = 'a1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(log_project, Some(team_project_id));
    }

    #[tokio::test]
    async fn running_up_twice_creates_no_duplicate_rows() {
        let pool = schema_pool().await;
        sqlx::query("INSERT INTO users (id, first_name) VALUES ('u1', 'Ann')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO teams (id, name) VALUES ('t1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role) VALUES ('t1', 'u1', 'ACTIVE', 'admin')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();

        BackfillProjects.up(&mut conn).await.unwrap();
        BackfillProjects.up(&mut conn).await.unwrap();

        let project_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(project_count, 2);
        let member_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_members")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(member_count, 2);
    }
}
