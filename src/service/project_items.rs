use crate::domain::item::{Item, ItemKind};
use crate::domain::project::Project;
use crate::service::error::ItemError;
use crate::service::item_dependencies;
use crate::service::item_series;
use crate::service::items::{self, CreateItemParams, UpdateItemParams, item_anchor};
use crate::service::projects::require_project_member;
use crate::service::reminders;
use crate::service::team_items::{
    self, CreateTeamItemParams, UpdateTeamItemContext, UpdateTeamItemParams,
};
use crate::storage::sqlite::{
    ActivityLogRepo, DueItem, ItemDependencyRepo, ItemRepo, ItemSeriesRepo, ProjectRepo,
    ReminderRepo, TeamRepo,
};
use async_recursion::async_recursion;
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Stage B3's unified read path — replaces the personal-vs-team authorization
/// branch (`repo.get`/`repo.get_team_item`, gated by two different checks) with a
/// single membership check against the item's owning project, per
/// docs/project-abstraction-plan.md's access-check formula. Not yet reachable via
/// HTTP (that's stage B4's `ProjectItem` Smithy resource) — unit-tested only, same
/// precedent stages A2/A3 set.
pub async fn get_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    item_id: &str,
) -> Result<Item, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(repo.get_by_project(project_id, item_id).await?)
}

/// Stage B3's unified list path — same shape as `get_project_item` above, wrapping
/// `ItemRepo::list_by_project`.
pub async fn list_project_items(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    parent_item_id: Option<String>,
) -> Result<Vec<Item>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(repo.list_by_project(project_id, parent_item_id).await?)
}

/// Unchecked twin of `get_project_item` — for use only when the caller has already verified
/// project membership earlier in the same request (e.g. via `get_project`, `get_project_item`,
/// or another checked call in this file). Exists so a follow-up read within an
/// already-authorized request doesn't have to pay for a second, redundant
/// `require_project_member` call — while still keeping the repo call itself inside the
/// service layer rather than in `web_ui`.
pub async fn get_project_item_unchecked(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item_id: &str,
) -> Result<Item, ItemError> {
    Ok(repo.get_by_project(project_id, item_id).await?)
}

/// Unchecked twin of `list_project_items` — see `get_project_item_unchecked`'s doc comment.
/// Also covers what used to be raw `ItemRepo::list_children(parent_item_id)` calls in
/// `web_ui` (pass `parent_item_id: Some(..)`) — this is strictly more scoped than that, since
/// it additionally requires the parent to belong to `project_id`.
pub async fn list_project_items_unchecked(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    parent_item_id: Option<String>,
) -> Result<Vec<Item>, ItemError> {
    Ok(repo.list_by_project(project_id, parent_item_id).await?)
}

/// Lists the Tasks linked to an Event via `sourceEventId` (see CLAUDE.md's Events section) —
/// `ItemRepo::list_by_source_event` isn't itself scoped by project, so this confirms the event
/// belongs to `project_id` first. Unchecked: assumes the caller already verified project
/// membership earlier in the same request (see `get_project_item_unchecked`'s doc comment).
pub async fn list_project_event_children_unchecked(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    event_item_id: &str,
) -> Result<Vec<Item>, ItemError> {
    repo.get_by_project(project_id, event_item_id).await?;
    Ok(repo.list_by_source_event(event_item_id).await?)
}

/// Unchecked project fetch — see `get_project_item_unchecked`'s doc comment.
pub async fn get_project_unchecked(
    projects: &Arc<dyn ProjectRepo>,
    project_id: &str,
) -> Result<Project, ItemError> {
    Ok(projects.get(project_id).await?)
}

/// Checked read of a project's due items, mirroring `get_project_item`/`list_project_items`'s
/// shape — use as the sole membership check for a request when no other `Project` data (e.g.
/// `team_id`) is needed. See `list_due_project_items_unchecked` for the already-checked case.
pub async fn list_due_project_items(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    deadline_after: Option<i64>,
    deadline_before: Option<i64>,
) -> Result<Vec<DueItem>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(repo
        .list_due_by_project(project_id, deadline_after, deadline_before)
        .await?)
}

/// Unchecked twin of `list_due_project_items` — see `get_project_item_unchecked`'s doc
/// comment.
pub async fn list_due_project_items_unchecked(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    deadline_after: Option<i64>,
    deadline_before: Option<i64>,
) -> Result<Vec<DueItem>, ItemError> {
    Ok(repo
        .list_due_by_project(project_id, deadline_after, deadline_before)
        .await?)
}

