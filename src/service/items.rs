use crate::domain::item::{
    EventItem, Item, ItemKind, ItemType, Recurrence, Schedule, SimpleItem, TaskItem, TemplateItem,
};
#[cfg(test)]
use crate::domain::recurrence;
use crate::service::activity_log::reverse_entry;
use crate::service::item_series;
use crate::storage::sqlite::{
    ActivityLogRepo, ItemDependencyRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, ReminderRepo,
    RepoError,
};
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
    pub description: Option<String>,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: Option<bool>,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemKind>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    /// Internal-only — never exposed via Smithy/CLI/MCP. Set exclusively by
    /// `service::item_series::get_or_materialize_occurrence`.
    pub series_id: Option<String>,
    /// Task-only, ungated — see root CLAUDE.md's Priority section. Unlike `points`
    /// (`team_items::CreateTeamItemParams` only), this is available on personal
    /// items too.
    pub priority: Option<i32>,
}

/// Builds the `ItemType` payload for a given kind from a `CreateItemParams`/`UpdateItemParams`-
/// shaped set of flat fields — the one place that decides which of `Schedule`/`Recurrence`/
/// `event_type` a kind actually gets to carry. Personal items never get a `TeamAssignment`
/// (points/assignment are team-item-only — see `team_items::build_item_type`, its sibling).
#[allow(clippy::too_many_arguments)]
fn build_item_type(
    kind: ItemKind,
    parent_item_id: Option<String>,
    schedule: Schedule,
    recurrence: Recurrence,
    event_type: Option<String>,
    source_event_id: Option<String>,
    priority: Option<i32>,
    complete: bool,
    series_id: Option<String>,
) -> ItemType {
    match kind {
        ItemKind::Simple => ItemType::Simple(SimpleItem { parent_item_id }),
        ItemKind::Task => ItemType::Task(TaskItem {
            parent_item_id,
            schedule,
            recurrence,
            team_assignment: None,
            source_event_id,
            priority,
            complete,
            series_id,
        }),
        ItemKind::Event => ItemType::Event(EventItem {
            schedule,
            recurrence,
            event_type,
            series_id,
        }),
        ItemKind::Template => ItemType::Template(TemplateItem {
            parent_item_id,
            schedule,
            recurrence,
            event_type,
        }),
    }
}

/// Moved from `json_api::items::create_item` (C.0.2 of the migration plan) — this is the one
/// place "what does creating an item mean" is decided; `json_api` and `web_ui` both call in.
pub async fn create_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    params: CreateItemParams,
) -> Result<String, ItemError> {
    if params.item_type == Some(ItemKind::Template) {
        return Err(ItemError::Invalid(
            "item_type Template can only be set via the template creation flow".to_string(),
        ));
    }
    if let (Some(start), Some(end)) = (params.scheduled_date, params.scheduled_end_date)
        && end < start
    {
        return Err(ItemError::Invalid(
            "scheduledEndDate cannot be before scheduledDate".to_string(),
        ));
    }

    let mut kind = params.item_type.unwrap_or_default();

    // Child items of a template automatically become template items; Events can never
    // have children (see `Item::source_event_id` — a task references an event instead
    // of nesting under it).
    if let Some(ref parent_id) = params.parent_item_id
        && let Ok(parent) = repo.get(&params.user_id, parent_id).await
    {
        if parent.kind() == ItemKind::Template {
            kind = ItemKind::Template;
        }
        if parent.kind() == ItemKind::Event {
            return Err(ItemError::Invalid(
                "Events cannot have children; link a task to it via sourceEventId instead"
                    .to_string(),
            ));
        }
    }

    let schedule = Schedule {
        due_date: params.due_date,
        has_due_time: params.has_due_time.unwrap_or(false),
        scheduled_date: params.scheduled_date,
        has_scheduled_time: params.has_scheduled_time.unwrap_or(false),
        scheduled_end_date: params.scheduled_end_date,
        has_end_time: params.has_end_time.unwrap_or(false),
    };
    // Item-level recurrence is retired (Stage 10 core) — nothing can ever set
    // `pattern`/`basis` again, only `due_offset_days` survives here.
    let recurrence_data = Recurrence {
        pattern: None,
        basis: None,
        due_offset_days: params.due_offset_days,
    };

    let mut item = match kind {
        ItemKind::Simple => Item::new_simple(&params.user_id, &params.name),
        _ => Item::new_user_item(&params.user_id, &params.name),
    };
    item.item_type = build_item_type(
        kind,
        params.parent_item_id.clone(),
        schedule,
        recurrence_data,
        params.event_type.clone(),
        params.source_event_id.clone(),
        params.priority,
        params.complete.unwrap_or(false),
        params.series_id.clone(),
    );
    item.description = params.description.clone();
    // Dual-write, stage B2 (docs/project-abstraction-plan.md) — alongside the
    // still-authoritative `user_id`. Left `None` if the user somehow has no personal
    // project yet (shouldn't happen post-login, see `ensure_default_project`) rather
    // than hard-failing item creation over it.
    item.project_id = projects
        .find_personal_project(&params.user_id)
        .await?
        .map(|p| p.id);

    item.validate().map_err(ItemError::Invalid)?;

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if item.is_offset_driven() {
        let anchor = resolve_offset_anchor(repo, &params.user_id, &item).await?;
        let new_due_date = anchor.and_then(|a| item.deadline_from_offset(a, tz_offset));
        if let Some(schedule) = item.item_type.schedule_mut() {
            schedule.due_date = new_due_date;
            schedule.has_due_time = false;
        }
    }
    let item_id = repo.create(&item).await?;

    // An event-typed item can auto-instantiate matching templates' direct children as
    // sourceEventId-linked top-level tasks (see copy_template_children_to_event) — same
    // trigger the templates screen's "Use" flow shares (copy_template_children), just fired
    // by event_type matching instead of a manual click, and landing as references rather
    // than nested children since Events can't have children.
    if let Some(event_type) = item.event_type() {
        let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
        // `item` here is always the just-created Event (see this trigger's own doc comment
        // above) — its anchor is `event_anchor` (scheduled_date), never `item_anchor` (due_date).
        let root_date = event_anchor(&item);
        let templates = repo.list_templates(&params.user_id).await?;
        for tpl in templates
            .iter()
            .filter(|t| t.event_type().as_deref() == Some(event_type.as_str()))
        {
            copy_template_children_to_event(repo, &tpl.id, &item_id, root_date, tz_offset).await?;
        }
    }
    Ok(item_id)
}

