use crate::domain::item::{Item, ItemType};
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
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: Option<bool>,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemType>,
    pub event_type: Option<String>,
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
    if params.item_type == Some(ItemType::Template) {
        return Err(ItemError::Invalid(
            "item_type Template can only be set via the template creation flow"
                .to_string(),
        ));
    }
    if let (Some(start), Some(end)) = (params.scheduled_date, params.scheduled_end_date)
        && end < start
    {
        return Err(ItemError::Invalid(
            "scheduledEndDate cannot be before scheduledDate".to_string(),
        ));
    }
    let mut item = match params.item_type.unwrap_or_default() {
        ItemType::Simple => Item::new_simple(&params.user_id, &params.name),
        ItemType::Event => Item::new_event(&params.user_id, &params.name),
        ItemType::Task => Item::new_task(&params.user_id, &params.name),
        // Unreachable via user input (rejected above); the "child of a template"
        // auto-promotion below is what actually produces a Template item.
        ItemType::Template => Item::new_user_item(&params.user_id, &params.name),
    };
    item.due_date = params.due_date;
    item.scheduled_date = params.scheduled_date;
    item.scheduled_end_date = params.scheduled_end_date;
    item.complete = params.complete.unwrap_or(false);
    item.recurrence = params.recurrence;
    item.recurrence_basis = params.recurrence_basis;
    item.has_due_time = params.has_due_time.unwrap_or(false);
    item.has_scheduled_time = params.has_scheduled_time.unwrap_or(false);
    item.has_end_time = params.has_end_time.unwrap_or(false);
    item.parent_item_id = params.parent_item_id.clone();
    item.event_type = params.event_type.clone();
    item.due_offset_days = params.due_offset_days;

    // Child items of a template automatically become template items.
    if let Some(ref parent_id) = params.parent_item_id
        && let Ok(parent) = repo.get(&params.user_id, parent_id).await
        && parent.item_type == ItemType::Template
    {
        item.item_type = ItemType::Template;
    }
    item.validate().map_err(ItemError::Invalid)?;
    if let Some(ref pattern) = item.recurrence
        && let Ok(rule) = recurrence::parse(pattern)
    {
        let basis = item.recurrence_basis.as_deref().unwrap_or("DUE_DATE");
        let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
        if basis == "DUE_DATE" && item.due_date.is_none() {
            let mut deadline = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
            if rule.time_override.is_none() {
                deadline = recurrence::apply_end_of_day(deadline, tz_offset);
            } else {
                item.has_due_time = true;
            }
            item.due_date = Some(deadline);
        } else if basis != "DUE_DATE" && item.scheduled_date.is_none() {
            let when = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
            if rule.time_override.is_some() {
                item.has_scheduled_time = true;
            }
            item.scheduled_date = Some(when);
        }
    }
    let item_id = repo.create(&item).await?;

    // An event-typed item can auto-instantiate matching templates'
    // children onto itself — same mechanism the templates screen's "Use" flow
    // already uses, just triggered by event_type matching instead of a manual click.
    if let Some(ref event_type) = item.event_type {
        let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
        let root_date = item_anchor(&item);
        let templates = repo.list_templates(&params.user_id).await?;
        for tpl in templates
            .iter()
            .filter(|t| t.event_type.as_deref() == Some(event_type.as_str()))
        {
            copy_template_children(repo, &tpl.id, &item_id, root_date, tz_offset).await?;
        }
    }
    Ok(item_id)
}

#[derive(Debug, Default)]
pub struct UpdateItemParams {
    pub user_id: String,
    pub item_id: String,
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemType>,
    pub event_type: Option<String>,
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
    if params.item_type == Some(ItemType::Template) {
        return Err(ItemError::Invalid(
            "item_type Template can only be set via the template creation flow"
                .to_string(),
        ));
    }
    if let (Some(start), Some(end)) = (params.scheduled_date, params.scheduled_end_date)
        && end < start
    {
        return Err(ItemError::Invalid(
            "scheduledEndDate cannot be before scheduledDate".to_string(),
        ));
    }

    let current = repo.get(&params.user_id, &params.item_id).await?;

    if params.complete
        && !current.complete
        && has_incomplete_children(repo, &params.item_id).await?
    {
        return Err(ItemError::Invalid(
            "cannot complete an item with incomplete sub-items".to_string(),
        ));
    }

