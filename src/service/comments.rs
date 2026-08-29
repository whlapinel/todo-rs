use crate::domain::comment::Comment;
use crate::domain::item::ItemKind;
use crate::service::error::ItemError;
use crate::service::projects::require_project_member;
use crate::storage::sqlite::{CommentRepo, ItemRepo, ProjectRepo, TeamRepo};
use chrono::Utc;
use std::sync::Arc;

/// Task-only, any-project-member (`docs/issues_and_features.md`'s "Add comments for
/// tasks" — no admin gate, unlike `points`/`assignedToUserId`). Rejects a missing item
/// the same way `get_by_project` naturally does for a virtual series occurrence, which
/// never exists as a row.
pub async fn create_comment(
    comments: &Arc<dyn CommentRepo>,
    items: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    item_id: &str,
    requester_user_id: &str,
    body: &str,
) -> Result<Comment, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let item = items.get_by_project(project_id, item_id).await?;
    if item.kind() != ItemKind::Task {
        return Err(ItemError::Invalid(
            "comments are only supported on tasks".to_string(),
        ));
    }
    let body = body.trim();
    if body.is_empty() {
        return Err(ItemError::Invalid(
            "comment body cannot be empty".to_string(),
        ));
    }

    let comment = Comment {
        id: uuid::Uuid::new_v4().to_string(),
        item_id: item_id.to_string(),
        project_id: project_id.to_string(),
        author_user_id: requester_user_id.to_string(),
        body: body.to_string(),
        created_at: Utc::now(),
    };
    comments.create(&comment).await?;
    Ok(comment)
}

pub async fn list_comments_for_item(
    comments: &Arc<dyn CommentRepo>,
    items: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    item_id: &str,
    requester_user_id: &str,
) -> Result<Vec<Comment>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let item = items.get_by_project(project_id, item_id).await?;
    if item.kind() != ItemKind::Task {
        return Err(ItemError::Invalid(
            "comments are only supported on tasks".to_string(),
        ));
    }
    Ok(comments.list_for_item(item_id).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{Item, ItemType};
    use crate::domain::project::Project;
    use crate::storage::sqlite::{MockCommentRepo, MockItemRepo, MockProjectRepo, MockTeamRepo};

    fn personal_project(id: &str, owner: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "p".to_string(),
            owner_user_id: owner.to_string(),
            team_id: None,
        }
    }

    fn task(id: &str, project_id: &str) -> Item {
        let mut item = Item::new_user_item("owner1", "task");
        item.id = id.to_string();
        item.project_id = Some(project_id.to_string());
        item
    }

    #[tokio::test]
    async fn create_comment_rejects_non_task_items() {
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get_by_project().returning(|_, id| {
            let mut item = task(id, "p1");
            item.item_type = ItemType::Simple;
            Ok(item)
        });
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|id| Ok(personal_project(id, "owner1")));
        let comments = Arc::new(MockCommentRepo::new()) as Arc<dyn CommentRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let projects = Arc::new(projects_mock) as Arc<dyn ProjectRepo>;
        let teams = Arc::new(MockTeamRepo::new()) as Arc<dyn TeamRepo>;

        let err = create_comment(
            &comments, &items, &projects, &teams, "p1", "i1", "owner1", "hello",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn create_comment_rejects_empty_body() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, id| Ok(task(id, "p1")));
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|id| Ok(personal_project(id, "owner1")));
        let comments = Arc::new(MockCommentRepo::new()) as Arc<dyn CommentRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let projects = Arc::new(projects_mock) as Arc<dyn ProjectRepo>;
        let teams = Arc::new(MockTeamRepo::new()) as Arc<dyn TeamRepo>;

        let err = create_comment(
            &comments, &items, &projects, &teams, "p1", "i1", "owner1", "   ",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn create_comment_rejects_non_members() {
        let items_mock = MockItemRepo::new();
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|id| Ok(personal_project(id, "owner1")));
        let comments = Arc::new(MockCommentRepo::new()) as Arc<dyn CommentRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let projects = Arc::new(projects_mock) as Arc<dyn ProjectRepo>;
        let teams = Arc::new(MockTeamRepo::new()) as Arc<dyn TeamRepo>;

        let err = create_comment(
            &comments,
            &items,
            &projects,
            &teams,
            "p1",
            "i1",
            "someone_else",
            "hello",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn create_comment_persists_a_trimmed_comment_for_a_valid_task() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, id| Ok(task(id, "p1")));
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|id| Ok(personal_project(id, "owner1")));
        let mut comments_mock = MockCommentRepo::new();
        comments_mock
            .expect_create()
            .withf(|c| c.item_id == "i1" && c.project_id == "p1" && c.body == "hello")
            .returning(|_| Ok(()));
        let comments = Arc::new(comments_mock) as Arc<dyn CommentRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let projects = Arc::new(projects_mock) as Arc<dyn ProjectRepo>;
        let teams = Arc::new(MockTeamRepo::new()) as Arc<dyn TeamRepo>;

        let comment = create_comment(
            &comments,
            &items,
            &projects,
            &teams,
            "p1",
            "i1",
            "owner1",
            "  hello  ",
        )
        .await
        .unwrap();
        assert_eq!(comment.body, "hello");
        assert_eq!(comment.author_user_id, "owner1");
    }

    #[tokio::test]
    async fn list_comments_for_item_returns_stored_comments() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, id| Ok(task(id, "p1")));
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|id| Ok(personal_project(id, "owner1")));
        let mut comments_mock = MockCommentRepo::new();
        comments_mock.expect_list_for_item().returning(|item_id| {
            Ok(vec![Comment {
                id: "c1".to_string(),
                item_id: item_id.to_string(),
                project_id: "p1".to_string(),
                author_user_id: "owner1".to_string(),
                body: "hi".to_string(),
                created_at: Utc::now(),
            }])
        });
        let comments = Arc::new(comments_mock) as Arc<dyn CommentRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let projects = Arc::new(projects_mock) as Arc<dyn ProjectRepo>;
        let teams = Arc::new(MockTeamRepo::new()) as Arc<dyn TeamRepo>;

        let result =
            list_comments_for_item(&comments, &items, &projects, &teams, "p1", "i1", "owner1")
                .await
                .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "c1");
    }
}
