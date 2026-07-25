pub mod invites;
pub mod items;
pub mod team_items;
pub mod teams;
pub mod templates;
pub mod users;
use crate::storage::sqlite::{ItemRepo, RepoError};
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use todo_server_sdk::error;

fn internal(msg: impl ToString) -> error::PeoplesRepublicOfListsError {
    error::PeoplesRepublicOfListsError {
        message: msg.to_string(),
    }
}

fn not_found() -> error::PeoplesRepublicOfListsError {
    error::PeoplesRepublicOfListsError {
        message: "not found".to_string(),
    }
}

/// Recursively re-parents the subtree under `old_parent_id` onto `new_parent_id`,
/// creating fresh (incomplete) copies of every descendant. Used when a recurring
/// item completes and is replaced by a new instance, so its children aren't
/// orphaned pointing at the deleted parent.
///
/// Every descendant's deadline is recomputed from its own `due_offset_days`
/// against `root_deadline` — the new deadline of the item that actually recurred,
/// not each descendant's immediate parent. This is a fixed reference for the
/// whole subtree, so a grandchild's offset is measured from the same root as a
/// direct child's, not chained through an intermediate parent's own offset.
/// Children have no independent recurrence (rejected at input validation), so
/// their own prior deadline is never consulted — offset-or-none, always.
fn clone_children<'a>(
    repo: &'a Arc<dyn ItemRepo>,
    old_parent_id: &'a str,
    new_parent_id: &'a str,
    root_deadline: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Pin<Box<dyn Future<Output = Result<(), RepoError>> + Send + 'a>> {
    Box::pin(async move {
        let children = repo.list_children(old_parent_id).await?;
        for child in children {
            let mut new_child = child.clone();
            new_child.id = String::new();
            new_child.parent_item_id = Some(new_parent_id.to_string());
            new_child.complete = false;
            new_child.due_date = child.deadline_from_offset(root_deadline, tz_offset_minutes);
            new_child.has_due_time = false;
            let new_child_id = repo.create(&new_child).await?;
            clone_children(
                repo,
                &child.id,
                &new_child_id,
                root_deadline,
                tz_offset_minutes,
            )
            .await?;
            repo.delete(&child.id).await?;
        }
        Ok(())
    })
}