#[derive(Debug, Default)]
pub struct UpdateItemParams {
    pub user_id: String,
    pub item_id: String,
    pub name: String,
    pub description: Option<String>,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: bool,
    pub has_due_time: Option<bool>,
    pub has_scheduled_time: Option<bool>,
    pub has_end_time: Option<bool>,
    pub parent_item_id: Option<String>,
    pub item_type: Option<ItemKind>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    /// Task-only, ungated — see root CLAUDE.md's Priority section. Direct-overwrite,
    /// same convention as `event_type`: omitting it on an update clears it, so every
    /// caller that isn't intentionally clearing priority must round-trip `current`'s
    /// value.
    pub priority: Option<i32>,
}

/// Moved from `json_api::items::update_item`. `repo.get` below scopes the fetch to
/// `params.user_id`, so a mismatched (non-owned) `item_id` surfaces as `ItemError::NotFound`
/// rather than silently operating on someone else's item.
///
/// `activity_log` (see docs/archived/archived_issues_and_features.md's "unify completion-undo" note) mirrors
/// `team_items::update_team_item`'s own completion logging, minus the points/assignee
/// concepts a personal item doesn't have: a top-level completion still gets logged (0
/// points, `team_id: None`), so the project activity feed's Undo button works on a
/// personal item exactly the way it already does on a team one, and an uncomplete
/// (whether via the checkbox or that Undo button) still reverses/flags that entry.
pub async fn update_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    params: UpdateItemParams,
) -> Result<(), ItemError> {
    if params.item_type == Some(ItemKind::Template) {
        return Err(ItemError::Invalid(
            "item_type Template can only be set via the template creation flow".to_string(),
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

    if current.google_event_id.is_some() {
        return Err(ItemError::Invalid(
            "this item was imported from Google Calendar and cannot be edited".to_string(),
        ));
    }

    if params.complete
        && !current.complete()
        && has_incomplete_children(repo, &params.item_id).await?
    {
        return Err(ItemError::Invalid(
            "cannot complete an item with incomplete sub-items".to_string(),
        ));
    }

    // Events can never have children (a task references an event via
    // sourceEventId instead of nesting under it).
    if let Some(ref parent_id) = params.parent_item_id
        && let Ok(parent) = repo.get(&params.user_id, parent_id).await
        && parent.kind() == ItemKind::Event
    {
        return Err(ItemError::Invalid(
            "Events cannot have children; link a task to it via sourceEventId instead".to_string(),
        ));
    }

    let kind = params.item_type.unwrap_or(current.kind());
    let schedule = Schedule {
        due_date: params.due_date,
        has_due_time: params.has_due_time.unwrap_or(false),
        scheduled_date: params.scheduled_date,
        has_scheduled_time: params.has_scheduled_time.unwrap_or(false),
        scheduled_end_date: params.scheduled_end_date,
        has_end_time: params.has_end_time.unwrap_or(false),
    };
    // Item-level recurrence is retired (Stage 10 core) — nothing can ever set
    // `pattern`/`basis` again, only `due_offset_days` survives here.
    let recurrence_data = Recurrence {
        pattern: None,
        basis: None,
        due_offset_days: params.due_offset_days,
    };

    let mut item = match kind {
        ItemKind::Simple => Item::new_simple(&params.user_id, &params.name),
        _ => Item::new_user_item(&params.user_id, &params.name),
    };
    item.item_type = build_item_type(
        kind,
        params.parent_item_id.clone(),
        schedule,
        recurrence_data,
        params.event_type.clone(),
        params.source_event_id.clone(),
        params.priority,
        params.complete,
        // Same reasoning as project_id below — series membership is set once at
        // materialization and never re-resolved from update params.
        current.series_id(),
    );
    item.id = params.item_id.clone();
    item.description = params.description.clone();
    // Carried forward from `current` rather than re-resolved (stage B2) — an item's
    // owner, and thus its personal project, never changes after creation.
    item.project_id = current.project_id.clone();

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if item.is_offset_driven() {
        let anchor = resolve_offset_anchor(repo, &params.user_id, &item).await?;
        let new_due_date = anchor.and_then(|a| item.deadline_from_offset(a, tz_offset));
        if let Some(schedule) = item.item_type.schedule_mut() {
            schedule.due_date = new_due_date;
            schedule.has_due_time = false;
        }
    }

    item.validate().map_err(ItemError::Invalid)?;

    if current.complete() && !is_pure_complete_toggle(&current, &item) {
        return Err(ItemError::Invalid(
            "cannot edit a completed item; un-complete it first".to_string(),
        ));
    }

    let just_completed = !current.complete() && item.complete();
    let just_uncompleted = current.complete() && !item.complete();
    if just_completed && item.parent_item_id().is_none() {
        activity_log
            .log_activity(
                None,
                item.project_id.as_deref(),
                &params.user_id,
                &item.id,
                &item.name,
                0,
            )
            .await?;
    }
    if just_uncompleted
        && let Some(entry) = activity_log
            .most_recent_unreversed(&item.id, &params.user_id)
            .await?
    {
        reverse_entry(projects, activity_log, &entry).await?;
    }

    repo.update(&item).await?;
    // Bug fix (docs/issues_and_features.md's "top-level parent id" entry): descendants must
    // always be measured against the true top-level ancestor's anchor, not `item`'s own anchor
    // — using `item_anchor(&item)` here silently chained a mid-chain item's own (already
    // offset-derived) date to its children whenever `item` itself has a `parent_item_id`,
    // corrupting every deeper descendant's due date.
    let old_due_anchor = top_level_anchor(repo, &params.user_id, &current).await?;
    let new_due_anchor = top_level_anchor(repo, &params.user_id, &item).await?;
    if let Some(new_due_anchor) = new_due_anchor
        && Some(new_due_anchor) != old_due_anchor
    {
        sync_offset_children(repo, &item.id, new_due_anchor, tz_offset).await?;
    }
    // A source-event-linked task's own anchor is the Event's `scheduled_date` (`event_anchor`),
    // never its `due_date` (`item_anchor`) — see that function's doc comment — so this is a
    // genuinely separate "did the anchor move" check from the due-date one above, not something
    // `top_level_anchor` above can also answer (an Event is always top-level, but its own
    // `item_anchor`/`due_date` is a different value entirely from its `event_anchor`).
    if item.kind() == ItemKind::Event {
        let old_event_anchor = event_anchor(&current);
        let new_event_anchor = event_anchor(&item);
        if let Some(new_event_anchor) = new_event_anchor
            && Some(new_event_anchor) != old_event_anchor
        {
            sync_source_event_tasks(repo, &item.id, new_event_anchor, tz_offset).await?;
        }
    }
    Ok(())
}

/// Moved from `json_api::items::delete_item`, with one behavior fix: the original never
/// scoped the delete to `user_id` at all (it deleted whatever `item_id` it was given,
/// regardless of who made the request), unlike every other item operation in this module and
/// unlike `team_items`'s `delete_team_item` (which checks `require_active_member` first).
/// `repo.get` below is scoped to `user_id`, so a non-owned `item_id` now surfaces as
/// `ItemError::NotFound` instead of being silently deleted.
///
/// Calls `item_series::unlink_deleted_item_occurrence` for every recursively-deleted child, not
/// just `item_id` itself (its caller, `project_items::delete_project_item`, already does that for
/// the top-level id after this returns) — a series-materialized item can end up as a *descendant*
/// of another item via the "Subordinate" reparenting feature, and before this fix the BFS loop's
/// raw `repo.delete(&child.id)` bypassed that unlink entirely, leaving such a child's
/// `item_occurrences` row pointing at a deleted `item_id` forever. See
/// docs/issues_and_features.md's "materialized occurrences are not properly deleted upon
/// skipping" item — this is the fix for that (skipping a *different* series' occurrence whose
/// item happened to have a reparented series-materialized descendant).
pub async fn delete_item(
    repo: &Arc<dyn ItemRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    reminders: &Arc<dyn ReminderRepo>,
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
    user_id: &str,
    item_id: &str,
) -> Result<(), ItemError> {
    let current = repo.get(user_id, item_id).await?;
    if current.google_event_id.is_some() {
        return Err(ItemError::Invalid(
            "this item was imported from Google Calendar and cannot be deleted".to_string(),
        ));
    }

    let mut queue = vec![item_id.to_string()];
    while let Some(parent_id) = queue.first().cloned() {
        queue.remove(0);
        let children = repo.list_children(&parent_id).await?;
        for child in children {
            queue.push(child.id.clone());
            repo.delete(&child.id).await?;
            item_series::unlink_deleted_item_occurrence(series, &child.id).await?;
            reminders.delete_for_item(&child.id).await?;
            item_dependencies.delete_for_item(&child.id).await?;
        }
    }
    unlink_source_event_tasks(repo, item_id).await?;
    repo.delete(item_id).await?;
    Ok(())
}

/// Clears `source_event_id` on every task referencing `event_id` — called before an item is
/// deleted, so a deleted Event unlinks the (independent, otherwise-untouched) tasks that
/// reference it rather than leaving them pointing at a nonexistent id. Deliberately *not* a
/// cascade delete: unlike structural parent/child deletion, these tasks are independent
/// entities that may carry their own points/assignment history.
pub(crate) async fn unlink_source_event_tasks(
    repo: &Arc<dyn ItemRepo>,
    event_id: &str,
) -> Result<(), RepoError> {
    let tasks = repo.list_by_source_event(event_id).await?;
    for mut task in tasks {
        if let ItemType::Task(task_item) = &mut task.item_type {
            task_item.source_event_id = None;
        }
        repo.update_by_project(&task).await?;
    }
    Ok(())
}

/// A Task's own reference date for anything measured relative to it (offset children nested via
/// `parent_item_id`, "Use template" copies) — `due_date` only, never `scheduled_date`. Scheduled
/// dates are always independently set (see `Item::validate`'s removal of the old "no scheduled
/// window on offset-driven items" restriction, docs/issues_and_features.md's "Can't schedule
/// sub-items" entry) — they must never become the basis another item's due date is derived from,
/// or a child's own "independent" scheduled window would end up silently gated on its parent's,
/// exactly the coupling that restriction existed to sidestep. A Task with no `due_date` (only a
/// `scheduled_date`) simply has no anchor at all — nothing derives from it — rather than falling
/// back to its scheduled window.
///
/// Task-only — an Event's own anchor is `event_anchor` below, never this. Conflating the two
/// was a same-day bug (see docs/archived/archived_issues_and_features.md's "Can't schedule
/// sub-items" entry's second follow-up): an Event is meant to never carry a `due_date` at all
/// (see docs/issues_and_features.md's "Remove the dueDate/dueTime field from Events" entry —
/// not yet enforced, but that's the direction), so resolving a source-event-linked task's anchor
/// through this function would silently strand it with no computable due date at all.
pub(crate) fn item_anchor(item: &Item) -> Option<DateTime<Utc>> {
    item.due_date()
}

/// An Event's own reference date for anything measured relative to it (a source-event-linked
/// task's offset, or the event-auto-trigger's template-children copy) — `scheduled_date`, never
/// `due_date`. The counterpart to `item_anchor` above: a Task chains off its ancestor's
/// `due_date`, but an Event is scheduled-window-primary and is meant to never carry a `due_date`
/// at all (see docs/issues_and_features.md's "Remove the dueDate/dueTime field from Events"
/// entry), so anything deriving a date from an Event must key off `scheduled_date` instead.
pub(crate) fn event_anchor(event: &Item) -> Option<DateTime<Utc>> {
    event.scheduled_date()
}

/// Walks `item`'s `parent_item_id` chain up to its true top-level ancestor and returns that
/// ancestor's own `item_anchor` — zero extra cost if `item` is already top-level. An offset is
/// always measured from the top-level ancestor, never chained through an intermediate parent's
/// own (possibly offset-derived) date — see CLAUDE.md's Recurrence section. Moved here from
/// `web_ui::tasks`/`web_ui::team_tasks` (built for the promote/subordinate reparent actions) so
/// `resolve_offset_anchor` below can share it instead of every offset computation duplicating
/// the walk.
pub(crate) async fn top_level_anchor(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    item: &Item,
) -> Result<Option<DateTime<Utc>>, RepoError> {
    let mut current = item.clone();
    while let Some(parent_id) = current.parent_item_id().map(|s| s.to_string()) {
        current = repo.get(user_id, &parent_id).await?;
    }
    Ok(item_anchor(&current))
}

/// Resolves the anchor date an offset-driven item's `due_date` should be measured from —
/// either the Event it references (`source_event_id`, via `event_anchor` — the event's
/// `scheduled_date`, never its `due_date`) or the true top-level ancestor of the parent it nests
/// under (`parent_item_id`, via `item_anchor`/`top_level_anchor` — that ancestor's `due_date`),
/// never both (see `Item::validate`). `None` if the item isn't offset-driven at all, or the
/// resolved anchor itself has no date.
pub(crate) async fn resolve_offset_anchor(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    item: &Item,
) -> Result<Option<DateTime<Utc>>, RepoError> {
    if let Some(event_id) = item.source_event_id() {
        let event = repo.get(user_id, &event_id).await?;
        Ok(event_anchor(&event))
    } else if let Some(parent_id) = item.parent_item_id() {
        let parent = repo.get(user_id, &parent_id).await?;
        top_level_anchor(repo, user_id, &parent).await
    } else {
        Ok(None)
    }
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
        .any(|child| !child.complete()))
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
        && current.description == item.description
        && current.due_date() == item.due_date()
        && current.scheduled_date() == item.scheduled_date()
        && current.scheduled_end_date() == item.scheduled_end_date()
        && current.recurrence_pattern() == item.recurrence_pattern()
        && current.recurrence_basis() == item.recurrence_basis()
        && current.has_due_time() == item.has_due_time()
        && current.has_scheduled_time() == item.has_scheduled_time()
        && current.has_end_time() == item.has_end_time()
        && current.parent_item_id() == item.parent_item_id()
        && current.kind() == item.kind()
        && current.event_type() == item.event_type()
        && current.due_offset_days() == item.due_offset_days()
        && current.assigned_to_user_id() == item.assigned_to_user_id()
        && current.points() == item.points()
        && current.priority() == item.priority()
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
            if child.due_offset_days().is_some() {
                let new_due_date = child.deadline_from_offset(new_anchor, tz_offset_minutes);
                if let Some(schedule) = child.item_type.schedule_mut() {
                    schedule.due_date = new_due_date;
                }
                repo.update_by_project(&child).await?;
            }
            sync_offset_children(repo, &child.id, new_anchor, tz_offset_minutes).await?;
        }
        Ok(())
    })
}

