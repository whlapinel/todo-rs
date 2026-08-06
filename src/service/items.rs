use crate::domain::item::Item;
use crate::domain::recurrence;
use crate::storage::sqlite::{ItemRepo, RepoError};
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Re-exported so existing `crate::service::items::ItemError` imports across the codebase
/// keep working unchanged — the type itself now lives in `service::error` since it's shared
/// across every module in this layer (items, team_items, templates, teams), not specific to
/// items.
pub use crate::service::error::ItemError;

#[derive(Debug, Default)]
pub struct CreateItemParams {
    pub user_id: String,
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub complete: Option<bool>,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_tasks: Option<bool>,
    pub parent_item_id: Option<String>,
    pub due_offset_days: Option<i32>,
    pub timezone_offset_minutes: Option<i32>,
}

/// Moved from `json_api::items::create_item` (C.0.2 of the migration plan) — this is the one
/// place "what does creating an item mean" is decided; `json_api` and `web_ui` both call in.
pub async fn create_item(
    repo: &Arc<dyn ItemRepo>,
    params: CreateItemParams,
) -> Result<String, ItemError> {
    if let Some(ref r) = params.recurrence {
        recurrence::parse(r).map_err(ItemError::Invalid)?;
    }
    if params.recurrence.is_some() && params.parent_item_id.is_some() {
        return Err(ItemError::Invalid(
            "child items cannot have their own recurrence; set dueOffsetDays instead"
                .to_string(),
        ));
    }
    let mut item = Item::new_user_item(&params.user_id, &params.name);
    item.due_date = params.due_date;
    item.complete = params.complete.unwrap_or(false);
    item.recurrence = params.recurrence;
    item.recurrence_basis = params.recurrence_basis;
    item.has_due_time = params.has_due_time.unwrap_or(false);
    item.has_tasks = params.has_tasks.unwrap_or(true);
    item.parent_item_id = params.parent_item_id.clone();
    item.due_offset_days = params.due_offset_days;

    // Child items of a template automatically become template items.
    if let Some(ref parent_id) = params.parent_item_id
        && let Ok(parent) = repo.get(&params.user_id, parent_id).await
        && parent.is_template
    {
        item.is_template = true;
    }
    if item.due_date.is_none()
        && let Some(ref pattern) = item.recurrence
        && let Ok(rule) = recurrence::parse(pattern)
    {
        let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
        let mut deadline = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
        if rule.time_override.is_none() {
            deadline = recurrence::apply_end_of_day(deadline, tz_offset);
        } else {
            item.has_due_time = true;
        }
        item.due_date = Some(deadline);
    }
    let item_id = repo.create(&item).await?;
    Ok(item_id)
}

#[derive(Debug, Default)]
pub struct UpdateItemParams {
    pub user_id: String,
    pub item_id: String,
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_tasks: Option<bool>,
    pub parent_item_id: Option<String>,
    pub due_offset_days: Option<i32>,
    pub timezone_offset_minutes: Option<i32>,
}

/// Moved from `json_api::items::update_item`. `repo.get` below scopes the fetch to
/// `params.user_id`, so a mismatched (non-owned) `item_id` surfaces as `ItemError::NotFound`
/// rather than silently operating on someone else's item.
pub async fn update_item(
    repo: &Arc<dyn ItemRepo>,
    params: UpdateItemParams,
) -> Result<(), ItemError> {
    if let Some(ref r) = params.recurrence {
        recurrence::parse(r).map_err(ItemError::Invalid)?;
    }
    if params.recurrence.is_some() && params.parent_item_id.is_some() {
        return Err(ItemError::Invalid(
            "child items cannot have their own recurrence; set dueOffsetDays instead"
                .to_string(),
        ));
    }

    let current = repo.get(&params.user_id, &params.item_id).await?;

    let mut item = Item::new_user_item(&params.user_id, &params.name);
    item.id = params.item_id.clone();
    item.complete = params.complete;
    item.due_date = params.due_date;
    item.recurrence = params.recurrence.clone();
    item.recurrence_basis = params.recurrence_basis.clone();
    item.has_due_time = params.has_due_time.unwrap_or(false);
    item.has_tasks = params.has_tasks.unwrap_or(true);
    item.parent_item_id = params.parent_item_id.clone();
    item.due_offset_days = params.due_offset_days;
    item.assigned_to_user_id = current.assigned_to_user_id.clone();

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if let Some(next_item) = item.next_recurrence(chrono::Utc::now(), tz_offset) {
        let next_deadline = next_item
            .due_date
            .expect("next_recurrence always sets a deadline");
        let next_id = repo.create(&next_item).await?;
        clone_children(repo, &item.id, &next_id, next_deadline, tz_offset).await?;
        repo.delete(&item.id).await?;
        return Ok(());
    }

    repo.update(&item).await?;
    Ok(())
}

/// Moved from `json_api::items::delete_item`, with one behavior fix: the original never
/// scoped the delete to `user_id` at all (it deleted whatever `item_id` it was given,
/// regardless of who made the request), unlike every other item operation in this module and
/// unlike `team_items`'s `delete_team_item` (which checks `require_active_member` first).
/// `repo.get` below is scoped to `user_id`, so a non-owned `item_id` now surfaces as
/// `ItemError::NotFound` instead of being silently deleted.
pub async fn delete_item(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    item_id: &str,
) -> Result<(), ItemError> {
    repo.get(user_id, item_id).await?;

    let mut queue = vec![item_id.to_string()];
    while let Some(parent_id) = queue.first().cloned() {
        queue.remove(0);
        let children = repo.list_children(&parent_id).await?;
        for child in children {
            queue.push(child.id.clone());
            repo.delete(&child.id).await?;
        }
    }
    repo.delete(item_id).await?;
    Ok(())
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
pub(crate) fn clone_children<'a>(
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

/// Recursively copies the subtree under `template_parent_id` onto `new_parent_id`, leaving
/// the template itself untouched (unlike `clone_children`, nothing is deleted — the template
/// must stay reusable for the next "Use" click). Used by `web_ui::checklists::use_checklist`
/// when instantiating a real item from a checklist template.
///
/// Same fixed-root-offset semantics as `clone_children`: every descendant's deadline is
/// `deadline_from_offset(root_due_date, tz_offset_minutes)`, measured from the single new
/// item that was just created, not chained through intermediate copied parents. If the new
/// item has no due date at all, copied children get none either, regardless of any offset —
/// there's nothing for an offset to be measured from.
pub(crate) fn copy_template_children<'a>(
    repo: &'a Arc<dyn ItemRepo>,
    template_parent_id: &'a str,
    new_parent_id: &'a str,
    root_due_date: Option<DateTime<Utc>>,
    tz_offset_minutes: i32,
) -> Pin<Box<dyn Future<Output = Result<(), RepoError>> + Send + 'a>> {
    Box::pin(async move {
        let children = repo.list_children(template_parent_id).await?;
        for child in children {
            let mut new_child = child.clone();
            new_child.id = String::new();
            new_child.parent_item_id = Some(new_parent_id.to_string());
            new_child.complete = false;
            new_child.is_template = false;
            new_child.due_date =
                root_due_date.and_then(|root| child.deadline_from_offset(root, tz_offset_minutes));
            new_child.has_due_time = false;
            let new_child_id = repo.create(&new_child).await?;
            copy_template_children(
                repo,
                &child.id,
                &new_child_id,
                root_due_date,
                tz_offset_minutes,
            )
            .await?;
        }
        Ok(())
    })
}