/// Walks `item`'s `parent_item_id` chain up to its true top-level ancestor within
/// `project_id` and returns that ancestor's own `item_anchor` — the project-scoped
/// counterpart to `service::items::top_level_anchor`/`service::team_items::top_level_anchor_team`.
/// Not exposed to `web_ui` directly: only called from `resolve_promotion_target`/
/// `resolve_subordination_target` below, which each own the single membership check for the
/// request this walk happens inside of.
async fn resolve_top_level_anchor_unchecked(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<DateTime<Utc>>, ItemError> {
    let mut current = item.clone();
    while let Some(parent_id) = current.parent_item_id.clone() {
        current = repo.get_by_project(project_id, &parent_id).await?;
    }
    Ok(item_anchor(&current))
}

/// What a promote action needs to reparent an item onto its own grandparent (see CLAUDE.md's
/// promote/subordinate reparent actions): the item itself, its new parent (`grandparent`,
/// `None` if the item's parent was already top-level), and — when reparenting to top level —
/// the offset anchor the item's own children should now be measured from.
pub struct PromotionTarget {
    pub current: Item,
    pub grandparent: Option<Item>,
    pub offset_anchor: Option<DateTime<Utc>>,
}

/// Resolves a promotion in one membership-checked call — replaces a handler doing a
/// membership-only `get_project` call followed by up to three raw, unchecked
/// `repo.get_by_project` calls (current, parent, grandparent) plus a separate top-level-anchor
/// walk. Checks membership exactly once, then walks the repo directly since the check has
/// already happened within this same call.
pub async fn resolve_promotion_target(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    item_id: &str,
) -> Result<PromotionTarget, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let current = repo.get_by_project(project_id, item_id).await?;
    let Some(parent_id) = current.parent_item_id.clone() else {
        return Err(ItemError::Invalid(
            "item has no parent to promote from".to_string(),
        ));
    };
    let parent = repo.get_by_project(project_id, &parent_id).await?;
    let grandparent = match &parent.parent_item_id {
        Some(gp_id) => Some(repo.get_by_project(project_id, gp_id).await?),
        None => None,
    };
    let offset_anchor = match &grandparent {
        Some(gp) => resolve_top_level_anchor_unchecked(repo, project_id, gp).await?,
        None => None,
    };
    Ok(PromotionTarget {
        current,
        grandparent,
        offset_anchor,
    })
}

/// What a subordinate action needs to reparent an item onto a sibling (`new_parent`).
pub struct SubordinationTarget {
    pub current: Item,
    pub new_parent: Item,
    pub offset_anchor: Option<DateTime<Utc>>,
}

/// Resolves a subordination in one membership-checked call — same rationale as
/// `resolve_promotion_target`.
pub async fn resolve_subordination_target(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    item_id: &str,
    new_parent_id: &str,
) -> Result<SubordinationTarget, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let current = repo.get_by_project(project_id, item_id).await?;
    let new_parent = repo.get_by_project(project_id, new_parent_id).await?;
    if new_parent.parent_item_id != current.parent_item_id {
        return Err(ItemError::Invalid(
            "target is not a sibling of this item".to_string(),
        ));
    }
    let offset_anchor = resolve_top_level_anchor_unchecked(repo, project_id, &new_parent).await?;
    Ok(SubordinationTarget {
        current,
        new_parent,
        offset_anchor,
    })
}

#[derive(Debug, Default)]
pub struct CreateProjectItemParams {
    pub project_id: String,
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
    pub assigned_to_user_id: Option<String>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub points: Option<i32>,
    /// Task-only, ungated — see root CLAUDE.md's Priority section.
    pub priority: Option<i32>,
    /// Internal-only — never exposed via Smithy/CLI/MCP. Set exclusively by
    /// `service::item_series::get_or_materialize_occurrence`.
    pub series_id: Option<String>,
}

/// Stage B4's unified create path. Rather than reimplementing the recurrence/
/// offset/event-trigger/points machinery a third time, this resolves `project_id`
/// down to a plain `user_id` (personal project) or delegates straight to
/// `items::create_item`/`team_items::create_team_item` — the same functions the
/// legacy `Item`/`TeamItem` operations already call, so personal project-created
/// items still dual-write `user_id` exactly like those do, keeping the legacy read
/// APIs consistent (see docs/project-abstraction-plan.md's stage B4 dual-write-bridge
/// verification). Team-backed items no longer dual-write `items.team_id` — that
/// column was dropped in Stage 6 of docs/team-id-removal-plan.md. `assigned_to_user_id`/
/// `points` are simply dropped for a personal project — `CreateItemParams` has no slot
/// for either, matching personal items never having carried a `TeamAssignment` at all.
pub async fn create_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    reminders_repo: &Arc<dyn ReminderRepo>,
    requester_user_id: &str,
    params: CreateProjectItemParams,
) -> Result<String, ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    let project = projects.get(&params.project_id).await?;
    let project_id = params.project_id.clone();
    let item_id = match project.team_id {
        Some(_) => {
            team_items::create_team_item(
                repo,
                teams,
                projects,
                requester_user_id,
                CreateTeamItemParams {
                    project_id: params.project_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    assigned_to_user_id: params.assigned_to_user_id,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                    points: params.points,
                    priority: params.priority,
                    series_id: params.series_id,
                },
            )
            .await
        }
        None => {
            items::create_item(
                repo,
                projects,
                CreateItemParams {
                    user_id: project.owner_user_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                    series_id: params.series_id,
                    priority: params.priority,
                },
            )
            .await
        }
    }?;
    let item = repo.get_by_project(&project_id, &item_id).await?;
    reminders::sync_item_reminders(reminders_repo, projects, &item).await?;
    Ok(item_id)
}

