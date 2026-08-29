use chrono::{DateTime, Utc};

/// A single comment on a Task item — see `docs/issues_and_features.md`'s "Add comments
/// for tasks" entry. Any project member may comment on any Task item in that project
/// (`service::comments::create_comment`); comments on virtual (not-yet-materialized)
/// series occurrences are impossible by construction, since a virtual occurrence has no
/// `item_id` row to attach to. Editing/deleting an existing comment is author-only
/// (`service::comments::update_comment`/`delete_comment`).
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: String,
    pub item_id: String,
    pub project_id: String,
    pub author_user_id: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}