    let item_type = params.item_type.unwrap_or(current.item_type);
    let mut item = match item_type {
        ItemType::Simple => Item::new_simple(&params.user_id, &params.name),
        ItemType::Event => Item::new_event(&params.user_id, &params.name),
        ItemType::Task => Item::new_task(&params.user_id, &params.name),
        ItemType::Template => Item::new_user_item(&params.user_id, &params.name),
    };
    item.item_type = item_type;
    item.id = params.item_id.clone();
    item.complete = params.complete;
    item.due_date = params.due_date;
    item.scheduled_date = params.scheduled_date;
    item.scheduled_end_date = params.scheduled_end_date;
    item.recurrence = params.recurrence.clone();
    item.recurrence_basis = params.recurrence_basis.clone();
    item.has_due_time = params.has_due_time.unwrap_or(false);
    item.has_scheduled_time = params.has_scheduled_time.unwrap_or(false);
    item.has_end_time = params.has_end_time.unwrap_or(false);
    item.parent_item_id = params.parent_item_id.clone();
    item.event_type = params.event_type.clone();
    item.due_offset_days = params.due_offset_days;
    item.assigned_to_user_id = current.assigned_to_user_id.clone();
    item.validate().map_err(ItemError::Invalid)?;

    if current.complete && !is_pure_complete_toggle(&current, &item) {
        return Err(ItemError::Invalid(
            "cannot edit a completed item; un-complete it first".to_string(),
        ));
    }

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if let Some((next_item, next_anchor)) = item.next_recurrence(chrono::Utc::now(), tz_offset) {
        let next_id = repo.create(&next_item).await?;
        clone_children(repo, &item.id, &next_id, next_anchor, tz_offset).await?;
        repo.delete(&item.id).await?;
        return Ok(());
    }

    repo.update(&item).await?;
    let (old_anchor, new_anchor) = (item_anchor(&current), item_anchor(&item));
    if let Some(new_anchor) = new_anchor
        && Some(new_anchor) != old_anchor
    {
        sync_offset_children(repo, &item.id, new_anchor, tz_offset).await?;
    }
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

/// An item's own reference date for anything measured relative to it (offset children,
/// template auto-trigger copies) — `due_date` if set, else `scheduled_date`, else neither.
pub(crate) fn item_anchor(item: &Item) -> Option<DateTime<Utc>> {
    item.due_date.or(item.scheduled_date)
}

/// True if `item_id` has at least one direct child that isn't complete — used to block a
/// fresh `false -> true` completion until every direct child is done. Direct children only
/// is sufficient by induction: each child was itself gated the same way when *it* was
/// completed, so an already-complete child's own descendants are guaranteed complete too.
pub(crate) async fn has_incomplete_children(
    repo: &Arc<dyn ItemRepo>,
    item_id: &str,
) -> Result<bool, RepoError> {
    Ok(repo
        .list_children(item_id)
        .await?
        .iter()
        .any(|child| !child.complete))
}

/// True if `item` differs from `current` only in `complete` — the edit-lock's definition of
/// "just the checkbox changed." Deliberately an explicit field allowlist rather than a
/// derived whole-struct `PartialEq`: `update_item`/`update_team_item` always build `item` via
/// `Item::new_*` (a fresh default) and then overlay fields from params, so it would otherwise
/// spuriously differ from `current` on fields the caller never intended to touch — e.g.
/// `has_children`, which isn't a real column at all (see `src/storage/sqlite/items.rs`'s
/// `EXISTS(...)` subquery) and so is always `false` on a freshly-built `item` regardless of
/// `current`'s DB-populated value.
pub(crate) fn is_pure_complete_toggle(current: &Item, item: &Item) -> bool {
    current.name == item.name
        && current.due_date == item.due_date
        && current.scheduled_date == item.scheduled_date
        && current.scheduled_end_date == item.scheduled_end_date
        && current.recurrence == item.recurrence
        && current.recurrence_basis == item.recurrence_basis
        && current.has_due_time == item.has_due_time
        && current.has_scheduled_time == item.has_scheduled_time
        && current.has_end_time == item.has_end_time
        && current.parent_item_id == item.parent_item_id
        && current.item_type == item.item_type
        && current.event_type == item.event_type
        && current.due_offset_days == item.due_offset_days
        && current.assigned_to_user_id == item.assigned_to_user_id
        && current.points == item.points
}

/// Recomputes `due_date` for every descendant of `parent_item_id` that has its own
/// `due_offset_days` set, measured against `new_anchor` — called after a plain edit to the
/// parent's own anchor date (see `item_anchor`), independent of recurrence. Unlike
/// `clone_children`, this updates children in place (same ids, no re-parenting) and skips any
/// child that has no `due_offset_days` — an independently-dated child isn't derived from the
/// parent, so a parent edit shouldn't touch it. Same fixed-root-offset semantics as
/// `clone_children` otherwise: every descendant, not just direct children, is measured against
/// the same `new_anchor`, not chained through an intermediate parent's own (already-recomputed)
/// date.
pub(crate) fn sync_offset_children<'a>(
    repo: &'a Arc<dyn ItemRepo>,
    parent_item_id: &'a str,
    new_anchor: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Pin<Box<dyn Future<Output = Result<(), RepoError>> + Send + 'a>> {
    Box::pin(async move {
        let children = repo.list_children(parent_item_id).await?;
        for mut child in children {
            if child.due_offset_days.is_some() {
                child.due_date = child.deadline_from_offset(new_anchor, tz_offset_minutes);
                if child.team_id.is_some() {
                    repo.update_team_item(&child).await?;
                } else {
                    repo.update(&child).await?;
                }
            }
            sync_offset_children(repo, &child.id, new_anchor, tz_offset_minutes).await?;
        }
        Ok(())
    })
}