#[derive(Debug, Default)]
pub struct UpdateProjectItemParams {
    pub project_id: String,
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
    pub assigned_to_user_id: Option<String>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub points: Option<i32>,
    /// Task-only, ungated — see root CLAUDE.md's Priority section.
    pub priority: Option<i32>,
    /// "Depends on" (docs/issues_and_features.md) — `None` means "leave dependencies
    /// unchanged" (deliberately not this struct's usual direct-overwrite-Option
    /// convention; see `service::item_dependencies::set_item_dependencies`'s doc comment
    /// for why). `Some(vec![])` clears every dependency.
    pub depends_on_item_ids: Option<Vec<String>>,
}

/// Stage B4's unified update path — same delegation shape as `create_project_item`.
/// `activity_log` is only ever touched on the team-backed branch (points award/
/// reversal, see `team_items::update_team_item`); the personal branch's
/// `items::update_item` has no use for it at all.
///
/// `series` (Stage 9 of docs/recurring-events-virtual-occurrences-rough-plan.md)
/// is the completion-side counterpart to `delete_project_item`'s own `series`
/// parameter: after a successful update that requests `complete: true`, this calls
/// `item_series::record_task_completion` — a cheap no-op for the overwhelmingly common
/// case (`item_id` never came from a series). Gated on `params.complete` rather than
/// on detecting an actual incomplete→complete *transition*, since `record_task_completion`'s
/// own cursor advance is already idempotent (`advance_cursor`'s forward-only max) — no
/// extra pre-read of the item's prior state is needed to make this safe to call on a
/// no-op re-completion. Stage 10a added a *pre*-persistence counterpart,
/// `item_series::validate_completable`, gated the same way but run before the dispatch
/// below rather than after — it's what actually rejects an out-of-order Task-series
/// completion; `record_task_completion` itself never rejects anything.
///
/// The Uncomplete side mirrors this but can't reuse the same idempotency trick:
/// `retreat_cursor`'s backward-only min isn't safe to call on every `complete: false`
/// request the way `advance_cursor` is on every `complete: true` one, since most
/// `complete: false` requests are plain edits of an item that was never complete to
/// begin with, not an actual complete→incomplete transition. So both
/// `item_series::validate_uncompletable` (pre-persistence, rejection) and
/// `item_series::record_task_uncompletion` (post-persistence, cursor restore) are
/// gated on `was_complete` — a real fetch-and-check of the item's prior state — rather
/// than on `!params.complete` alone.
pub async fn update_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    activity_log: &Arc<dyn ActivityLogRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    reminders_repo: &Arc<dyn ReminderRepo>,
    item_dependencies_repo: &Arc<dyn ItemDependencyRepo>,
    requester_user_id: &str,
    params: UpdateProjectItemParams,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    let project = projects.get(&params.project_id).await?;
    let complete = params.complete;
    let item_id = params.item_id.clone();
    let project_id = params.project_id.clone();
    let depends_on_item_ids = params.depends_on_item_ids.clone();

    // `was_complete` is this item's *pre-update* completion state — needed below for three
    // things: gating the Task-series uncomplete-order check and `record_task_uncompletion`
    // (unlike `record_task_completion`'s forward-only `advance_cursor`, retreating the cursor
    // isn't idempotent, so it must only fire on a genuine transition), gating the
    // incomplete-dependency check to a genuine `false -> true` transition (not a no-op
    // re-completion), and gating the "depends-on edits are frozen once complete" check below
    // the same way `is_pure_complete_toggle` gates the analogous Item-field lock (on the
    // item's state *before* this update, not after). Always fetched now (previously skipped
    // when `complete == true`, since only the uncomplete-order check needed it) — the depends-on
    // guards below need it in both directions.
    let was_complete = repo.get_by_project(&project_id, &item_id).await?.complete;

    // Stage 10a: reject an out-of-order Task-series completion before any mutation
    // happens — `record_task_completion` below only ever runs after a successful
    // persist, so it can no longer be the rejection point.
    if complete {
        item_series::validate_completable(
            series,
            &item_id,
            params.timezone_offset_minutes.unwrap_or(0),
        )
        .await?;
        // "Depends on" (docs/issues_and_features.md): reject a fresh incomplete->complete
        // transition while any dependency is still incomplete — the same shape as
        // `service::items::has_incomplete_children`'s own gate, just resolved one layer up
        // here since dependencies aren't threaded through `items::update_item`/
        // `team_items::update_team_item` at all (see `has_incomplete_dependencies`'s doc
        // comment). Only checked on a genuine transition, not on a no-op re-completion of an
        // already-complete item or a plain field edit.
        if !was_complete
            && item_dependencies::has_incomplete_dependencies(
                item_dependencies_repo,
                repo,
                &project_id,
                &item_id,
            )
            .await?
        {
            return Err(ItemError::Invalid(
                "cannot complete an item while it has an incomplete dependency".to_string(),
            ));
        }
    } else if was_complete {
        // The Uncomplete-side counterpart to the completable check above: reject
        // out-of-order Task-series uncompletion before any mutation happens, so the
        // cursor can never get out of sync with what's actually settled.
        item_series::validate_uncompletable(series, &item_id).await?;
    }

    let result = match project.team_id {
        Some(team_id) => {
            team_items::update_team_item(
                repo,
                &UpdateTeamItemContext {
                    teams: teams.clone(),
                    projects: projects.clone(),
                    activity_log: activity_log.clone(),
                },
                requester_user_id,
                &team_id,
                UpdateTeamItemParams {
                    project_id: params.project_id,
                    item_id: params.item_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    assigned_to_user_id: params.assigned_to_user_id,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                    points: params.points,
                    priority: params.priority,
                },
            )
            .await
        }
        None => {
            items::update_item(
                repo,
                projects,
                activity_log,
                UpdateItemParams {
                    user_id: project.owner_user_id,
                    item_id: params.item_id,
                    name: params.name,
                    description: params.description,
                    due_date: params.due_date,
                    scheduled_date: params.scheduled_date,
                    scheduled_end_date: params.scheduled_end_date,
                    complete: params.complete,
                    has_due_time: params.has_due_time,
                    has_scheduled_time: params.has_scheduled_time,
                    has_end_time: params.has_end_time,
                    parent_item_id: params.parent_item_id,
                    item_type: params.item_type,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    source_event_id: params.source_event_id,
                    timezone_offset_minutes: params.timezone_offset_minutes,
                    priority: params.priority,
                },
            )
            .await
        }
    };
    result?;
    if complete {
        item_series::record_task_completion(series, &item_id).await?;
    } else if was_complete {
        item_series::record_task_uncompletion(
            series,
            &item_id,
            params.timezone_offset_minutes.unwrap_or(0),
        )
        .await?;
    }
    let item = repo.get_by_project(&project_id, &item_id).await?;
    reminders::sync_item_reminders(reminders_repo, projects, &item).await?;
    if let Some(ids) = depends_on_item_ids {
        // Dependencies live in a side table, not on `Item` itself, so `items::update_item`/
        // `team_items::update_team_item`'s own `is_pure_complete_toggle` field-lock (CLAUDE.md's
        // Completion-transition guards) can't see or enforce this — a request that pure-toggles
        // `complete` (or leaves it at `true`) would otherwise sail through here uncontested.
        // Mirror that lock's own rejection explicitly, gated the same way it is (on the item's
        // state *before* this update, `was_complete` — not `item.complete`, which is the
        // post-update state): a fresh incomplete->complete transition may still set
        // dependencies in the same request (the Item-field lock allows that too, since it never
        // engages on a first completion), only an item that was *already* complete has its
        // dependency set frozen, the same as every other field, un-freezable only by
        // un-completing the item first.
        if was_complete {
            return Err(ItemError::Invalid(
                "cannot edit a completed item; un-complete it first".to_string(),
            ));
        }
        item_dependencies::set_item_dependencies(
            item_dependencies_repo,
            repo,
            &project_id,
            &item,
            &ids,
        )
        .await?;
    }
    Ok(())
}

