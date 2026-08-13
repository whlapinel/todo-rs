use async_trait::async_trait;
use std::str::FromStr;

use crate::storage::sqlite::{TeamRepo, TeamMemberInfo, TeamWithStatus, RepoError, db_err, not_found, row_to_user};
use sqlx::{Row, SqlitePool};

use crate::domain::team::{Team, TeamRole};

pub struct SqliteTeamRepo(pub SqlitePool);

#[async_trait]
impl TeamRepo for SqliteTeamRepo {
    async fn create(&self, name: &str, creator_user_id: &str) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO teams (id, name) VALUES (?, ?)")
            .bind(&id)
            .bind(name)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, invited_by, role) VALUES (?, ?, 'ACTIVE', NULL, 'admin')",
        )
        .bind(&id)
        .bind(creator_user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn get(&self, team_id: &str) -> Result<Team, RepoError> {
        sqlx::query("SELECT id, name FROM teams WHERE id = ?")
            .bind(team_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| Team {
                id: row.get("id"),
                name: row.get("name"),
            })
            .ok_or_else(not_found)
    }

    async fn update_name(&self, team_id: &str, name: &str) -> Result<(), RepoError> {
        let result = sqlx::query("UPDATE teams SET name = ? WHERE id = ?")
            .bind(name)
            .bind(team_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(not_found());
        }
        Ok(())
    }

    async fn list_for_user(&self, user_id: &str) -> Result<Vec<TeamWithStatus>, RepoError> {
        sqlx::query(
            "SELECT teams.id, teams.name, team_members.status,
                    inviter.first_name AS inviter_first_name, inviter.last_name AS inviter_last_name
             FROM team_members
             JOIN teams ON team_members.team_id = teams.id
             LEFT JOIN users inviter ON team_members.invited_by = inviter.id
             WHERE team_members.user_id = ?
             ORDER BY teams.name ASC",
        )
        .bind(user_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let inviter_first: Option<String> = row.get("inviter_first_name");
                    let inviter_last: Option<String> = row.get("inviter_last_name");
                    let invited_by_name = inviter_first.map(|f| match inviter_last {
                        Some(l) if !l.is_empty() => format!("{f} {l}"),
                        _ => f,
                    });
                    TeamWithStatus {
                        team: Team {
                            id: row.get("id"),
                            name: row.get("name"),
                        },
                        status: row.get("status"),
                        invited_by_name,
                    }
                })
                .collect()
        })
    }

    /// `points` is sourced from the team's backing project's `project_members` row
    /// (stage C1, docs/project-abstraction-plan.md — points authority moved off
    /// `team_members.points`, which nothing writes to anymore), not
    /// `team_members.points` itself. `role` stays `team_members.role` — team
    /// management (invite/rename/set-role) is a separate authority C1 deliberately
    /// left alone, see that stage's own implementation notes. `COALESCE(...,  0)`
    /// covers a team with no backing project yet, or a member row not yet synced
    /// into `project_members` (e.g. a still-`PENDING` invitee) — same "no balance
    /// yet" default `team_members.points`'s own `NOT NULL DEFAULT 0` used to give
    /// for free.
    async fn list_members(&self, team_id: &str) -> Result<Vec<TeamMemberInfo>, RepoError> {
        sqlx::query(
            "SELECT users.id, users.first_name, users.last_name, users.email, users.google_id,
                    team_members.status, team_members.role,
                    COALESCE(project_members.points, 0) AS points
             FROM team_members
             JOIN users ON team_members.user_id = users.id
             LEFT JOIN projects ON projects.team_id = team_members.team_id
             LEFT JOIN project_members
                 ON project_members.project_id = projects.id
                AND project_members.user_id = team_members.user_id
             WHERE team_members.team_id = ?
             ORDER BY users.first_name ASC",
        )
        .bind(team_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let role_str: String = row.get("role");
                    TeamMemberInfo {
                        user: row_to_user(row),
                        status: row.get("status"),
                        role: TeamRole::from_str(&role_str).unwrap_or_default(),
                        points: row.get("points"),
                    }
                })
                .collect()
        })
    }

    async fn member_status(
        &self,
        team_id: &str,
        user_id: &str,
    ) -> Result<Option<String>, RepoError> {
        sqlx::query("SELECT status FROM team_members WHERE team_id = ? AND user_id = ?")
            .bind(team_id)
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)
            .map(|row| row.map(|r| r.get("status")))
    }

    async fn member_role(
        &self,
        team_id: &str,
        user_id: &str,
    ) -> Result<Option<TeamRole>, RepoError> {
        sqlx::query("SELECT role FROM team_members WHERE team_id = ? AND user_id = ?")
            .bind(team_id)
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)
            .map(|row| {
                row.map(|r| {
                    let role_str: String = r.get("role");
                    TeamRole::from_str(&role_str).unwrap_or_default()
                })
            })
    }

    async fn count_active_admins(&self, team_id: &str) -> Result<i64, RepoError> {
        sqlx::query(
            "SELECT COUNT(*) AS n FROM team_members
             WHERE team_id = ? AND status = 'ACTIVE' AND role = 'admin'",
        )
        .bind(team_id)
        .fetch_one(&self.0)
        .await
        .map_err(db_err)
        .map(|row| row.get("n"))
    }

    async fn set_member_role(
        &self,
        team_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<(), RepoError> {
        let rows = sqlx::query(
            "UPDATE team_members SET role = ? WHERE team_id = ? AND user_id = ?",
        )
        .bind(role.as_str())
        .bind(team_id)
        .bind(user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn add_team_points(
        &self,
        team_id: &str,
        user_id: &str,
        delta: i32,
    ) -> Result<i64, RepoError> {
        sqlx::query(
            "UPDATE team_members SET points = points + ? WHERE team_id = ? AND user_id = ? \
             RETURNING points",
        )
        .bind(delta)
        .bind(team_id)
        .bind(user_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        .map(|row| row.get("points"))
        .ok_or_else(not_found)
    }

    async fn invite(
        &self,
        team_id: &str,
        invitee_user_id: &str,
        invited_by: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, invited_by) VALUES (?, ?, 'PENDING', ?)",
        )
        .bind(team_id)
        .bind(invitee_user_id)
        .bind(invited_by)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn accept(&self, team_id: &str, user_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query(
            "UPDATE team_members SET status = 'ACTIVE' WHERE team_id = ? AND user_id = ? AND status = 'PENDING'",
        )
        .bind(team_id)
        .bind(user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 {
            return Err(not_found());
        }
        // Cascades the new ACTIVE membership to every project this team backs —
        // see docs/project-abstraction-plan.md stage A4. NOT EXISTS skips a
        // project where this user is already a member (e.g. they're also that
        // project's owner) so an existing role is never clobbered.
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role, points)
             SELECT projects.id, ?, 'member', 0
             FROM projects
             WHERE projects.team_id = ?
               AND NOT EXISTS (
                   SELECT 1 FROM project_members
                   WHERE project_members.project_id = projects.id
                     AND project_members.user_id = ?
               )",
        )
        .bind(user_id)
        .bind(team_id)
        .bind(user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn remove_member(&self, team_id: &str, user_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("DELETE FROM team_members WHERE team_id = ? AND user_id = ?")
            .bind(team_id)
            .bind(user_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 {
            return Err(not_found());
        }
        // Cascades the departure to every project this team backs — the mirror
        // of the `accept` cascade above.
        sqlx::query(
            "DELETE FROM project_members
             WHERE user_id = ? AND project_id IN (SELECT id FROM projects WHERE team_id = ?)",
        )
        .bind(user_id)
        .bind(team_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "DELETE FROM teams WHERE id = ? AND NOT EXISTS (SELECT 1 FROM team_members WHERE team_id = ?)",
        )
        .bind(team_id)
        .bind(team_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn share_active_team(&self, user_a: &str, user_b: &str) -> Result<bool, RepoError> {
        let row = sqlx::query(
            "SELECT 1 FROM team_members a
             JOIN team_members b ON a.team_id = b.team_id
             WHERE a.user_id = ? AND a.status = 'ACTIVE' AND b.user_id = ? AND b.status = 'ACTIVE'
             LIMIT 1",
        )
        .bind(user_a)
        .bind(user_b)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?;
        Ok(row.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    // Mirrors `sqlite::projects`'s own `test_pool()` pattern (teams.rs had no
    // sqlite-level tests before stage A4 — see the plan's A2 implementation
    // notes). Includes `projects`/`project_members` alongside `teams`/
    // `team_members`/`users` since this stage's whole point is the cascade
    // between the two pairs of tables — see docs/project-abstraction-plan.md
    // stage A4.
    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .shared_cache(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE users (
                id TEXT PRIMARY KEY,
                first_name TEXT NOT NULL,
                last_name TEXT NOT NULL,
                email TEXT,
                google_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE teams (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL
            )",
        )
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

    async fn insert_user(pool: &SqlitePool, id: &str) {
        sqlx::query("INSERT INTO users (id, first_name, last_name) VALUES (?, ?, '')")
            .bind(id)
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn insert_project(pool: &SqlitePool, id: &str, owner_user_id: &str, team_id: &str) {
        sqlx::query("INSERT INTO projects (id, name, owner_user_id, team_id) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(id)
            .bind(owner_user_id)
            .bind(team_id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn project_member_role(pool: &SqlitePool, project_id: &str, user_id: &str) -> Option<String> {
        sqlx::query("SELECT role FROM project_members WHERE project_id = ? AND user_id = ?")
            .bind(project_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .unwrap()
            .map(|row| row.get::<String, _>("role"))
    }

    #[tokio::test]
    async fn accept_adds_project_member_row_to_every_project_the_team_backs() {
        let pool = test_pool().await;
        let repo = SqliteTeamRepo(pool.clone());
        insert_user(&pool, "owner1").await;
        insert_user(&pool, "member1").await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('team1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role) VALUES ('team1', 'owner1', 'ACTIVE', 'admin')",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_project(&pool, "p1", "owner1", "team1").await;
        insert_project(&pool, "p2", "owner1", "team1").await;
        repo.invite("team1", "member1", "owner1").await.unwrap();

        repo.accept("team1", "member1").await.unwrap();

        assert_eq!(
            project_member_role(&pool, "p1", "member1").await,
            Some("member".to_string())
        );
        assert_eq!(
            project_member_role(&pool, "p2", "member1").await,
            Some("member".to_string())
        );
    }

    #[tokio::test]
    async fn accept_only_seeds_from_active_members_not_pending() {
        let pool = test_pool().await;
        let repo = SqliteTeamRepo(pool.clone());
        insert_user(&pool, "owner1").await;
        insert_user(&pool, "member1").await;
        insert_user(&pool, "member2").await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('team1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        insert_project(&pool, "p1", "owner1", "team1").await;
        repo.invite("team1", "member1", "owner1").await.unwrap();
        repo.invite("team1", "member2", "owner1").await.unwrap();

        repo.accept("team1", "member1").await.unwrap();

        assert!(project_member_role(&pool, "p1", "member1").await.is_some());
        // member2 is still PENDING — never accepted — so no row for them yet.
        assert!(project_member_role(&pool, "p1", "member2").await.is_none());
    }

    #[tokio::test]
    async fn accept_does_not_clobber_an_existing_project_members_row() {
        let pool = test_pool().await;
        let repo = SqliteTeamRepo(pool.clone());
        insert_user(&pool, "owner1").await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('team1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        insert_project(&pool, "p1", "owner1", "team1").await;
        // owner1 already has an admin row on p1 (as if seeded at project
        // creation) — accepting an invite for the same team must not downgrade it.
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role, points) VALUES ('p1', 'owner1', 'admin', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        repo.invite("team1", "owner1", "owner1").await.unwrap();

        repo.accept("team1", "owner1").await.unwrap();

        assert_eq!(
            project_member_role(&pool, "p1", "owner1").await,
            Some("admin".to_string())
        );
    }

    #[tokio::test]
    async fn remove_member_removes_project_member_row_from_every_backed_project() {
        let pool = test_pool().await;
        let repo = SqliteTeamRepo(pool.clone());
        insert_user(&pool, "owner1").await;
        insert_user(&pool, "member1").await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('team1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role) VALUES ('team1', 'owner1', 'ACTIVE', 'admin')",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_project(&pool, "p1", "owner1", "team1").await;
        insert_project(&pool, "p2", "owner1", "team1").await;
        repo.invite("team1", "member1", "owner1").await.unwrap();
        repo.accept("team1", "member1").await.unwrap();
        assert!(project_member_role(&pool, "p1", "member1").await.is_some());
        assert!(project_member_role(&pool, "p2", "member1").await.is_some());

        repo.remove_member("team1", "member1").await.unwrap();

        assert!(project_member_role(&pool, "p1", "member1").await.is_none());
        assert!(project_member_role(&pool, "p2", "member1").await.is_none());
    }

    /// Stage C1 (docs/project-abstraction-plan.md): points authority moved off
    /// `team_members.points` onto the backing project's `project_members.points` —
    /// `list_members` (and everything built on it: `member_points`, the legacy
    /// `ListTeamMembers` JSON API operation, `prl teams members`, `teams.rs`'s own
    /// member listing) must read the live, project-sourced balance, not the
    /// frozen `team_members` one nothing writes to anymore.
    #[tokio::test]
    async fn list_members_sources_points_from_the_teams_backing_project() {
        let pool = test_pool().await;
        let repo = SqliteTeamRepo(pool.clone());
        insert_user(&pool, "member1").await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('team1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role, points) \
             VALUES ('team1', 'member1', 'ACTIVE', 'member', 999)",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_project(&pool, "p1", "member1", "team1").await;
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role, points) \
             VALUES ('p1', 'member1', 'member', 42)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let members = repo.list_members("team1").await.unwrap();
        assert_eq!(members.len(), 1);
        // The stale team_members.points value (999) must never surface here.
        assert_eq!(members[0].points, 42);
    }

    #[tokio::test]
    async fn list_members_defaults_points_to_zero_with_no_backing_project() {
        let pool = test_pool().await;
        let repo = SqliteTeamRepo(pool.clone());
        insert_user(&pool, "member1").await;
        sqlx::query("INSERT INTO teams (id, name) VALUES ('team1', 'Family')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, status, role, points) \
             VALUES ('team1', 'member1', 'ACTIVE', 'member', 999)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // No `projects` row backs this team at all.

        let members = repo.list_members("team1").await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].points, 0);
    }
}
