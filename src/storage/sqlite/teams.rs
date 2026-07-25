use async_trait::async_trait;

use crate::storage::sqlite::{TeamRepo, TeamMemberInfo, TeamWithStatus, RepoError, db_err, not_found, row_to_user};
use sqlx::{Row, SqlitePool};

use crate::domain::{team::Team};

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
            "INSERT INTO team_members (team_id, user_id, status, invited_by) VALUES (?, ?, 'ACTIVE', NULL)",
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

    async fn list_members(&self, team_id: &str) -> Result<Vec<TeamMemberInfo>, RepoError> {
        sqlx::query(
            "SELECT users.id, users.first_name, users.last_name, users.email, users.google_id,
                    team_members.status
             FROM team_members
             JOIN users ON team_members.user_id = users.id
             WHERE team_members.team_id = ?
             ORDER BY users.first_name ASC",
        )
        .bind(team_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| {
            rows.iter()
                .map(|row| TeamMemberInfo {
                    user: row_to_user(row),
                    status: row.get("status"),
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
        if rows == 0 { Err(not_found()) } else { Ok(()) }
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
