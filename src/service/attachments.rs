use crate::domain::attachment::Attachment;
use crate::domain::item::ItemKind;
use crate::service::error::ItemError;
use crate::service::projects::require_project_member;
use crate::storage::attachment_store::AttachmentStore;
use crate::storage::sqlite::{AttachmentRepo, ItemRepo, ProjectRepo, TeamRepo};
use chrono::Utc;
use std::sync::Arc;

/// Same restriction shape as `service::comments` — Task-only, any project member. See
/// root CLAUDE.md's Attachments section. Also enforced as the axum `DefaultBodyLimit` on
/// the web UI's upload route (`main.rs`), which must stay >= this value plus multipart
/// overhead or a large-but-valid upload gets rejected before ever reaching this check.
pub const MAX_ATTACHMENT_SIZE_BYTES: usize = 25 * 1024 * 1024;

/// Validates, writes the bytes via `store`, then records the metadata row. Not
/// transactional across the two writes — same precedent as `create_item`'s template-
/// trigger copies (root CLAUDE.md's Events section): if the metadata insert fails after
/// `store.put` already succeeded, the blob is orphaned (a cheap leak, cleaned up by
/// nothing today) rather than the alternative of a metadata row pointing at bytes that
/// were never written.
#[allow(clippy::too_many_arguments)]
pub async fn upload_attachment(
    attachments: &Arc<dyn AttachmentRepo>,
    store: &Arc<dyn AttachmentStore>,
    items: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    item_id: &str,
    requester_user_id: &str,
    filename: &str,
    content_type: &str,
    bytes: Vec<u8>,
    comment_id: &str,
) -> Result<Attachment, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let item = items.get_by_project(project_id, item_id).await?;
    if item.kind() != ItemKind::Task {
        return Err(ItemError::Invalid(
            "attachments are only supported on tasks".to_string(),
        ));
    }
    if bytes.is_empty() {
        return Err(ItemError::Invalid("attachment file is empty".to_string()));
    }
    if bytes.len() > MAX_ATTACHMENT_SIZE_BYTES {
        return Err(ItemError::Invalid(format!(
            "attachment exceeds the {}MB limit",
            MAX_ATTACHMENT_SIZE_BYTES / (1024 * 1024)
        )));
    }
    let filename = filename.trim();
    if filename.is_empty() {
        return Err(ItemError::Invalid(
            "attachment filename is required".to_string(),
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let storage_key = format!(
        "{project_id}/{item_id}/{id}_{}",
        sanitize_filename(filename)
    );
    let size_bytes = bytes.len() as i64;

    store
        .put(&storage_key, bytes)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;

    let attachment = Attachment {
        id,
        comment_id: comment_id.to_string(),
        item_id: item_id.to_string(),
        project_id: project_id.to_string(),
        uploaded_by_user_id: requester_user_id.to_string(),
        filename: filename.to_string(),
        content_type: content_type.to_string(),
        size_bytes,
        storage_key,
        created_at: Utc::now(),
    };
    attachments.create(&attachment).await?;
    Ok(attachment)
}

/// Returns the attachment's own metadata alongside its bytes (the caller needs the
/// former for the download response's `Content-Type`/filename). `attachment_id` is
/// checked against `project_id`/`item_id` from the URL rather than trusted alone, so a
/// leaked or guessed attachment id belonging to a different item can't be used to read
/// this one's bytes out from under an unrelated project.
pub async fn download_attachment(
    attachments: &Arc<dyn AttachmentRepo>,
    store: &Arc<dyn AttachmentStore>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    item_id: &str,
    attachment_id: &str,
    requester_user_id: &str,
) -> Result<(Attachment, Vec<u8>), ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let attachment = attachments.get(attachment_id).await?;
    if attachment.project_id != project_id || attachment.item_id != item_id {
        return Err(ItemError::NotFound);
    }
    let bytes = store
        .get(&attachment.storage_key)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
    Ok((attachment, bytes))
}

/// Any project member may delete any attachment on a task in that project — same
/// ungated-among-members shape as `service::comments::create_comment`, not restricted to
/// the uploader or a project admin.
pub async fn delete_attachment(
    attachments: &Arc<dyn AttachmentRepo>,
    store: &Arc<dyn AttachmentStore>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    item_id: &str,
    attachment_id: &str,
    requester_user_id: &str,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let attachment = attachments.get(attachment_id).await?;
    if attachment.project_id != project_id || attachment.item_id != item_id {
        return Err(ItemError::NotFound);
    }
    attachments.delete(attachment_id).await?;
    // Best-effort: the metadata row is already gone by this point, so a failure here
    // just leaks an orphaned blob rather than leaving a row that points at nothing —
    // same tradeoff `upload_attachment`'s doc comment describes for the reverse order.
    let _ = store.delete(&attachment.storage_key).await;
    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
