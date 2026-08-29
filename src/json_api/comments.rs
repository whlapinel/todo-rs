use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::domain::comment::Comment;
use crate::service::comments as comments_service;
use crate::service::items::ItemError;
use crate::storage::sqlite::{CommentRepo, ItemRepo, ProjectRepo, TeamRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, model, output, server, types::DateTime as SmithyDateTime};

fn to_msg(e: ItemError) -> error::PeoplesRepublicOfListsError {
    match e {
        ItemError::NotFound => not_found(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg),
    }
}

fn to_summary(comment: Comment) -> model::CommentSummary {
    model::CommentSummary {
        comment_id: comment.id,
        item_id: comment.item_id,
        project_id: comment.project_id,
        author_user_id: comment.author_user_id,
        body: comment.body,
        created_at: SmithyDateTime::from_secs(comment.created_at.timestamp()),
    }
}

pub async fn create_item_comment(
    input: input::CreateItemCommentInput,
    server::Extension(comments): server::Extension<Arc<dyn CommentRepo>>,
    server::Extension(items): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateItemCommentOutput, error::CreateItemCommentError> {
    let comment = comments_service::create_comment(
        &comments,
        &items,
        &projects,
        &teams,
        &input.project_id,
        &input.item_id,
        &auth.user_id,
        &input.body,
    )
    .await
    .map_err(|e| error::CreateItemCommentError::from(to_msg(e)))?;
    Ok(output::CreateItemCommentOutput {
        comment_id: comment.id,
    })
}

pub async fn list_item_comments(
    input: input::ListItemCommentsInput,
    server::Extension(comments): server::Extension<Arc<dyn CommentRepo>>,
    server::Extension(items): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListItemCommentsOutput, error::ListItemCommentsError> {
    let comment_list = comments_service::list_comments_for_item(
        &comments,
        &items,
        &projects,
        &teams,
        &input.project_id,
        &input.item_id,
        &auth.user_id,
    )
    .await
    .map_err(|e| error::ListItemCommentsError::from(to_msg(e)))?;
    Ok(output::ListItemCommentsOutput {
        comments: comment_list.into_iter().map(to_summary).collect(),
    })
}