pub async fn duplicate_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    project_id: &str,
    item_id: &str,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let mut copy = repo.get_by_project(project_id, item_id).await?;
    copy.name = format!("{} (copy)", copy.name);
    let copy_id = repo.create(&copy).await?;
    copy_children(&copy.id, &copy_id, repo).await?;
    Ok(())
}

#[async_recursion]
async fn copy_children(
    old_parent_id: &str,
    new_parent_id: &str,
    repo: &Arc<dyn ItemRepo>,
) -> Result<(), ItemError> {
    let children = repo.list_children(old_parent_id).await?;
    for mut child in children {
        child.parent_item_id = Some(new_parent_id.to_string());
        let copy_id = repo.create(&child).await?;
        copy_children(&child.id, &copy_id, repo).await?;
    }
    Ok(())
}

/// Stage B4's unified delete path — same delegation shape as `create_project_item`.
/// Stage 6 of docs/recurring-events-virtual-occurrences-rough-plan.md added the
/// trailing `item_series::unlink_deleted_item_occurrence` call: every delete of an
/// item goes through here regardless of caller (web UI, CLI, MCP), so this is the one
/// place that can catch "this item was a materialized series occurrence" for all of
/// them, rather than duplicating that check into each per-screen delete handler.
pub async fn delete_project_item(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    reminders_repo: &Arc<dyn ReminderRepo>,
    item_dependencies_repo: &Arc<dyn ItemDependencyRepo>,
    requester_user_id: &str,
    project_id: &str,
    item_id: &str,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    let project = projects.get(project_id).await?;
    match project.team_id {
        Some(_) => {
            team_items::delete_team_item(
                repo,
                series,
                reminders_repo,
                item_dependencies_repo,
                teams,
                projects,
                requester_user_id,
                project_id,
                item_id,
            )
            .await?;
        }
        None => {
            items::delete_item(
                repo,
                series,
                reminders_repo,
                item_dependencies_repo,
                &project.owner_user_id,
                item_id,
            )
            .await?
        }
    }
    item_series::unlink_deleted_item_occurrence(series, item_id).await?;
    reminders_repo.delete_for_item(item_id).await?;
    // "Depends on" cleanup (docs/issues_and_features.md) for the top-level id itself —
    // `delete_item`/`delete_team_item` above already did this for every recursively-deleted
    // *child* (mirroring `reminders_repo.delete_for_item`'s identical split). Both directions
    // matter here: `item_id`'s own dependency rows, and any sibling's now-dangling reference
    // to `item_id` as *its* dependency.
    item_dependencies_repo.delete_for_item(item_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::project::Project;
    use crate::domain::team::TeamRole;
    use crate::storage::sqlite::{
        MockActivityLogRepo, MockItemDependencyRepo, MockItemRepo, MockItemSeriesRepo,
        MockProjectRepo, MockReminderRepo, MockTeamRepo,
    };

    /// Every `delete_project_item` test but the dedicated series-unlinking one below just
    /// needs the reverse lookup to be a harmless no-op — this is what those tests share.
    fn no_op_series_repo() -> Arc<dyn ItemSeriesRepo> {
        let mut mock = MockItemSeriesRepo::new();
        mock.expect_find_occurrence_by_item_id()
            .returning(|_| Ok(None));
        Arc::new(mock)
    }

    /// `create_project_item`/`update_project_item`/`delete_project_item` all resync/clear
    /// reminders as their final step — a harmless no-op stub for tests that don't care about
    /// reminder rows at all.
    fn no_op_reminders_repo() -> Arc<dyn ReminderRepo> {
        let mut mock = MockReminderRepo::new();
        mock.expect_sync_auto_reminders()
            .returning(|_, _, _, _| Ok(()));
        mock.expect_delete_for_item().returning(|_| Ok(()));
        Arc::new(mock)
    }

    /// `update_project_item`/`delete_project_item` both touch this on every call (the
    /// completion-guard read and/or the final cleanup/set step) — a harmless no-op stub for
    /// tests that don't care about dependency rows at all.
    fn no_op_item_dependencies_repo() -> Arc<dyn ItemDependencyRepo> {
        let mut mock = MockItemDependencyRepo::new();
        mock.expect_list_for_item().returning(|_| Ok(vec![]));
        mock.expect_delete_for_item().returning(|_| Ok(()));
        mock.expect_set_dependencies().returning(|_, _| Ok(()));
        Arc::new(mock)
    }

    fn personal_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Personal".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: None,
        }
    }

    fn shared_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Shared".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: Some("team1".to_string()),
        }
    }

    #[tokio::test]
    async fn get_project_item_allows_owner_on_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "i1")
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Task")));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_project_item(&repo, &projects, &teams, "p1", "owner1", "i1")
            .await
            .unwrap();
        assert_eq!(item.name, "Task");
    }

    #[tokio::test]
    async fn get_project_item_rejects_non_owner_on_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let items_mock = MockItemRepo::new();

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_project_item(&repo, &projects, &teams, "p1", "not-owner", "i1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_project_item_allows_active_team_member_on_shared_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let teams_mock = MockTeamRepo::new();
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Task")));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let item = get_project_item(&repo, &projects, &teams, "p1", "member1", "i1")
            .await
            .unwrap();
        assert_eq!(item.name, "Task");
    }

    #[tokio::test]
    async fn get_project_item_rejects_inactive_team_member_on_shared_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(None));
        let teams_mock = MockTeamRepo::new();
        let items_mock = MockItemRepo::new();

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams_mock);

        let result = get_project_item(&repo, &projects, &teams, "p1", "pending1", "i1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_project_items_delegates_to_repo_after_membership_check() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_list_by_project()
            .withf(|project_id: &str, parent_item_id: &Option<String>| {
                project_id == "p1" && parent_item_id.is_none()
            })
            .returning(|_, _| {
                Ok(vec![
                    Item::new_user_item("owner1", "One"),
                    Item::new_user_item("owner1", "Two"),
                ])
            });

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let items = list_project_items(&repo, &projects, &teams, "p1", "owner1", None)
            .await
            .unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn list_project_items_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let items_mock = MockItemRepo::new();

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = list_project_items(&repo, &projects, &teams, "p1", "not-owner", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_project_item_delegates_to_personal_item_creation() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        projects_mock
            .expect_find_personal_project()
            .returning(|_| Ok(None));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.user_id.as_deref() == Some("owner1"))
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Buy milk")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let reminders = no_op_reminders_repo();

        let item_id = create_project_item(
            &repo,
            &projects,
            &teams,
            &reminders,
            "owner1",
            CreateProjectItemParams {
                project_id: "p1".to_string(),
                name: "Buy milk".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create personal project item");

        assert_eq!(item_id, "new-item-id");
    }

    #[tokio::test]
    async fn create_project_item_delegates_to_team_item_creation() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.project_id.as_deref() == Some("p1") && item.user_id.is_none())
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Mow the lawn")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let reminders = no_op_reminders_repo();

        let item_id = create_project_item(
            &repo,
            &projects,
            &teams,
            &reminders,
            "member1",
            CreateProjectItemParams {
                project_id: "p1".to_string(),
                name: "Mow the lawn".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create team project item");

        assert_eq!(item_id, "new-item-id");
    }

    #[tokio::test]
    async fn create_project_item_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let reminders = no_op_reminders_repo();

        let result = create_project_item(
            &repo,
            &projects,
            &teams,
            &reminders,
            "not-owner",
            CreateProjectItemParams {
                project_id: "p1".to_string(),
                name: "Sneaky".to_string(),
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_project_item_delegates_to_personal_item_update() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Old name")));
        // `complete: false` in the request against an already-incomplete item is not a
        // complete->incomplete transition, so this is the `!complete` fetch that gates
        // `validate_uncompletable`/`record_task_uncompletion` (see update_project_item's
        // doc comment) — not a series-related call at all here.
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Old name")));
        items_mock.expect_update().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());
        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "New name".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("should update personal project item");
    }

    #[tokio::test]
    async fn update_project_item_rejects_completing_with_an_incomplete_dependency() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        // Every `get_by_project` call in this test — the item's own pre-update fetch and
        // `has_incomplete_dependencies`' lookup of its one dependency — resolves to a plain
        // incomplete item, so the dependency is what blocks completion here.
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Task")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());
        let series = no_op_series_repo();
        let reminders = no_op_reminders_repo();
        let mut dep_mock = MockItemDependencyRepo::new();
        dep_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec!["dep1".to_string()]));
        let item_dependencies: Arc<dyn ItemDependencyRepo> = Arc::new(dep_mock);

        let err = update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "Task".to_string(),
                complete: true,
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject completion while a dependency is incomplete");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_project_item_rejects_editing_dependencies_on_an_already_complete_item() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        // `was_complete`'s pre-update fetch — the item is already complete, so a pure
        // resubmit (`complete: true` again) reaches the dependency-edit branch, which must
        // reject before ever calling `ItemDependencyRepo::set_dependencies`.
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get_by_project().returning(|_, _| {
            let mut item = Item::new_user_item("owner1", "Task");
            item.complete = true;
            Ok(item)
        });
        items_mock.expect_get().returning(|_, _| {
            let mut item = Item::new_user_item("owner1", "Task");
            item.complete = true;
            Ok(item)
        });
        items_mock.expect_update().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());
        let series = no_op_series_repo();
        let reminders = no_op_reminders_repo();
        // No expectations set — asserts `set_dependencies` is never reached.
        let item_dependencies: Arc<dyn ItemDependencyRepo> =
            Arc::new(MockItemDependencyRepo::new());

        let err = update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "Task".to_string(),
                complete: true,
                depends_on_item_ids: Some(vec!["dep1".to_string()]),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject editing dependencies on an already-complete item");
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_project_item_triggers_series_completion_hook_when_completing() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Old name")));
        items_mock.expect_list_children().returning(|_| Ok(vec![]));
        items_mock.expect_update().times(1).returning(|_| Ok(()));
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Old name")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut activity_log_mock = MockActivityLogRepo::new();
        // A personal item's completion now logs too (see docs/archived/archived_issues_and_features.md's "unify
        // completion-undo" note) — 0 points, no team.
        activity_log_mock
            .expect_log_activity()
            .withf(
                |team_id, _project_id, user_id, item_id, item_name, points_delta| {
                    team_id.is_none()
                        && user_id == "owner1"
                        && item_id == "i1"
                        && item_name == "Old name"
                        && *points_delta == 0
                },
            )
            .times(1)
            .returning(|_, _, _, _, _, _| Ok("entry1".to_string()));
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(activity_log_mock);

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .withf(|item_id: &str| item_id == "i1")
            // Stage 10a: called twice — once by the pre-persistence
            // `validate_completable` check, once by the post-persistence
            // `record_task_completion` hook. Both are cheap no-ops here since this
            // item never came from a series.
            .times(2)
            .returning(|_| Ok(None));
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "Old name".to_string(),
                complete: true,
                ..Default::default()
            },
        )
        .await
        .expect("should update and check for a linked series occurrence");
    }

    #[tokio::test]
    async fn update_project_item_triggers_series_uncompletion_hook_when_uncompleting() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Old name")));
        // The `!complete` fetch that decides whether this is actually a
        // complete->incomplete transition — the item was complete, and the request
        // asks for `complete: false`, so it is.
        items_mock.expect_get_by_project().returning(|_, _| {
            let mut item = Item::new_user_item("owner1", "Old name");
            item.complete = true;
            Ok(item)
        });
        items_mock.expect_update().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .withf(|item_id: &str| item_id == "i1")
            // Called twice — once by the pre-persistence `validate_uncompletable`
            // check, once by the post-persistence `record_task_uncompletion` hook.
            // Both are cheap no-ops here since this item never came from a series.
            .times(2)
            .returning(|_| Ok(None));
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "Old name".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("should update and check for a linked series occurrence");
    }

    #[tokio::test]
    async fn update_project_item_delegates_to_team_item_update() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Old name")));
        items_mock
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());
        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "member1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "New name".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("should update team project item");
    }

    /// End-to-end through `update_project_item` (not `team_items::update_team_item`
    /// directly) for a team-backed item that's both points-bearing *and* a
    /// materialized series occurrence — the two reversal concerns (points, via
    /// `team_items::update_team_item`'s existing `reverse_entry` call, and the series
    /// cursor, via `item_series::record_task_uncompletion`) are independent hooks off
    /// the same `complete: false` request and must both fire together on an
    /// uncomplete of a series-backed team item.
    #[tokio::test]
    async fn update_project_item_uncomplete_reverses_points_and_restores_series_cursor() {
        use crate::domain::activity_log::ActivityLogEntry;
        use crate::domain::item::{ItemType, Recurrence, Schedule, TeamAssignment};
        use crate::domain::item_series::{ItemOccurrence, ItemSeries};

        let occurrence_date = DateTime::from_timestamp(1_700_500_000, 0).unwrap();
        let series_recurrence = "every 7 days".to_string();
        let rule = crate::domain::recurrence::parse(&series_recurrence).unwrap();
        let expected_previous = crate::domain::recurrence::retreat_once(&rule, occurrence_date, 0);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        projects_mock
            .expect_add_project_points()
            .withf(|project_id, user_id, delta| {
                project_id == "p1" && user_id == "member1" && *delta == -15
            })
            .times(1)
            .returning(|_, _, _| Ok(0));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "i1".to_string(),
                name: "Standup".to_string(),
                project_id: Some("p1".to_string()),
                complete: true,
                item_type: ItemType::Task {
                    schedule: Schedule::default(),
                    recurrence: Recurrence::default(),
                    team_assignment: Some(TeamAssignment {
                        points: Some(15),
                        assigned_to_user_id: Some("member1".to_string()),
                    }),
                    source_event_id: None,
                    priority: None,
                },
                ..Item::default()
            })
        });
        items_mock
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut activity_log_mock = MockActivityLogRepo::new();
        activity_log_mock
            .expect_most_recent_unreversed()
            .withf(|item_id, user_id| item_id == "i1" && user_id == "member1")
            .times(1)
            .returning(|_, _| {
                Ok(Some(ActivityLogEntry {
                    id: "entry1".to_string(),
                    team_id: Some("team1".to_string()),
                    project_id: Some("p1".to_string()),
                    user_id: "member1".to_string(),
                    item_id: "i1".to_string(),
                    item_name: "Standup".to_string(),
                    points_delta: 15,
                    reversed: false,
                    created_at: chrono::Utc::now(),
                }))
            });
        activity_log_mock
            .expect_mark_reversed()
            .withf(|id| id == "entry1")
            .times(1)
            .returning(|_| Ok(()));
        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(activity_log_mock);

        let task_series = ItemSeries {
            id: "s1".to_string(),
            project_id: "p1".to_string(),
            name: "Standup".to_string(),
            description: None,
            event_type: None,
            recurrence: series_recurrence,
            anchor_date: occurrence_date - chrono::Duration::days(7),
            item_type: ItemKind::Task,
            cursor_date: Some(occurrence_date),
            basis: None,
            template_item_id: None,
            assigned_to_user_id: Some("member1".to_string()),
            points: Some(15),
            priority: None,
        };
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .withf(|item_id: &str| item_id == "i1")
            .times(2)
            .returning(move |_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date,
                    item_id: Some("i1".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .times(2)
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_get_occurrence()
            .returning(move |_, date| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: date,
                    item_id: Some("i1".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_retreat_cursor()
            .withf(move |series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == expected_previous
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "member1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "Standup".to_string(),
                complete: false,
                assigned_to_user_id: Some("member1".to_string()),
                points: Some(15),
                ..Default::default()
            },
        )
        .await
        .expect("should reverse points and restore the series cursor together");
    }

    #[tokio::test]
    async fn delete_project_item_delegates_to_personal_item_delete() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Task")));
        items_mock.expect_list_children().returning(|_| Ok(vec![]));
        items_mock
            .expect_list_by_source_event()
            .returning(|_| Ok(vec![]));
        items_mock.expect_delete().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let series = no_op_series_repo();

        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();
        delete_project_item(
            &repo,
            &projects,
            &teams,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            "p1",
            "i1",
        )
        .await
        .expect("should delete personal project item");
    }

    #[tokio::test]
    async fn delete_project_item_delegates_to_team_item_delete() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Task")));
        items_mock.expect_list_children().returning(|_| Ok(vec![]));
        items_mock
            .expect_list_by_source_event()
            .returning(|_| Ok(vec![]));
        items_mock.expect_delete().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let series = no_op_series_repo();

        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();
        delete_project_item(
            &repo,
            &projects,
            &teams,
            &series,
            &reminders,
            &item_dependencies,
            "member1",
            "p1",
            "i1",
        )
        .await
        .expect("should delete team project item");
    }

    /// Regression test for the bug docs/team-id-removal-plan.md exists to fix:
    /// before Stage 4, `team_items::update_team_item` scoped its fetch/update via
    /// `repo.get_team_item(&project.team_id, item_id)`/`update_team_item` — the
    /// *project's current* team, not the item's own (frozen-at-creation) `team_id`
    /// column (removed entirely in Stage 6), which `ProjectRepo::attach_team`/
    /// `detach_team` never touched. An item created while the project was attached
    /// to `team1`, read again after the project was re-attached to `team2`, would
    /// 404 forever even though the item itself never moved. Post-Stage4,
    /// `update_team_item` scopes via `repo.get_by_project`/`update_by_project`
    /// instead — `project_id` never changes when a project's attached team is
    /// swapped, so this now succeeds.
    #[tokio::test]
    async fn update_project_item_succeeds_after_project_reattaches_a_different_team() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| {
            Ok(Project {
                id: "p1".to_string(),
                name: "Shared".to_string(),
                owner_user_id: "owner1".to_string(),
                team_id: Some("team2".to_string()),
            })
        });
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "i1".to_string(),
                project_id: Some("p1".to_string()),
                name: "Old name".to_string(),
                ..Item::default()
            })
        });
        items_mock
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let activity_log: Arc<dyn ActivityLogRepo> = Arc::new(MockActivityLogRepo::new());
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());
        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();

        update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            "member1",
            UpdateProjectItemParams {
                project_id: "p1".to_string(),
                item_id: "i1".to_string(),
                name: "New name".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect(
            "update should succeed even though the project's team changed since the item was created",
        );
    }

    /// Delete-side twin of `update_project_item_succeeds_after_project_reattaches_a_different_team`.
    #[tokio::test]
    async fn delete_project_item_succeeds_after_project_reattaches_a_different_team() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| {
            Ok(Project {
                id: "p1".to_string(),
                name: "Shared".to_string(),
                owner_user_id: "owner1".to_string(),
                team_id: Some("team2".to_string()),
            })
        });
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "i1".to_string(),
                project_id: Some("p1".to_string()),
                name: "Task".to_string(),
                ..Item::default()
            })
        });
        items_mock.expect_list_children().returning(|_| Ok(vec![]));
        items_mock
            .expect_list_by_source_event()
            .returning(|_| Ok(vec![]));
        items_mock.expect_delete().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let series = no_op_series_repo();

        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();
        delete_project_item(&repo, &projects, &teams, &series, &reminders, &item_dependencies, "member1", "p1", "i1")
            .await
            .expect(
                "delete should succeed even though the project's team changed since the item was created",
            );
    }

    #[tokio::test]
    async fn delete_project_item_unlinks_a_materialized_series_occurrence() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Standup")));
        items_mock.expect_list_children().returning(|_| Ok(vec![]));
        items_mock
            .expect_list_by_source_event()
            .returning(|_| Ok(vec![]));
        items_mock.expect_delete().times(1).returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .withf(|item_id: &str| item_id == "i1")
            .returning(|_| {
                Ok(Some(crate::domain::item_series::ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                    item_id: Some("i1".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_delete_occurrence()
            .withf(|series_id: &str, _date| series_id == "s1")
            .times(1)
            .returning(|_, _| Ok(()));
        series_mock.expect_mark_exdate().times(0);
        series_mock.expect_get_series().times(0);
        series_mock.expect_advance_cursor().times(0);
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();
        delete_project_item(
            &repo,
            &projects,
            &teams,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            "p1",
            "i1",
        )
        .await
        .expect("should delete the item and un-materialize its occurrence");
    }

    /// Regression test for docs/issues_and_features.md's "materialized occurrences are not
    /// properly deleted upon skipping" — a series-materialized item can be reparented under
    /// another (unrelated) item via the "Subordinate" feature, so `items::delete_item`'s
    /// recursive child-delete loop must call `unlink_deleted_item_occurrence` for every
    /// deleted child too, not just for the top-level id `delete_project_item` was given
    /// (which the dedicated test above already covers).
    #[tokio::test]
    async fn delete_project_item_unlinks_a_series_materialized_descendant() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get()
            .returning(|_, _| Ok(Item::new_user_item("owner1", "Parent")));
        items_mock
            .expect_list_children()
            .withf(|parent_id: &str| parent_id == "i1")
            .returning(|_| {
                Ok(vec![Item {
                    id: "i2".to_string(),
                    ..Item::new_user_item("owner1", "Reparented child")
                }])
            });
        items_mock
            .expect_list_children()
            .withf(|parent_id: &str| parent_id == "i2")
            .returning(|_| Ok(vec![]));
        items_mock
            .expect_list_by_source_event()
            .returning(|_| Ok(vec![]));
        items_mock
            .expect_delete()
            .withf(|id: &str| id == "i1" || id == "i2")
            .times(2)
            .returning(|_| Ok(()));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .withf(|item_id: &str| item_id == "i2")
            .times(1)
            .returning(|_| {
                Ok(Some(crate::domain::item_series::ItemOccurrence {
                    series_id: "s2".to_string(),
                    occurrence_date: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                    item_id: Some("i2".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_delete_occurrence()
            .withf(|series_id: &str, _date| series_id == "s2")
            .times(1)
            .returning(|_, _| Ok(()));
        // "i1" (the delete target itself) was never materialized from a series.
        series_mock
            .expect_find_occurrence_by_item_id()
            .withf(|item_id: &str| item_id == "i1")
            .times(1)
            .returning(|_| Ok(None));
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let reminders = no_op_reminders_repo();
        let item_dependencies = no_op_item_dependencies_repo();
        delete_project_item(
            &repo,
            &projects,
            &teams,
            &series,
            &reminders,
            &item_dependencies,
            "owner1",
            "p1",
            "i1",
        )
        .await
        .expect(
            "should delete the whole tree and un-materialize the reparented child's occurrence",
        );
    }
}
