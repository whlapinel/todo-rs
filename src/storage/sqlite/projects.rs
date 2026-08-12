use async_trait::async_trait;
use std::str::FromStr;

use crate::storage::sqlite::{
    ProjectMemberInfo, ProjectRepo, RepoError, db_err, not_found, row_to_user,
};
use sqlx::{Row, SqlitePool};

use crate::domain::project::Project;
use crate::domain::team::TeamRole;

pub struct SqliteProjectRepo(pub SqlitePool);

#[async_trait]
impl ProjectRepo for SqliteProjectRepo {
    async fn create<'a>(
        &'a self,
        name: &'a str,
        owner_user_id: &'a str,
        team_id: Option<&'a str>,
    ) -> Result<String, RepoError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO projects (id, name, owner_user_id, team_id) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(name)
            .bind(owner_user_id)
            .bind(team_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role, points) VALUES (?, ?, 'admin', 0)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    async fn get(&self, project_id: &str) -> Result<Project, RepoError> {
        sqlx::query("SELECT id, name, owner_user_id, team_id FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&self.0)
            .await
            .map_err(db_err)?
            .map(|row| Project {
                id: row.get("id"),
                name: row.get("name"),
                owner_user_id: row.get("owner_user_id"),
                team_id: row.get("team_id"),
            })
            .ok_or_else(not_found)
    }

    async fn update_name(&self, project_id: &str, name: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("UPDATE projects SET name = ? WHERE id = ?")
            .bind(name)
            .bind(project_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn attach_team(&self, project_id: &str, team_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("UPDATE projects SET team_id = ? WHERE id = ?")
            .bind(team_id)
            .bind(project_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn detach_team(&self, project_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("UPDATE projects SET team_id = NULL WHERE id = ?")
            .bind(project_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn delete(&self, project_id: &str) -> Result<(), RepoError> {
        let rows = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(project_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows == 0 {
            return Err(not_found());
        }
        sqlx::query("DELETE FROM project_members WHERE project_id = ?")
            .bind(project_id)
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_for_user(&self, user_id: &str) -> Result<Vec<Project>, RepoError> {
        sqlx::query(
            "SELECT projects.id, projects.name, projects.owner_user_id, projects.team_id
             FROM project_members
             JOIN projects ON project_members.project_id = projects.id
             WHERE project_members.user_id = ?
             ORDER BY projects.name ASC",
        )
        .bind(user_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| {
            rows.iter()
                .map(|row| Project {
                    id: row.get("id"),
                    name: row.get("name"),
                    owner_user_id: row.get("owner_user_id"),
                    team_id: row.get("team_id"),
                })
                .collect()
        })
    }

    async fn list_members(&self, project_id: &str) -> Result<Vec<ProjectMemberInfo>, RepoError> {
        sqlx::query(
            "SELECT users.id, users.first_name, users.last_name, users.email, users.google_id,
                    project_members.role, project_members.points
             FROM project_members
             JOIN users ON project_members.user_id = users.id
             WHERE project_members.project_id = ?
             ORDER BY users.first_name ASC",
        )
        .bind(project_id)
        .fetch_all(&self.0)
        .await
        .map_err(db_err)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let role_str: String = row.get("role");
                    ProjectMemberInfo {
                        user: row_to_user(row),
                        role: TeamRole::from_str(&role_str).unwrap_or_default(),
                        points: row.get("points"),
                    }
                })
                .collect()
        })
    }

    async fn member_role(
        &self,
        project_id: &str,
        user_id: &str,
    ) -> Result<Option<TeamRole>, RepoError> {
        sqlx::query("SELECT role FROM project_members WHERE project_id = ? AND user_id = ?")
            .bind(project_id)
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

    async fn set_member_role(
        &self,
        project_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<(), RepoError> {
        let rows = sqlx::query(
            "UPDATE project_members SET role = ? WHERE project_id = ? AND user_id = ?",
        )
        .bind(role.as_str())
        .bind(project_id)
        .bind(user_id)
        .execute(&self.0)
        .await
        .map_err(db_err)?
        .rows_affected();
        if rows == 0 { Err(not_found()) } else { Ok(()) }
    }

    async fn add_project_points(
        &self,
        project_id: &str,
        user_id: &str,
        delta: i32,
    ) -> Result<i64, RepoError> {
        sqlx::query(
            "UPDATE project_members SET points = points + ? WHERE project_id = ? AND user_id = ? \
             RETURNING points",
        )
        .bind(delta)
        .bind(project_id)
        .bind(user_id)
        .fetch_optional(&self.0)
        .await
        .map_err(db_err)?
        .map(|row| row.get("points"))
        .ok_or_else(not_found)
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

    async fn insert_user(pool: &SqlitePool, id: &str, first_name: &str) {
        sqlx::query("INSERT INTO users (id, first_name, last_name) VALUES (?, ?, '')")
            .bind(id)
            .bind(first_name)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_seeds_owner_as_admin_member() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;

        let id = repo.create("Home", "u1", None).await.unwrap();

        let project = repo.get(&id).await.unwrap();
        assert_eq!(project.name, "Home");
        assert_eq!(project.owner_user_id, "u1");
        assert_eq!(project.team_id, None);

        let role = repo.member_role(&id, "u1").await.unwrap();
        assert_eq!(role, Some(TeamRole::Admin));
    }

    #[tokio::test]
    async fn create_with_team_id_round_trips() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;

        let id = repo.create("Family", "u1", Some("t1")).await.unwrap();

        let project = repo.get(&id).await.unwrap();
        assert_eq!(project.team_id, Some("t1".to_string()));
    }

    #[tokio::test]
    async fn get_missing_project_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        let err = repo.get("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn update_name_changes_row() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        repo.update_name(&id, "Home Sweet Home").await.unwrap();

        assert_eq!(repo.get(&id).await.unwrap().name, "Home Sweet Home");
    }

    #[tokio::test]
    async fn update_name_missing_project_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        let err = repo.update_name("missing", "X").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn attach_and_detach_team_round_trip() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        repo.attach_team(&id, "t1").await.unwrap();
        assert_eq!(repo.get(&id).await.unwrap().team_id, Some("t1".to_string()));

        repo.detach_team(&id).await.unwrap();
        assert_eq!(repo.get(&id).await.unwrap().team_id, None);
    }

    #[tokio::test]
    async fn delete_removes_project_and_members() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        repo.delete(&id).await.unwrap();

        assert!(matches!(
            repo.get(&id).await.unwrap_err(),
            RepoError::NotFound
        ));
        let members: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM project_members WHERE project_id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(members.0, 0);
    }

    #[tokio::test]
    async fn delete_missing_project_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        let err = repo.delete("missing").await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn list_for_user_orders_by_name_and_scopes_to_member() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        insert_user(&pool, "u2", "Bea").await;
        repo.create("Zebra", "u1", None).await.unwrap();
        repo.create("Apple", "u1", None).await.unwrap();
        repo.create("Other", "u2", None).await.unwrap();

        let projects = repo.list_for_user("u1").await.unwrap();
        let names: Vec<_> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "Zebra"]);
    }

    #[tokio::test]
    async fn list_members_returns_role_and_points() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        let members = repo.list_members(&id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user.id, "u1");
        assert_eq!(members[0].role, TeamRole::Admin);
        assert_eq!(members[0].points, 0);
    }

    #[tokio::test]
    async fn member_role_returns_none_for_non_member() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        assert_eq!(repo.member_role(&id, "u2").await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_member_role_updates_role() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        repo.set_member_role(&id, "u1", TeamRole::Member)
            .await
            .unwrap();

        assert_eq!(
            repo.member_role(&id, "u1").await.unwrap(),
            Some(TeamRole::Member)
        );
    }

    #[tokio::test]
    async fn set_member_role_missing_member_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        let err = repo
            .set_member_role(&id, "u2", TeamRole::Admin)
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }

    #[tokio::test]
    async fn add_project_points_accumulates_and_returns_balance() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        let balance = repo.add_project_points(&id, "u1", 5).await.unwrap();
        assert_eq!(balance, 5);
        let balance = repo.add_project_points(&id, "u1", -2).await.unwrap();
        assert_eq!(balance, 3);
    }

    #[tokio::test]
    async fn add_project_points_missing_member_returns_not_found() {
        let pool = test_pool().await;
        let repo = SqliteProjectRepo(pool.clone());
        insert_user(&pool, "u1", "Ann").await;
        let id = repo.create("Home", "u1", None).await.unwrap();

        let err = repo.add_project_points(&id, "u2", 5).await.unwrap_err();
        assert!(matches!(err, RepoError::NotFound));
    }
}