/// Recursively copies the subtree under `template_parent_id` onto `new_parent_id`, leaving
/// the template itself untouched (unlike `clone_children`, nothing is deleted — the template
/// must stay reusable for the next "Use" click). Used by `web_ui::templates::use_template_form`
/// when instantiating a real item from a template.
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
            new_child.item_type = ItemType::Task;
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

/// Recursively copies the subtree under `source_parent_id` onto `new_template_parent_id`,
/// converting each descendant into a `Template`-typed row — the mirror image of
/// `copy_template_children` above (which copies FROM a template TO a real item; this copies
/// FROM a real item TO a template). Used by `service::templates::create_template` when a
/// "save as template" request names a `source_item_id`, so the resulting template actually
/// reflects that item's children, not just its own fields.
///
/// `due_offset_days` rides along unchanged (that's what lets `copy_template_children` later
/// recompute each child's deadline when the template is used); dates themselves are cleared,
/// matching `create_template`'s "templates have no dates" rule for the root.
pub(crate) fn copy_children_as_template<'a>(
    repo: &'a Arc<dyn ItemRepo>,
    source_parent_id: &'a str,
    new_template_parent_id: &'a str,
) -> Pin<Box<dyn Future<Output = Result<(), RepoError>> + Send + 'a>> {
    Box::pin(async move {
        let children = repo.list_children(source_parent_id).await?;
        for child in children {
            let mut new_child = child.clone();
            new_child.id = String::new();
            new_child.parent_item_id = Some(new_template_parent_id.to_string());
            new_child.complete = false;
            new_child.item_type = ItemType::Template;
            new_child.due_date = None;
            new_child.scheduled_date = None;
            new_child.scheduled_end_date = None;
            let new_child_id = repo.create(&new_child).await?;
            copy_children_as_template(repo, &child.id, &new_child_id).await?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::MockItemRepo;

    fn template_item(id: &str, user_id: &str, event_type: &str) -> Item {
        Item {
            id: id.to_string(),
            user_id: Some(user_id.to_string()),
            item_type: ItemType::Template,
            event_type: Some(event_type.to_string()),
            ..Item::default()
        }
    }

    fn template_child(id: &str, parent_id: &str, offset_days: i32) -> Item {
        Item {
            id: id.to_string(),
            parent_item_id: Some(parent_id.to_string()),
            item_type: ItemType::Template,
            due_offset_days: Some(offset_days),
            ..Item::default()
        }
    }

    #[tokio::test]
    async fn create_item_with_matching_event_type_copies_template_children() {
        let mut mock = MockItemRepo::new();

        mock.expect_create()
            .withf(|item: &Item| item.parent_item_id.is_none())
            .times(1)
            .returning(|_| Ok("new-event-id".to_string()));

        mock.expect_list_templates()
            .withf(|user_id: &str| user_id == "u1")
            .times(1)
            .returning(|_| Ok(vec![template_item("tpl1", "u1", "rain")]));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "tpl1")
            .times(1)
            .returning(|_| Ok(vec![template_child("child1", "tpl1", 1)]));

        mock.expect_create()
            .withf(|item: &Item| item.parent_item_id.as_deref() == Some("new-event-id"))
            .times(1)
            .returning(|_| Ok("new-child-id".to_string()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "child1")
            .times(1)
            .returning(|_| Ok(vec![]));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let item_id = create_item(
            &repo,
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "It rained".to_string(),
                item_type: Some(ItemType::Event),
                event_type: Some("rain".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("should create item");

        assert_eq!(item_id, "new-event-id");
    }

    #[tokio::test]
    async fn create_item_with_no_matching_template_creates_nothing_else() {
        let mut mock = MockItemRepo::new();

        mock.expect_create()
            .times(1)
            .returning(|_| Ok("new-event-id".to_string()));

        mock.expect_list_templates()
            .times(1)
            .returning(|_| Ok(vec![template_item("tpl1", "u1", "snow")]));

        // No expectations set on create (beyond the one above) or list_children — mockall
        // panics on an unexpected call, so this also proves the mismatch short-circuits the
        // trigger before touching either.
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        create_item(
            &repo,
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "It rained".to_string(),
                item_type: Some(ItemType::Event),
                event_type: Some("rain".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("should create item");
    }

    #[tokio::test]
    async fn create_item_rejects_template_item_type() {
        let mock = MockItemRepo::new();
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = create_item(
            &repo,
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "Sneaky template".to_string(),
                item_type: Some(ItemType::Template),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject Template item_type");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_item_recurrence_anchors_child_offset_to_new_scheduled_date_not_stale_due_date() {
        let mut mock = MockItemRepo::new();

        let stale_due_date = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let scheduled_base = DateTime::parse_from_rfc3339("2026-01-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get()
            .returning(move |_, _| {
                Ok(Item {
                    id: "item1".to_string(),
                    user_id: Some("u1".to_string()),
                    ..Item::default()
                })
            });

        // The recurring item itself is re-created fresh (fresh id) with an advanced
        // scheduled_date; due_date rides along unchanged (still the stale value).
        mock.expect_create()
            .withf(|item: &Item| item.parent_item_id.is_none())
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));

        // Called twice: once by the parent-gating check (Stage 5), once by
        // `clone_children` when the recurrence branch fires. The child is already
        // complete, matching what parent-gating requires before a fresh completion.
        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(2)
            .returning(|_| {
                Ok(vec![Item {
                    id: "child1".to_string(),
                    parent_item_id: Some("item1".to_string()),
                    due_offset_days: Some(2),
                    complete: true,
                    ..Item::default()
                }])
            });

        let rule = recurrence::parse("every week").unwrap();
        let expected_scheduled = recurrence::next_date(&rule, scheduled_base, 0);
        let expected_child_due =
            recurrence::apply_end_of_day(expected_scheduled + chrono::Duration::days(2), 0);

        mock.expect_create()
            .withf(move |item: &Item| {
                item.parent_item_id.as_deref() == Some("new-item-id")
                    && item.due_date == Some(expected_child_due)
            })
            .times(1)
            .returning(|_| Ok("new-child-id".to_string()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "child1")
            .times(1)
            .returning(|_| Ok(vec![]));

        mock.expect_delete()
            .withf(|id: &str| id == "child1")
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_delete()
            .withf(|id: &str| id == "item1")
            .times(1)
            .returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Work session".to_string(),
                complete: true,
                recurrence: Some("every week".to_string()),
                recurrence_basis: Some("SCHEDULED_DATE".to_string()),
                scheduled_date: Some(scheduled_base),
                due_date: Some(stale_due_date),
                ..Default::default()
            },
        )
        .await
        .expect("should recur and clone children");
    }

    #[tokio::test]
    async fn create_item_rejects_scheduled_end_before_start() {
        let mock = MockItemRepo::new();
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let start = chrono::DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let end = start - chrono::Duration::days(1);

        let err = create_item(
            &repo,
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "Backwards window".to_string(),
                scheduled_date: Some(start),
                scheduled_end_date: Some(end),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject end before start");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_item_rejects_scheduled_end_before_start() {
        let mock = MockItemRepo::new();
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let start = chrono::DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let end = start - chrono::Duration::days(1);

        let err = update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Backwards window".to_string(),
                scheduled_date: Some(start),
                scheduled_end_date: Some(end),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject end before start");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_item_plain_edit_syncs_offset_children_leaving_non_offset_children_untouched() {
        let mut mock = MockItemRepo::new();

        let old_due = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let new_due = DateTime::parse_from_rfc3339("2026-01-17T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let manual_child_due = DateTime::parse_from_rfc3339("2026-01-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get().returning(move |_, _| {
            Ok(Item {
                id: "item1".to_string(),
                user_id: Some("u1".to_string()),
                due_date: Some(old_due),
                ..Item::default()
            })
        });

        mock.expect_update()
            .withf(move |item: &Item| item.id == "item1" && item.due_date == Some(new_due))
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(1)
            .returning(move |_| {
                Ok(vec![
                    Item {
                        id: "offset-child".to_string(),
                        parent_item_id: Some("item1".to_string()),
                        due_offset_days: Some(3),
                        ..Item::default()
                    },
                    Item {
                        id: "manual-child".to_string(),
                        parent_item_id: Some("item1".to_string()),
                        due_date: Some(manual_child_due),
                        ..Item::default()
                    },
                ])
            });

        let expected_offset_child_due =
            recurrence::apply_end_of_day(new_due + chrono::Duration::days(3), 0);
        mock.expect_update()
            .withf(move |item: &Item| {
                item.id == "offset-child" && item.due_date == Some(expected_offset_child_due)
            })
            .times(1)
            .returning(|_| Ok(()));

        // manual-child has no due_offset_days, so it must never be passed to `update` at all —
        // no expectation is set up for it; mockall panics on any unmatched call.
        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "offset-child")
            .times(1)
            .returning(|_| Ok(vec![]));
        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "manual-child")
            .times(1)
            .returning(|_| Ok(vec![]));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Rescheduled".to_string(),
                due_date: Some(new_due),
                ..Default::default()
            },
        )
        .await
        .expect("should update and sync offset children only");
    }

    #[tokio::test]
    async fn update_item_plain_edit_skips_sync_when_anchor_unchanged() {
        let mut mock = MockItemRepo::new();

        let due = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get().returning(move |_, _| {
            Ok(Item {
                id: "item1".to_string(),
                user_id: Some("u1".to_string()),
                due_date: Some(due),
                ..Item::default()
            })
        });

        mock.expect_update()
            .withf(move |item: &Item| item.id == "item1" && item.due_date == Some(due))
            .times(1)
            .returning(|_| Ok(()));

        // Anchor (due_date) is unchanged, so sync_offset_children must never run —
        // no list_children expectation is set up; mockall panics on any unmatched call.

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Renamed only".to_string(),
                due_date: Some(due),
                ..Default::default()
            },
        )
        .await
        .expect("should update without touching children");
    }

    #[tokio::test]
    async fn update_item_rejects_completion_with_incomplete_child() {
        let mut mock = MockItemRepo::new();

        mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                user_id: Some("u1".to_string()),
                complete: false,
                ..Item::default()
            })
        });
        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(1)
            .returning(|_| {
                Ok(vec![Item {
                    id: "child1".to_string(),
                    parent_item_id: Some("item1".to_string()),
                    complete: false,
                    ..Item::default()
                }])
            });

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Parent".to_string(),
                complete: true,
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject completing with an incomplete child");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_item_allows_completion_when_all_children_complete() {
        let mut mock = MockItemRepo::new();

        mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                user_id: Some("u1".to_string()),
                complete: false,
                ..Item::default()
            })
        });
        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(1)
            .returning(|_| {
                Ok(vec![Item {
                    id: "child1".to_string(),
                    parent_item_id: Some("item1".to_string()),
                    complete: true,
                    ..Item::default()
                }])
            });
        mock.expect_update().times(1).returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Parent".to_string(),
                complete: true,
                ..Default::default()
            },
        )
        .await
        .expect("should allow completion when all children are complete");
    }

    #[tokio::test]
    async fn update_item_rejects_field_edit_on_completed_item() {
        let mut mock = MockItemRepo::new();

        mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                user_id: Some("u1".to_string()),
                name: "Original name".to_string(),
                complete: true,
                ..Item::default()
            })
        });

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Changed name".to_string(),
                complete: true,
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject editing a field on a completed item");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_item_allows_pure_complete_toggle_both_directions() {
        let mut mock = MockItemRepo::new();

        mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                user_id: Some("u1".to_string()),
                name: "Same name".to_string(),
                complete: true,
                ..Item::default()
            })
        });
        mock.expect_update().times(1).returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        // true -> false: un-completing, every other field round-tripped unchanged.
        update_item(
            &repo,
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Same name".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("pure toggle should be allowed on a completed item");
    }
}
