use crate::domain::comment::Comment;
use crate::domain::item::ItemKind;
use crate::service::attachments as attachments_service;
use crate::service::error::ItemError;
use crate::service::projects::require_project_member;
use crate::service::push;
use crate::storage::attachment_store::AttachmentStore;
use crate::storage::sqlite::{AttachmentRepo, CommentRepo, ItemRepo, ProjectRepo, TeamRepo};
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

    push::notify_comment(
        Arc::clone(projects),
        project_id.to_string(),
        item.name.clone(),
        push::detail_url(&item, project_id),
        requester_user_id.to_string(),
        comment.body.clone(),
    );

    Ok(comment)
}

/// The web UI's actual comment-creation path — see root CLAUDE.md's Attachments
/// section: a file, once uploaded, always belongs to a comment (never to the item
/// directly), so this is how the two get created together. Unlike `create_comment`
/// above, `body` may be empty *if* `file` is `Some` — the attachment is the content in
/// that case, and `body` becomes an optional caption. `create_comment` itself is
/// unchanged and still what `json_api::comments::create_item_comment` (the CLI/MCP
/// path, no file-upload concept) calls, so that path keeps requiring non-empty text.
#[allow(clippy::too_many_arguments)]
pub async fn create_comment_with_attachment(
    comments: &Arc<dyn CommentRepo>,
    attachments: &Arc<dyn AttachmentRepo>,
    attachment_store: &Arc<dyn AttachmentStore>,
    items: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    item_id: &str,
    requester_user_id: &str,
    body: &str,
    file: Option<(String, String, Vec<u8>)>,
) -> Result<Comment, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let item = items.get_by_project(project_id, item_id).await?;
    if item.kind() != ItemKind::Task {
        return Err(ItemError::Invalid(
            "comments are only supported on tasks".to_string(),
        ));
    }
    let body = body.trim();
    if body.is_empty() && file.is_none() {
        return Err(ItemError::Invalid(
            "comment must have text or an attachment".to_string(),
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

    let has_attachment = file.is_some();
    if let Some((filename, content_type, bytes)) = file {
        attachments_service::upload_attachment(
            attachments,
            attachment_store,
            items,
            projects,
            teams,
            project_id,
            item_id,
            requester_user_id,
            &filename,
            &content_type,
            bytes,
            &comment.id,
        )
        .await?;
    }

    let notify_body = if comment.body.is_empty() && has_attachment {
        "📎 sent an attachment".to_string()
    } else {
        comment.body.clone()
    };
    push::notify_comment(
        Arc::clone(projects),
        project_id.to_string(),
        item.name.clone(),
        push::detail_url(&item, project_id),
        requester_user_id.to_string(),
        notify_body,
    );

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
    use crate::storage::attachment_store::MockAttachmentStore;
    use crate::storage::sqlite::{
        MockAttachmentRepo, MockCommentRepo, MockItemRepo, MockProjectRepo, MockTeamRepo,
    };

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

    #[tokio::test]
    async fn create_comment_with_attachment_rejects_empty_body_and_no_file() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, id| Ok(task(id, "p1")));
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|id| Ok(personal_project(id, "owner1")));
        let comments = Arc::new(MockCommentRepo::new()) as Arc<dyn CommentRepo>;
        let attachments = Arc::new(MockAttachmentRepo::new()) as Arc<dyn AttachmentRepo>;
        let attachment_store = Arc::new(MockAttachmentStore::new()) as Arc<dyn AttachmentStore>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let projects = Arc::new(projects_mock) as Arc<dyn ProjectRepo>;
        let teams = Arc::new(MockTeamRepo::new()) as Arc<dyn TeamRepo>;

        let err = create_comment_with_attachment(
            &comments,
            &attachments,
            &attachment_store,
            &items,
            &projects,
            &teams,
            "p1",
            "i1",
            "owner1",
            "   ",
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn create_comment_with_attachment_allows_empty_body_when_file_present() {
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
            .withf(|c| c.body.is_empty())
            .returning(|_| Ok(()));
        let mut attachments_mock = MockAttachmentRepo::new();
        attachments_mock
            .expect_create()
            .withf(|a| {
                a.item_id == "i1"
                    && a.project_id == "p1"
                    && a.filename == "photo.jpg"
                    && !a.comment_id.is_empty()
            })
            .returning(|_| Ok(()));
        let mut store_mock = MockAttachmentStore::new();
        store_mock.expect_put().returning(|_, _| Ok(()));
        let comments = Arc::new(comments_mock) as Arc<dyn CommentRepo>;
        let attachments = Arc::new(attachments_mock) as Arc<dyn AttachmentRepo>;
        let attachment_store = Arc::new(store_mock) as Arc<dyn AttachmentStore>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let projects = Arc::new(projects_mock) as Arc<dyn ProjectRepo>;
        let teams = Arc::new(MockTeamRepo::new()) as Arc<dyn TeamRepo>;

        let comment = create_comment_with_attachment(
            &comments,
            &attachments,
            &attachment_store,
            &items,
            &projects,
            &teams,
            "p1",
            "i1",
            "owner1",
            "   ",
            Some((
                "photo.jpg".to_string(),
                "image/jpeg".to_string(),
                vec![1, 2, 3],
            )),
        )
        .await
        .unwrap();
        assert_eq!(comment.body, "");
    }
}