/// Recomputes `due_date` for every task referencing `event_id` via `source_event_id`, measured
/// against `new_anchor` — the source-event-reference counterpart to `sync_offset_children`,
/// called after a plain edit to an Event's own `scheduled_date` (`event_anchor`, never its
/// `due_date` — see that function's doc comment; the caller is responsible for passing the right
/// one in). Unlike `sync_offset_children`, this doesn't recurse by walking `parent_item_id`
/// itself (a source-event task is never nested), but each referencing task can have its own
/// ordinary `parent_item_id` subtree, which still needs the normal cascade (measured against
/// *that* task's own `due_date` via `item_anchor`, once its own due date is recomputed below —
/// a Task's descendants still chain off `due_date`, only the Event → task link uses
/// `scheduled_date`) — hence the trailing `sync_offset_children` call per task.
pub(crate) async fn sync_source_event_tasks(
    repo: &Arc<dyn ItemRepo>,
    event_id: &str,
    new_anchor: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<(), RepoError> {
    let tasks = repo.list_by_source_event(event_id).await?;
    for mut task in tasks {
        let new_due_date = task.deadline_from_offset(new_anchor, tz_offset_minutes);
        if let Some(schedule) = task.item_type.schedule_mut() {
            schedule.due_date = new_due_date;
        }
        repo.update_by_project(&task).await?;
        if let Some(anchor) = item_anchor(&task) {
            sync_offset_children(repo, &task.id, anchor, tz_offset_minutes).await?;
        }
    }
    Ok(())
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
/// there's nothing for an offset to be measured from. Copied children always come out as
/// `Task`s — a template child can never carry points/assignment (only `Task` has a
/// `TeamAssignment` slot at all), matching the "points/assignment are Task-only" rule.
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
            let mut schedule = child.item_type.schedule().cloned().unwrap_or_default();
            schedule.due_date =
                root_due_date.and_then(|root| child.deadline_from_offset(root, tz_offset_minutes));
            schedule.has_due_time = false;
            let recurrence = child.item_type.recurrence().cloned().unwrap_or_default();
            new_child.item_type = ItemType::Task(TaskItem {
                parent_item_id: Some(new_parent_id.to_string()),
                schedule,
                recurrence,
                team_assignment: None,
                source_event_id: None,
                priority: child.priority(),
                complete: false,
                series_id: None,
            });
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

/// Copies a matching template's *direct* children onto a newly created Event as top-level,
/// `source_event_id`-referencing tasks instead of `parent_item_id`-nested ones — Events can
/// never have children (see `Item::validate`), so this is the event-auto-trigger's own entry
/// point rather than a call to `copy_template_children` directly. Each direct child's own
/// descendants (grandchildren of the template, if any) nest normally under it via the ordinary
/// `copy_template_children` — only the direct link to the Event itself is a reference, not the
/// whole subtree; a source-event-linked task is free to have its own ordinary child subtree.
pub(crate) fn copy_template_children_to_event<'a>(
    repo: &'a Arc<dyn ItemRepo>,
    template_parent_id: &'a str,
    event_id: &'a str,
    event_anchor: Option<DateTime<Utc>>,
    tz_offset_minutes: i32,
) -> Pin<Box<dyn Future<Output = Result<(), RepoError>> + Send + 'a>> {
    Box::pin(async move {
        let children = repo.list_children(template_parent_id).await?;
        for child in children {
            let mut new_child = child.clone();
            new_child.id = String::new();
            let mut schedule = child.item_type.schedule().cloned().unwrap_or_default();
            schedule.due_date =
                event_anchor.and_then(|root| child.deadline_from_offset(root, tz_offset_minutes));
            schedule.has_due_time = false;
            let recurrence = child.item_type.recurrence().cloned().unwrap_or_default();
            new_child.item_type = ItemType::Task(TaskItem {
                parent_item_id: None,
                schedule,
                recurrence,
                team_assignment: None,
                source_event_id: Some(event_id.to_string()),
                priority: child.priority(),
                complete: false,
                series_id: None,
            });
            let new_child_id = repo.create(&new_child).await?;
            copy_template_children(
                repo,
                &child.id,
                &new_child_id,
                event_anchor,
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
/// matching `create_template`'s "templates have no dates" rule for the root. A source child's
/// `TeamAssignment`, if it had one (only possible if it was itself a `Task`), is dropped —
/// `Template` has no slot for it, matching "points/assignment are Task-only."
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
            let mut schedule = child.item_type.schedule().cloned().unwrap_or_default();
            schedule.due_date = None;
            schedule.scheduled_date = None;
            schedule.scheduled_end_date = None;
            let recurrence = child.item_type.recurrence().cloned().unwrap_or_default();
            let event_type = child.event_type();
            new_child.item_type = ItemType::Template(TemplateItem {
                parent_item_id: Some(new_template_parent_id.to_string()),
                schedule,
                recurrence,
                event_type,
            });
            let new_child_id = repo.create(&new_child).await?;
            copy_children_as_template(repo, &child.id, &new_child_id).await?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{
        MockActivityLogRepo, MockItemDependencyRepo, MockItemRepo, MockItemSeriesRepo,
        MockProjectRepo, MockReminderRepo,
    };

    /// Harmless stub for `delete_item` tests whose deleted item has no children — `delete_item`
    /// only calls `ReminderRepo::delete_for_item` per recursively-deleted child, not for the
    /// top-level id (that's `delete_project_item`'s job), so a childless delete never touches
    /// this mock at all.
    fn no_op_reminders() -> Arc<dyn ReminderRepo> {
        Arc::new(MockReminderRepo::new())
    }

    /// Same rationale as `no_op_reminders` — `delete_item` only calls
    /// `ItemDependencyRepo::delete_for_item` per recursively-deleted child.
    fn no_op_item_dependencies() -> Arc<dyn ItemDependencyRepo> {
        Arc::new(MockItemDependencyRepo::new())
    }

    /// `create_item`'s `find_personal_project` lookup, stubbed to "none found" — none
    /// of these tests care about the resolved `project_id`, so this keeps them from
    /// each having to build their own `MockProjectRepo`. Doubles as a harmless
    /// `update_item` `projects` stub for tests that never hit a genuine uncomplete
    /// transition (see `no_op_activity_log`'s doc comment) — a mock method with no
    /// matching call made against it is never an error, only an unmocked *called*
    /// method is.
    fn no_personal_project() -> Arc<dyn ProjectRepo> {
        let mut mock = MockProjectRepo::new();
        mock.expect_find_personal_project().returning(|_| Ok(None));
        Arc::new(mock)
    }

    /// `update_item` only ever touches `activity_log` on a genuine complete<->incomplete
    /// transition (see its own doc comment) — every test that isn't specifically
    /// exercising that logging/reversal behavior can pass this empty mock and never
    /// set up an expectation on it.
    fn no_op_activity_log() -> Arc<dyn ActivityLogRepo> {
        Arc::new(MockActivityLogRepo::new())
    }

    fn template_item(id: &str, user_id: &str, event_type: &str) -> Item {
        Item {
            id: id.to_string(),
            user_id: Some(user_id.to_string()),
            item_type: ItemType::Template(TemplateItem {
                parent_item_id: None,
                schedule: Schedule::default(),
                recurrence: Recurrence::default(),
                event_type: Some(event_type.to_string()),
            }),
            ..Item::default()
        }
    }

    fn template_child(id: &str, parent_id: &str, offset_days: i32) -> Item {
        Item {
            id: id.to_string(),
            item_type: ItemType::Template(TemplateItem {
                parent_item_id: Some(parent_id.to_string()),
                schedule: Schedule::default(),
                recurrence: Recurrence {
                    due_offset_days: Some(offset_days),
                    ..Recurrence::default()
                },
                event_type: None,
            }),
            ..Item::default()
        }
    }

    fn task_with_due_date(id: &str, due_date: DateTime<Utc>) -> Item {
        Item {
            id: id.to_string(),
            item_type: ItemType::Task(TaskItem {
                parent_item_id: None,
                schedule: Schedule {
                    due_date: Some(due_date),
                    ..Schedule::default()
                },
                recurrence: Recurrence::default(),
                team_assignment: None,
                source_event_id: None,
                priority: None,
                complete: false,
                series_id: None,
            }),
            ..Item::default()
        }
    }

    /// A changed `priority` alone must fail `is_pure_complete_toggle` — otherwise a
    /// completed item's priority could be edited without un-completing it first,
    /// breaking the completion-transition guard (root CLAUDE.md's Completion-transition
    /// guards section).
    #[test]
    fn is_pure_complete_toggle_rejects_a_changed_priority() {
        let mut current = task_with_due_date("item1", Utc::now());
        if let ItemType::Task(task) = &mut current.item_type {
            task.complete = true;
        }
        let mut edited = current.clone();
        if let ItemType::Task(task) = &mut edited.item_type {
            task.priority = Some(2);
        }
        assert!(!is_pure_complete_toggle(&current, &edited));
    }

    fn task_with_due_offset(id: &str, parent_id: &str, offset_days: i32) -> Item {
        Item {
            id: id.to_string(),
            item_type: ItemType::Task(TaskItem {
                parent_item_id: Some(parent_id.to_string()),
                schedule: Schedule::default(),
                recurrence: Recurrence {
                    due_offset_days: Some(offset_days),
                    ..Recurrence::default()
                },
                team_assignment: None,
                source_event_id: None,
                priority: None,
                complete: false,
                series_id: None,
            }),
            ..Item::default()
        }
    }

    #[tokio::test]
    async fn create_item_with_matching_event_type_copies_template_children() {
        let mut mock = MockItemRepo::new();

        mock.expect_create()
            .withf(|item: &Item| item.parent_item_id().is_none())
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
            .withf(|item: &Item| {
                item.parent_item_id().is_none()
                    && item.source_event_id().as_deref() == Some("new-event-id")
            })
            .times(1)
            .returning(|_| Ok("new-child-id".to_string()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "child1")
            .times(1)
            .returning(|_| Ok(vec![]));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let item_id = create_item(
            &repo,
            &no_personal_project(),
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "It rained".to_string(),
                item_type: Some(ItemKind::Event),
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
            &no_personal_project(),
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "It rained".to_string(),
                item_type: Some(ItemKind::Event),
                event_type: Some("rain".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("should create item");
    }

    fn event_item(id: &str, user_id: &str, due_date: DateTime<Utc>) -> Item {
        Item {
            id: id.to_string(),
            user_id: Some(user_id.to_string()),
            item_type: ItemType::Event(EventItem {
                schedule: Schedule {
                    due_date: Some(due_date),
                    ..Schedule::default()
                },
                recurrence: Recurrence::default(),
                event_type: None,
                series_id: None,
            }),
            ..Item::default()
        }
    }

    #[tokio::test]
    async fn create_item_rejects_parent_that_is_an_event() {
        let mut mock = MockItemRepo::new();
        let due_date = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "event1")
            .times(1)
            .returning(move |_, _| Ok(event_item("event1", "u1", due_date)));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = create_item(
            &repo,
            &no_personal_project(),
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "Sneaky child".to_string(),
                parent_item_id: Some("event1".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject an Event as parent");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn create_item_computes_due_date_from_source_events_scheduled_date() {
        // An Event is scheduled-window-primary and is meant to never carry a `due_date` at all
        // (see `event_anchor`'s doc comment and docs/issues_and_features.md's "Remove the
        // dueDate/dueTime field from Events" entry) — a source-event-linked task's offset
        // anchor must be the event's `scheduled_date`, not its `due_date`.
        let mut mock = MockItemRepo::new();
        let event_scheduled = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "event1")
            .times(1)
            .returning(move |_, _| {
                Ok(Item {
                    id: "event1".to_string(),
                    user_id: Some("u1".to_string()),
                    item_type: ItemType::Event(EventItem {
                        schedule: Schedule {
                            scheduled_date: Some(event_scheduled),
                            ..Schedule::default()
                        },
                        recurrence: Recurrence::default(),
                        event_type: None,
                        series_id: None,
                    }),
                    ..Item::default()
                })
            });

        let expected_due =
            recurrence::apply_end_of_day(event_scheduled - chrono::Duration::days(2), 0);
        mock.expect_create()
            .withf(move |item: &Item| {
                item.source_event_id().as_deref() == Some("event1")
                    && item.due_date() == Some(expected_due)
            })
            .times(1)
            .returning(|_| Ok("new-task-id".to_string()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        create_item(
            &repo,
            &no_personal_project(),
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "Buy cake".to_string(),
                source_event_id: Some("event1".to_string()),
                due_offset_days: Some(-2),
                // A manually-submitted due_date must be ignored/overwritten for an
                // offset-driven item — this stale value proves it never reaches storage.
                due_date: Some(
                    DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
                ..Default::default()
            },
        )
        .await
        .expect("should create source-event-linked task with a computed due date");
    }

    #[tokio::test]
    async fn create_item_ignores_source_events_due_date_as_anchor() {
        // Regression guard: even if an Event somehow carries a `due_date` (not supposed to
        // happen going forward, but no enforcement exists yet — see the entry referenced
        // above), it must never be used as a source-event-linked task's anchor. Only the
        // event's `scheduled_date` counts.
        let mut mock = MockItemRepo::new();
        let event_due = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let event_scheduled = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "event1")
            .times(1)
            .returning(move |_, _| {
                Ok(Item {
                    id: "event1".to_string(),
                    user_id: Some("u1".to_string()),
                    item_type: ItemType::Event(EventItem {
                        schedule: Schedule {
                            due_date: Some(event_due),
                            scheduled_date: Some(event_scheduled),
                            ..Schedule::default()
                        },
                        recurrence: Recurrence::default(),
                        event_type: None,
                        series_id: None,
                    }),
                    ..Item::default()
                })
            });

        let expected_due =
            recurrence::apply_end_of_day(event_scheduled - chrono::Duration::days(2), 0);
        mock.expect_create()
            .withf(move |item: &Item| item.due_date() == Some(expected_due))
            .times(1)
            .returning(|_| Ok("new-task-id".to_string()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        create_item(
            &repo,
            &no_personal_project(),
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "Buy cake".to_string(),
                source_event_id: Some("event1".to_string()),
                due_offset_days: Some(-2),
                ..Default::default()
            },
        )
        .await
        .expect("should anchor off scheduled_date, ignoring the event's due_date");
    }

    #[tokio::test]
    async fn delete_item_unlinks_source_event_tasks_without_deleting_them() {
        let mut mock = MockItemRepo::new();

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "event1")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "event1".to_string(),
                    user_id: Some("u1".to_string()),
                    item_type: ItemType::Event(EventItem {
                        schedule: Schedule::default(),
                        recurrence: Recurrence::default(),
                        event_type: None,
                        series_id: None,
                    }),
                    ..Item::default()
                })
            });

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "event1")
            .times(1)
            .returning(|_| Ok(vec![]));

        let mut linked_task = task_with_due_date(
            "task1",
            DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        linked_task.user_id = Some("u1".to_string());
        if let ItemType::Task(task) = &mut linked_task.item_type {
            task.source_event_id = Some("event1".to_string());
        }
        let returned_task = linked_task.clone();
        mock.expect_list_by_source_event()
            .withf(|event_id: &str| event_id == "event1")
            .times(1)
            .returning(move |_| Ok(vec![returned_task.clone()]));

        mock.expect_update_by_project()
            .withf(|item: &Item| item.id == "task1" && item.source_event_id().is_none())
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_delete()
            .withf(|id: &str| id == "event1")
            .times(1)
            .returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());

        delete_item(
            &repo,
            &series,
            &no_op_reminders(),
            &no_op_item_dependencies(),
            "u1",
            "event1",
        )
        .await
        .expect("should delete the event and unlink referencing tasks");
    }

    #[tokio::test]
    async fn create_item_rejects_template_item_type() {
        let mock = MockItemRepo::new();
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = create_item(
            &repo,
            &no_personal_project(),
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "Sneaky template".to_string(),
                item_type: Some(ItemKind::Template),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject Template item_type");

        assert!(matches!(err, ItemError::Invalid(_)));
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
            &no_personal_project(),
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
            &no_personal_project(),
            &no_op_activity_log(),
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
    async fn update_item_rejects_editing_a_google_calendar_imported_item() {
        let mut mock = MockItemRepo::new();
        mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                google_event_id: Some("gcal-uid-1".to_string()),
                calendar_subscription_id: Some("sub1".to_string()),
                ..Item::default()
            })
        });
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = update_item(
            &repo,
            &no_personal_project(),
            &no_op_activity_log(),
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Trying to rename".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject editing an imported item");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn delete_item_rejects_deleting_a_google_calendar_imported_item() {
        let mut mock = MockItemRepo::new();
        mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                google_event_id: Some("gcal-uid-1".to_string()),
                calendar_subscription_id: Some("sub1".to_string()),
                ..Item::default()
            })
        });
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());

        let err = delete_item(
            &repo,
            &series,
            &no_op_reminders(),
            &no_op_item_dependencies(),
            "u1",
            "item1",
        )
        .await
        .expect_err("should reject deleting an imported item");

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

        mock.expect_get()
            .returning(move |_, _| Ok(task_with_due_date("item1", old_due)));

        mock.expect_update()
            .withf(move |item: &Item| item.id == "item1" && item.due_date() == Some(new_due))
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(1)
            .returning(move |_| {
                Ok(vec![
                    task_with_due_offset("offset-child", "item1", 3),
                    task_with_due_date("manual-child", manual_child_due),
                ])
            });

        let expected_offset_child_due =
            recurrence::apply_end_of_day(new_due + chrono::Duration::days(3), 0);
        mock.expect_update_by_project()
            .withf(move |item: &Item| {
                item.id == "offset-child" && item.due_date() == Some(expected_offset_child_due)
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
            &no_personal_project(),
            &no_op_activity_log(),
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
    async fn update_item_plain_edit_syncs_grandchild_against_true_top_level_anchor() {
        // Regression test for docs/issues_and_features.md's "top-level parent id" bug: a
        // grandchild's offset must be measured from the true top-level ancestor's anchor, not
        // from its immediate parent's own (already offset-derived) due date.
        let mut mock = MockItemRepo::new();

        let old_due = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let new_due = DateTime::parse_from_rfc3339("2026-02-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get()
            .returning(move |_, _| Ok(task_with_due_date("item1", old_due)));

        mock.expect_update()
            .withf(move |item: &Item| item.id == "item1" && item.due_date() == Some(new_due))
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(1)
            .returning(|_| Ok(vec![task_with_due_offset("child-a", "item1", -10)]));

        let expected_child_a_due =
            recurrence::apply_end_of_day(new_due - chrono::Duration::days(10), 0);
        mock.expect_update_by_project()
            .withf(move |item: &Item| {
                item.id == "child-a" && item.due_date() == Some(expected_child_a_due)
            })
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "child-a")
            .times(1)
            .returning(|_| Ok(vec![task_with_due_offset("grandchild-b", "child-a", -5)]));

        // The key assertion: grandchild-b's offset is measured from `new_due` (the true
        // top-level anchor), not from child-a's own new due date (`new_due` - 10 days).
        let expected_grandchild_b_due =
            recurrence::apply_end_of_day(new_due - chrono::Duration::days(5), 0);
        mock.expect_update_by_project()
            .withf(move |item: &Item| {
                item.id == "grandchild-b" && item.due_date() == Some(expected_grandchild_b_due)
            })
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "grandchild-b")
            .times(1)
            .returning(|_| Ok(vec![]));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_item(
            &repo,
            &no_personal_project(),
            &no_op_activity_log(),
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Rescheduled".to_string(),
                due_date: Some(new_due),
                ..Default::default()
            },
        )
        .await
        .expect("should update and cascade both levels off the same top-level anchor");
    }

    #[tokio::test]
    async fn update_item_plain_edit_skips_sync_when_anchor_unchanged() {
        let mut mock = MockItemRepo::new();

        let due = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        mock.expect_get()
            .returning(move |_, _| Ok(task_with_due_date("item1", due)));

        mock.expect_update()
            .withf(move |item: &Item| item.id == "item1" && item.due_date() == Some(due))
            .times(1)
            .returning(|_| Ok(()));

        // Anchor (due_date) is unchanged, so sync_offset_children must never run —
        // no list_children expectation is set up; mockall panics on any unmatched call.

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_item(
            &repo,
            &no_personal_project(),
            &no_op_activity_log(),
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
                ..Item::default()
            })
        });
        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(1)
            .returning(|_| {
                Ok(vec![Item {
                    id: "child1".to_string(),
                    item_type: ItemType::Task(TaskItem {
                        parent_item_id: Some("item1".to_string()),
                        ..TaskItem::default()
                    }),
                    ..Item::default()
                }])
            });

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = update_item(
            &repo,
            &no_personal_project(),
            &no_op_activity_log(),
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
                ..Item::default()
            })
        });
        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(1)
            .returning(|_| {
                Ok(vec![Item {
                    id: "child1".to_string(),
                    item_type: ItemType::Task(TaskItem {
                        parent_item_id: Some("item1".to_string()),
                        complete: true,
                        ..TaskItem::default()
                    }),
                    ..Item::default()
                }])
            });
        mock.expect_update().times(1).returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let mut activity_log = MockActivityLogRepo::new();
        activity_log
            .expect_log_activity()
            .withf(
                |team_id, _project_id, user_id, item_id, item_name, points_delta| {
                    team_id.is_none()
                        && user_id == "u1"
                        && item_id == "item1"
                        && item_name == "Parent"
                        && *points_delta == 0
                },
            )
            .times(1)
            .returning(|_, _, _, _, _, _| Ok("entry1".to_string()));

        update_item(
            &repo,
            &no_personal_project(),
            &(Arc::new(activity_log) as Arc<dyn ActivityLogRepo>),
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
                item_type: ItemType::Task(TaskItem {
                    complete: true,
                    ..TaskItem::default()
                }),
                ..Item::default()
            })
        });

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let err = update_item(
            &repo,
            &no_personal_project(),
            &no_op_activity_log(),
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
                item_type: ItemType::Task(TaskItem {
                    complete: true,
                    ..TaskItem::default()
                }),
                ..Item::default()
            })
        });
        mock.expect_update().times(1).returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let mut activity_log = MockActivityLogRepo::new();
        // Nothing was ever logged for this item in this test, so there's nothing to
        // reverse — `update_item` must still check, though (see its own doc comment).
        activity_log
            .expect_most_recent_unreversed()
            .withf(|item_id, user_id| item_id == "item1" && user_id == "u1")
            .times(1)
            .returning(|_, _| Ok(None));

        // true -> false: un-completing, every other field round-tripped unchanged.
        update_item(
            &repo,
            &no_personal_project(),
            &(Arc::new(activity_log) as Arc<dyn ActivityLogRepo>),
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

    #[tokio::test]
    async fn create_item_resolves_project_id_from_personal_project() {
        let mut mock = MockItemRepo::new();
        mock.expect_create()
            .withf(|item: &Item| item.project_id.as_deref() == Some("p1"))
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_find_personal_project()
            .withf(|user_id: &str| user_id == "u1")
            .returning(|_| {
                Ok(Some(crate::domain::project::Project {
                    id: "p1".to_string(),
                    name: "Personal".to_string(),
                    owner_user_id: "u1".to_string(),
                    team_id: None,
                }))
            });
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        create_item(
            &repo,
            &projects,
            CreateItemParams {
                user_id: "u1".to_string(),
                name: "Buy milk".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create item");
    }

    #[tokio::test]
    async fn update_item_carries_forward_project_id_from_current() {
        let mut mock = MockItemRepo::new();
        mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                user_id: Some("u1".to_string()),
                project_id: Some("p1".to_string()),
                name: "Same name".to_string(),
                ..Item::default()
            })
        });
        mock.expect_update()
            .withf(|item: &Item| item.project_id.as_deref() == Some("p1"))
            .times(1)
            .returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_item(
            &repo,
            &no_personal_project(),
            &no_op_activity_log(),
            UpdateItemParams {
                user_id: "u1".to_string(),
                item_id: "item1".to_string(),
                name: "Renamed".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("should update and carry project_id forward");
    }
}
