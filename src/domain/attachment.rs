use chrono::{DateTime, Utc};

/// A file/photo attached to a comment on a Task item — attachments always belong to a
/// comment (`comment_id`), which is what lets a file be annotated with the comment's own
/// text; there is no way to attach a file to an item directly, unannotated. `item_id`/
/// `project_id` are carried alongside `comment_id` as a deliberate denormalization
/// (`Comment` itself does the same relative to `Item`) so a lookup/ownership check never
/// needs to join through `comments` to scope an attachment to its item/project. Metadata
/// lives in the `attachments` table; the bytes live wherever
/// `storage::attachment_store::AttachmentStore` puts them, addressed by `storage_key` —
/// an opaque string only the store implementation interprets (a relative path under
/// `LocalFsAttachmentStore`'s root, an object key for a future S3-backed store). See
/// root CLAUDE.md's Attachments section.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub comment_id: String,
    pub item_id: String,
    pub project_id: String,
    pub uploaded_by_user_id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub storage_key: String,
    pub created_at: DateTime<Utc>,
}
