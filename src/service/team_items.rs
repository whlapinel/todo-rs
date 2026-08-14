use crate::domain::item::{Item, ItemKind, ItemType, Recurrence, Schedule, TeamAssignment};
use crate::domain::recurrence;
use crate::service::activity_log::reverse_entry;
use crate::service::items::{
    archive_recurrence, clone_children, copy_template_children_to_event, has_incomplete_children,
    is_pure_complete_toggle, item_anchor, repoint_source_event_tasks, sync_offset_children,
    sync_source_event_tasks, unlink_source_event_tasks, ItemError,
};
use crate::service::projects::{require_project_admin, require_project_member, resolve_project_assignee};
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, ProjectRepo, TeamRepo};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Bundles the extra repos `update_team_item` needs for points award/reversal (see
/// CLAUDE.md's Points plan, Stage 6), so its own argument list doesn't keep growing.
/// `create_team_item`/`delete_team_item` never touch points at completion time (see
/// that module doc), so they keep taking a plain `&Arc<dyn TeamRepo>` instead.
/// `projects` was added in stage C1 (docs/project-abstraction-plan.md) — points
/// authority moved from `team_members` to `project_members`.
pub struct UpdateTeamItemContext {
    pub teams: Arc<dyn TeamRepo>,
    pub projects: Arc<dyn ProjectRepo>,
    pub activity_log: Arc<dyn ActivityLogRepo>,
}

#[derive(Debug, Default)]
pub struct CreateTeamItemParams {
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
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
    pub item_type: Option<ItemKind>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub points: Option<i32>,
}

/// Builds the `ItemType` payload for a team item. `team_assignment` is only ever
/// `Some` for `Task` — points/assignment are Task-only (see issues.md); requesting
/// them on any other kind is silently dropped, the same shape as the existing
/// non-admin-points-request handling below, rather than rejecting the rest of an
/// otherwise-valid request.
fn build_item_type(
    kind: ItemKind,
    schedule: Schedule,
    recurrence: Recurrence,
    event_type: Option<String>,
    team_assignment: Option<TeamAssignment>,
    source_event_id: Option<String>,
) -> ItemType {
    match kind {
        ItemKind::Simple => ItemType::Simple,
        ItemKind::Task => ItemType::Task {
            schedule,
            recurrence,
            team_assignment,
            source_event_id,
        },
        ItemKind::Event => ItemType::Event {
            schedule,
            recurrence,
            event_type,
        },
        ItemKind::Template => ItemType::Template {
            schedule,
            recurrence,
            event_type,
        },
    }
}

/// Project-scoped mirror of `service::items::top_level_anchor` — walks `item`'s
/// `parent_item_id` chain up to its true top-level ancestor via `repo.get_by_project`.
/// Replaced the old `team_id`-keyed `top_level_anchor_team` in Stage 4 of
/// docs/team-id-removal-plan.md, once `project_id` became this module's sole
/// scoping key.
pub(crate) async fn top_level_anchor_project(
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

/// Project-scoped mirror of `service::items::resolve_offset_anchor`.
pub(crate) async fn resolve_offset_anchor_project(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<DateTime<Utc>>, ItemError> {
    if let Some(event_id) = item.source_event_id() {
        let event = repo.get_by_project(project_id, &event_id).await?;
        Ok(item_anchor(&event))
    } else if let Some(parent_id) = item.parent_item_id.clone() {
        let parent = repo.get_by_project(project_id, &parent_id).await?;
        top_level_anchor_project(repo, project_id, &parent).await
    } else {
        Ok(None)
    }
}

/// Moved from `json_api::team_items::create_team_item`; rewritten in Stage 4 of
/// docs/team-id-removal-plan.md to be `project_id`-primary — every repo/membership
/// lookup below goes through `params.project_id`, not a `team_id`.
///
/// `team_id` is threaded in as a plain parameter (not a `CreateTeamItemParams`
/// field) purely to dual-write `items.team_id` on the created row: that column is
/// still read directly by a handful of not-yet-migrated call sites (see Stage 6 of
/// docs/team-id-removal-plan.md's "external-API read sites" — this dual-write gap
/// surfaced during Stage 4's implementation and was folded in here rather than left
/// as a silent regression window between Stage 4 and Stage 6). The caller
/// (`project_items::create_project_item`) already has this value from its own
/// `project.team_id` match before delegating here, so nothing extra is fetched.
pub async fn create_team_item(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    projects: &Arc<dyn ProjectRepo>,
    requester_user_id: &str,
    team_id: &str,
    params: CreateTeamItemParams,
) -> Result<String, ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    if let Some(ref r) = params.recurrence {
        recurrence::parse(r).map_err(ItemError::Invalid)?;
    }
    if params.recurrence.is_some()
        && (params.parent_item_id.is_some() || params.source_event_id.is_some())
    {
        return Err(ItemError::Invalid(
            "child or event-linked items cannot have their own recurrence; set dueOffsetDays instead"
                .to_string(),
        ));
    }
    if params.item_type == Some(ItemKind::Template) {
        return Err(ItemError::Invalid(
            "item_type Template is not supported for team items".to_string(),
        ));
    }
    if let (Some(start), Some(end)) = (params.scheduled_date, params.scheduled_end_date)
        && end < start
    {
        return Err(ItemError::Invalid(
            "scheduledEndDate cannot be before scheduledDate".to_string(),
        ));
    }

    // Events can never have children (a task references an event via
    // sourceEventId instead of nesting under it).
    if let Some(ref parent_id) = params.parent_item_id
        && let Ok(parent) = repo.get_by_project(&params.project_id, parent_id).await
        && parent.kind() == ItemKind::Event
    {
        return Err(ItemError::Invalid(
            "Events cannot have children; link a task to it via sourceEventId instead"
                .to_string(),
        ));
    }

    let kind = params.item_type.unwrap_or_default();

    let schedule = Schedule {
        due_date: params.due_date,
        has_due_time: params.has_due_time.unwrap_or(false),
        scheduled_date: params.scheduled_date,
        has_scheduled_time: params.has_scheduled_time.unwrap_or(false),
        scheduled_end_date: params.scheduled_end_date,
        has_end_time: params.has_end_time.unwrap_or(false),
    };
    let recurrence_data = Recurrence {
        pattern: params.recurrence.clone(),
        basis: params.recurrence_basis.clone(),
        due_offset_days: params.due_offset_days,
    };

    let team_assignment = if kind == ItemKind::Task {
        let assigned_to_user_id =
            resolve_project_assignee(projects, &params.project_id, params.assigned_to_user_id.clone())
                .await?;
        // Points authority is project-admin-only as of stage C1
        // (docs/project-abstraction-plan.md) — moved off `team_members.role` onto
        // `project_members.role`. A non-admin's requested value is silently
        // dropped rather than rejecting the whole create — the rest of the
        // request (name, dates, etc.) is still perfectly valid.
        let points = if params.points.is_some()
            && require_project_admin(projects, teams, &params.project_id, requester_user_id)
                .await
                .is_ok()
        {
            params.points
        } else {
            None
        };
        Some(TeamAssignment {
            assigned_to_user_id,
            points,
        })
    } else {
        None
    };

    let mut item = Item::new_project_item(&params.project_id, &params.name);
    // Dual-write — see this function's own doc comment above.
    item.team_id = Some(team_id.to_string());
    item.item_type = build_item_type(
        kind,
        schedule,
        recurrence_data,
        params.event_type.clone(),
        team_assignment,
        params.source_event_id.clone(),
    );
    item.complete = params.complete.unwrap_or(false);
    item.parent_item_id = params.parent_item_id.clone();
    item.description = params.description.clone();

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if item.is_offset_driven() {
        let anchor = resolve_offset_anchor_project(repo, &params.project_id, &item).await?;
        let new_due_date = anchor.and_then(|a| item.deadline_from_offset(a, tz_offset));
        if let Some(schedule) = item.item_type.schedule_mut() {
            schedule.due_date = new_due_date;
            schedule.has_due_time = false;
        }
    } else if let Some(pattern) = item.recurrence_pattern()
        && let Ok(rule) = recurrence::parse(&pattern)
    {
        let basis = item
            .recurrence_basis()
            .unwrap_or_else(|| "DUE_DATE".to_string());
        if let Some(schedule) = item.item_type.schedule_mut() {
            if basis == "DUE_DATE" && schedule.due_date.is_none() {
                let mut deadline = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
                if rule.time_override.is_none() {
                    deadline = recurrence::apply_end_of_day(deadline, tz_offset);
                } else {
                    schedule.has_due_time = true;
                }
                schedule.due_date = Some(deadline);
            } else if basis != "DUE_DATE" && schedule.scheduled_date.is_none() {
                let when = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
                if rule.time_override.is_some() {
                    schedule.has_scheduled_time = true;
                }
                schedule.scheduled_date = Some(when);
            }
        }
    }

    item.validate().map_err(ItemError::Invalid)?;

    let item_id = repo.create(&item).await?;

    // Considers both the requester's personal templates and this project's own —
    // same mechanism as service::items::create_item's trigger step. Lands as
    // sourceEventId-linked top-level tasks (copy_template_children_to_event), not
    // nested children — Events can never have children (see the parent-fetch check
    // above). Reads the project's templates via `list_templates_by_project` (Stage 1
    // of docs/team-id-removal-plan.md) rather than the old `team_id`-keyed
    // `list_team_templates`.
    if let Some(event_type) = item.event_type() {
        let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
        let root_date = item_anchor(&item);
        let mut templates = repo.list_templates(requester_user_id).await?;
        templates.extend(repo.list_templates_by_project(&params.project_id).await?);
        for tpl in templates
            .iter()
            .filter(|t| t.event_type().as_deref() == Some(event_type.as_str()))
        {
            copy_template_children_to_event(repo, &tpl.id, &item_id, root_date, tz_offset)
                .await?;
        }
    }
    Ok(item_id)
}

/// Moved from `json_api::team_items::delete_team_item`, with one behavior fix (same class as
/// `service::items::delete_item`'s): the original never confirmed `item_id` actually belongs
/// to `project_id` before deleting it. `repo.get_by_project` below closes that gap; a
/// mismatched pair now surfaces as `ItemError::NotFound`. Rewritten in Stage 4 of
/// docs/team-id-removal-plan.md to scope by `project_id` rather than `team_id` — unlike
/// `create_team_item`/`update_team_item`, delete never writes a row, so there's no
/// `items.team_id` dual-write concern here.
pub async fn delete_team_item(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    projects: &Arc<dyn ProjectRepo>,
    requester_user_id: &str,
    project_id: &str,
    item_id: &str,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    repo.get_by_project(project_id, item_id).await?;

    let mut queue = vec![item_id.to_string()];
    while let Some(parent_id) = queue.first().cloned() {
        queue.remove(0);
        let children = repo.list_children(&parent_id).await?;
        for child in children {
            queue.push(child.id.clone());
            repo.delete(&child.id).await?;
        }
    }
    unlink_source_event_tasks(repo, item_id).await?;
    repo.delete(item_id).await?;
    Ok(())
}

/// Moved from `json_api::team_items::require_active_member`. No longer called by
/// this file's own `create_team_item`/`update_team_item`/`delete_team_item` as of
/// Stage 4 of docs/team-id-removal-plan.md (they use `require_project_member`
/// instead), nor by `service::templates.rs`'s team-template functions as of that
/// plan's Stage 5 (same repointing) — kept because it's still the right check for the
/// genuinely `team_id`-keyed legacy surfaces that stay out of this plan's scope:
/// `service::activity_log::undo_activity_log_entry` (the legacy
/// `UndoActivityLogEntry` operation, permanently `team_id`-keyed — see Stage 6), and
/// the legacy `json_api::team_templates::list_team_templates`/`json_api::activity_log`
/// handlers (`json_api::team_templates::create_team_template` moved off it in Stage 5,
/// resolving its project via `ProjectRepo::get_by_team` instead).
pub(crate) async fn require_active_member(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    user_id: &str,
) -> Result<(), ItemError> {
    let status = teams
        .member_status(team_id, user_id)
        .await
        .map_err(|e| ItemError::Internal(format!("{e:?}")))?;
    match status.as_deref() {
        Some("ACTIVE") => Ok(()),
        Some(_) => Err(ItemError::Invalid("team invite not yet accepted".to_string())),
        None => Err(ItemError::Invalid(format!(
            "user id: {user_id} is not a member of team id: {team_id}"
        ))),
    }
}

#[derive(Debug, Default)]
pub struct UpdateTeamItemParams {
    pub project_id: String,
    pub item_id: String,
    pub name: String,
    pub description: Option<String>,
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
    pub item_type: Option<ItemKind>,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
    pub source_event_id: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub points: Option<i32>,
}

/// Moved from `json_api::team_items::update_team_item`; rewritten in Stage 4 of
/// docs/team-id-removal-plan.md to be `project_id`-primary — every repo/membership
/// lookup below goes through `params.project_id`, not a `team_id`.
///
/// `team_id` is threaded in as a plain parameter (not an `UpdateTeamItemParams`
/// field) for two narrow reasons, neither of which is repo scoping: (1)
/// `activity_log.log_activity`'s `team_id` column is still `NOT NULL` and out of
/// this plan's scope (see Stage 6 of docs/team-id-removal-plan.md); (2) unlike
/// `update_by_project`, `create`'s `INSERT` still writes `items.team_id`, but
/// `update_by_project` itself never touches that column at all — so, unlike
/// `create_team_item`, there is nothing here for `team_id` to dual-write into. The
/// caller (`project_items::update_project_item`) already has this value from its
/// own `projects.get(project_id)` call before delegating here, so nothing extra is
/// fetched.
pub async fn update_team_item(
    repo: &Arc<dyn ItemRepo>,
    ctx: &UpdateTeamItemContext,
    requester_user_id: &str,
    team_id: &str,
    params: UpdateTeamItemParams,
) -> Result<(), ItemError> {
    let teams = &ctx.teams;
    let projects = &ctx.projects;
    let activity_log = &ctx.activity_log;
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    if let Some(ref r) = params.recurrence {
        recurrence::parse(r).map_err(ItemError::Invalid)?;
    }
    if params.recurrence.is_some()
        && (params.parent_item_id.is_some() || params.source_event_id.is_some())
    {
        return Err(ItemError::Invalid(
            "child or event-linked items cannot have their own recurrence; set dueOffsetDays instead"
                .to_string(),
        ));
    }
    if params.item_type == Some(ItemKind::Template) {
        return Err(ItemError::Invalid(
            "item_type Template is not supported for team items".to_string(),
        ));
    }
    if let (Some(start), Some(end)) = (params.scheduled_date, params.scheduled_end_date)
        && end < start
    {
        return Err(ItemError::Invalid(
            "scheduledEndDate cannot be before scheduledDate".to_string(),
        ));
    }
    let current = repo.get_by_project(&params.project_id, &params.item_id).await?;

    if params.complete
        && !current.complete
        && has_incomplete_children(repo, &params.item_id).await?
    {
        return Err(ItemError::Invalid(
            "cannot complete an item with incomplete sub-items".to_string(),
        ));
    }

    // Events can never have children (a task references an event via
    // sourceEventId instead of nesting under it).
    if let Some(ref parent_id) = params.parent_item_id
        && let Ok(parent) = repo.get_by_project(&params.project_id, parent_id).await
        && parent.kind() == ItemKind::Event
    {
        return Err(ItemError::Invalid(
            "Events cannot have children; link a task to it via sourceEventId instead"
                .to_string(),
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
    let recurrence_data = Recurrence {
        pattern: params.recurrence.clone(),
        basis: params.recurrence_basis.clone(),
        due_offset_days: params.due_offset_days,
    };

    let team_assignment = if kind == ItemKind::Task {
        let assigned_to_user_id = if params.assigned_to_user_id == current.assigned_to_user_id() {
            current.assigned_to_user_id()
        } else {
            resolve_project_assignee(projects, &params.project_id, params.assigned_to_user_id.clone())
                .await?
        };
        // Points authority is project-admin-only as of stage C1
        // (docs/project-abstraction-plan.md) — moved off `team_members.role` onto
        // `project_members.role`. A non-admin's request simply can't change the
        // existing value — it's preserved as-is rather than erroring the rest of
        // the (otherwise valid) update. Unlike the old `current.project_id.as_deref()`
        // check this replaced, `params.project_id` is always known (it's the
        // primary key this whole update is scoped by), so there's no longer an
        // "unresolvable backing project" case to fall back on.
        let points = if require_project_admin(projects, teams, &params.project_id, requester_user_id)
            .await
            .is_ok()
        {
            params.points
        } else {
            current.points()
        };
        Some(TeamAssignment {
            assigned_to_user_id,
            points,
        })
    } else {
        None
    };

    let mut item = Item::new_project_item(&params.project_id, &params.name);
    item.id = params.item_id.clone();
    item.complete = params.complete;
    item.parent_item_id = params.parent_item_id.clone();
    item.description = params.description.clone();
    item.item_type = build_item_type(
        kind,
        schedule,
        recurrence_data,
        params.event_type.clone(),
        team_assignment,
        params.source_event_id.clone(),
    );

    let tz_offset = params.timezone_offset_minutes.unwrap_or(0);
    if item.is_offset_driven() {
        let anchor = resolve_offset_anchor_project(repo, &params.project_id, &item).await?;
        let new_due_date = anchor.and_then(|a| item.deadline_from_offset(a, tz_offset));
        if let Some(schedule) = item.item_type.schedule_mut() {
            schedule.due_date = new_due_date;
            schedule.has_due_time = false;
        }
    }

    item.validate().map_err(ItemError::Invalid)?;

    if current.complete && !is_pure_complete_toggle(&current, &item) {
        return Err(ItemError::Invalid(
            "cannot edit a completed item; un-complete it first".to_string(),
        ));
    }

    // Team completion validation — unconditional for every team item, not just
    // points-bearing ones (a real behavior change from before Stage 6; see
    // CLAUDE.md's Points plan). Checked against the just-resolved `item`, not
    // `current`, since a request can assign and complete in the same PUT.
    let just_completed = !current.complete && item.complete;
    let just_uncompleted = current.complete && !item.complete;
    if just_completed {
        match item.assigned_to_user_id() {
            None => {
                return Err(ItemError::Invalid(
                    "cannot complete an unassigned team item; assign it first".to_string(),
                ));
            }
            Some(assignee) if assignee != requester_user_id => {
                return Err(ItemError::Invalid(
                    "only the assigned user can complete this item".to_string(),
                ));
            }
            _ => {}
        }
    }

    // Points award/reversal must run against `current`'s captured identity, strictly
    // before the recurrence branch below deletes it and creates a successor under a
    // fresh id — otherwise a recurring item's completion would silently never award
    // anything (see CLAUDE.md's Points plan, Stage 6, and its cross-stage risk #2).
    if just_completed
        && item.parent_item_id.is_none()
        && let Some(points) = item.points()
    {
        // Guarded by the match above: a just-completed item always has an assignee.
        let assignee = item
            .assigned_to_user_id()
            .expect("just-completed team item must be assigned");
        activity_log
            .log_activity(
                team_id,
                item.project_id.as_deref(),
                &assignee,
                &current.id,
                &current.name,
                points,
            )
            .await?;
        // Points authority lives on `project_members` as of stage C1
        // (docs/project-abstraction-plan.md). As of Stage 4 of
        // docs/team-id-removal-plan.md, `params.project_id` is this update's own
        // primary key — always known, no `get_by_team` fallback needed (this is the
        // very bug docs/team-id-removal-plan.md exists to close: awarding against a
        // project id that can never go stale when a team is attached/detached).
        projects
            .add_project_points(&params.project_id, &assignee, points)
            .await?;
    }
    if just_uncompleted
        && let Some(assignee) = current.assigned_to_user_id()
        && let Some(entry) = activity_log
            .most_recent_unreversed(&current.id, &assignee)
            .await?
    {
        reverse_entry(projects, activity_log, &entry).await?;
    }

    if let Some((next_item, next_anchor)) = item.next_recurrence(chrono::Utc::now(), tz_offset) {
        let next_id = repo.create(&next_item).await?;
        clone_children(repo, &item.id, &next_id, next_anchor, tz_offset).await?;
        if item.kind() == ItemKind::Event {
            repoint_source_event_tasks(repo, &item.id, &next_id, next_anchor, tz_offset).await?;
        }
        archive_recurrence(&mut item);
        repo.update_by_project(&item).await?;
        return Ok(());
    }

    repo.update_by_project(&item).await?;
    let (old_anchor, new_anchor) = (item_anchor(&current), item_anchor(&item));
    if let Some(new_anchor) = new_anchor
        && Some(new_anchor) != old_anchor
    {
        sync_offset_children(repo, &item.id, new_anchor, tz_offset).await?;
        if item.kind() == ItemKind::Event {
            sync_source_event_tasks(repo, &item.id, new_anchor, tz_offset).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::activity_log::ActivityLogEntry;
    use crate::domain::team::TeamRole;
    use crate::storage::sqlite::{
        MockActivityLogRepo, MockItemRepo, MockProjectRepo, MockTeamRepo,
    };

    /// A `ProjectRepo` resolving `project_id` as a team-backed project (`team_id`)
    /// on which the caller holds `role`. As of Stage 4 of
    /// docs/team-id-removal-plan.md, `require_project_member`/`require_project_admin`
    /// are the sole membership/admin gates `create_team_item`/`update_team_item`
    /// call — both read `ProjectRepo` exclusively (`teams` is passed through but
    /// never actually queried by either), so every test below needs this instead of
    /// the old `active_member_teams`'s `TeamRepo` stub. Returned unwrapped so
    /// callers can chain on more expectations (e.g. `add_project_points`) before
    /// wrapping in `Arc`.
    fn project_with_role(project_id: &str, team_id: &str, role: TeamRole) -> MockProjectRepo {
        let mut mock = MockProjectRepo::new();
        {
            let pid = project_id.to_string();
            let tid = team_id.to_string();
            mock.expect_get().returning(move |_| {
                Ok(crate::domain::project::Project {
                    id: pid.clone(),
                    name: "Team Project".to_string(),
                    owner_user_id: "owner1".to_string(),
                    team_id: Some(tid.clone()),
                })
            });
        }
        mock.expect_member_role().returning(move |_, _| Ok(Some(role)));
        mock
    }

    fn ctx_with(teams: Arc<dyn TeamRepo>, projects: Arc<dyn ProjectRepo>) -> UpdateTeamItemContext {
        UpdateTeamItemContext {
            teams,
            projects,
            activity_log: Arc::new(MockActivityLogRepo::new()),
        }
    }

    #[tokio::test]
    async fn create_team_item_strips_points_for_non_admin() {
        let mut items = MockItemRepo::new();
        items
            .expect_create()
            .withf(|item: &Item| item.points().is_none())
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        create_team_item(
            &items,
            &teams,
            &projects,
            "member1",
            "t1",
            CreateTeamItemParams {
                project_id: "p1".to_string(),
                name: "Mow the lawn".to_string(),
                points: Some(50),
                ..Default::default()
            },
        )
        .await
        .expect("should create item");
    }

    #[tokio::test]
    async fn create_team_item_honors_points_for_admin() {
        let mut items = MockItemRepo::new();
        items
            .expect_create()
            .withf(|item: &Item| item.points() == Some(50))
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Admin));

        create_team_item(
            &items,
            &teams,
            &projects,
            "admin1",
            "t1",
            CreateTeamItemParams {
                project_id: "p1".to_string(),
                name: "Mow the lawn".to_string(),
                points: Some(50),
                ..Default::default()
            },
        )
        .await
        .expect("should create item");
    }

    fn team_item_with_points(id: &str, team_id: &str, points: Option<i32>) -> Item {
        team_item_with_points_and_assignee(id, team_id, points, None)
    }

    fn team_item_with_points_and_assignee(
        id: &str,
        team_id: &str,
        points: Option<i32>,
        assigned_to_user_id: Option<&str>,
    ) -> Item {
        Item {
            id: id.to_string(),
            team_id: Some(team_id.to_string()),
            name: "Mow the lawn".to_string(),
            item_type: ItemType::Task {
                schedule: Schedule::default(),
                recurrence: Recurrence::default(),
                team_assignment: Some(TeamAssignment {
                    points,
                    assigned_to_user_id: assigned_to_user_id.map(str::to_string),
                }),
                source_event_id: None,
            },
            ..Item::default()
        }
    }

    fn with_due_date_and_recurrence(mut item: Item, due_date: DateTime<Utc>, pattern: &str) -> Item {
        if let Some(schedule) = item.item_type.schedule_mut() {
            schedule.due_date = Some(due_date);
        }
        if let Some(recurrence) = item.item_type.recurrence_mut() {
            recurrence.pattern = Some(pattern.to_string());
        }
        item
    }

    #[tokio::test]
    async fn update_team_item_preserves_existing_points_for_non_admin() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                project_id: Some("p1".to_string()),
                ..team_item_with_points("item1", "t1", Some(30))
            })
        });
        items
            .expect_update_by_project()
            .withf(|item: &Item| item.points() == Some(30))
            .times(1)
            .returning(|_| Ok(()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: false,
                points: Some(999),
                ..Default::default()
            },
        )
        .await
        .expect("should update item");
    }

    #[tokio::test]
    async fn update_team_item_honors_new_points_for_admin() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                project_id: Some("p1".to_string()),
                ..team_item_with_points("item1", "t1", Some(30))
            })
        });
        items
            .expect_update_by_project()
            .withf(|item: &Item| item.points() == Some(999))
            .times(1)
            .returning(|_| Ok(()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Admin));

        update_team_item(
            &items,
            &ctx_with(teams, projects),
            "admin1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: false,
                points: Some(999),
                ..Default::default()
            },
        )
        .await
        .expect("should update item");
    }

    #[tokio::test]
    async fn update_team_item_rejects_completion_with_incomplete_child() {
        let mut items = MockItemRepo::new();
        items
            .expect_get_by_project()
            .returning(|_, _| Ok(team_item_with_points("item1", "t1", None)));
        items
            .expect_list_children()
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

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        let err = update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: true,
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject completing with an incomplete child");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_team_item_allows_completion_when_all_children_complete() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(team_item_with_points_and_assignee(
                "item1",
                "t1",
                None,
                Some("member1"),
            ))
        });
        items
            .expect_list_children()
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
        items
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: true,
                assigned_to_user_id: Some("member1".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("should allow completion when all children are complete");
    }

    #[tokio::test]
    async fn update_team_item_rejects_field_edit_on_completed_item() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                team_id: Some("t1".to_string()),
                project_id: Some("p1".to_string()),
                name: "Original name".to_string(),
                complete: true,
                ..Item::default()
            })
        });

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        let err = update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
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
    async fn update_team_item_allows_pure_complete_toggle() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                team_id: Some("t1".to_string()),
                project_id: Some("p1".to_string()),
                name: "Same name".to_string(),
                complete: true,
                ..Item::default()
            })
        });
        items
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
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
    async fn update_team_item_rejects_completion_of_unassigned_item() {
        let mut items = MockItemRepo::new();
        items
            .expect_get_by_project()
            .returning(|_, _| Ok(team_item_with_points("item1", "t1", Some(20))));
        items.expect_list_children().returning(|_| Ok(vec![]));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        let err = update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: true,
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject completing an unassigned team item");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_team_item_rejects_completion_by_non_assignee() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(team_item_with_points_and_assignee(
                "item1",
                "t1",
                Some(20),
                Some("member1"),
            ))
        });
        items.expect_list_children().returning(|_| Ok(vec![]));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        let err = update_team_item(
            &items,
            &ctx_with(teams, projects),
            "someone-else",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: true,
                assigned_to_user_id: Some("member1".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("should reject completion attempted by someone other than the assignee");

        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_team_item_awards_points_on_genuine_completion() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(team_item_with_points_and_assignee(
                "item1",
                "t1",
                Some(20),
                Some("member1"),
            ))
        });
        items
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));
        items.expect_list_children().returning(|_| Ok(vec![]));

        let mut activity_log = MockActivityLogRepo::new();
        activity_log
            .expect_log_activity()
            .withf(|team_id, _project_id, user_id, item_id, item_name, points_delta| {
                team_id == "t1"
                    && user_id == "member1"
                    && item_id == "item1"
                    && item_name == "Mow the lawn"
                    && *points_delta == 20
            })
            .times(1)
            .returning(|_, _, _, _, _, _| Ok("entry1".to_string()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        // Points award straight to `params.project_id` ("p1") — no more
        // `get_by_team` fallback needed now that `project_id` is this module's
        // primary key (Stage 4 of docs/team-id-removal-plan.md fixes the very bug
        // this plan exists to close).
        let mut projects_mock = project_with_role("p1", "t1", TeamRole::Member);
        projects_mock
            .expect_add_project_points()
            .withf(|project_id, user_id, delta| {
                project_id == "p1" && user_id == "member1" && *delta == 20
            })
            .times(1)
            .returning(|_, _, _| Ok(20));

        update_team_item(
            &items,
            &UpdateTeamItemContext {
                teams,
                projects: Arc::new(projects_mock),
                activity_log: Arc::new(activity_log),
            },
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: true,
                assigned_to_user_id: Some("member1".to_string()),
                points: Some(20),
                ..Default::default()
            },
        )
        .await
        .expect("should award points on a genuine completion");
    }

    #[tokio::test]
    async fn update_team_item_does_not_double_award_on_no_op_resubmit() {
        // current.complete is already true, and the request also sends complete:
        // true — no false->true transition happens, so `just_completed` never
        // fires. `ctx_with`'s `activity_log` has no expectations set at all, so
        // this test doubles as an assertion that log_activity/add_project_points
        // are never called on this path.
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                complete: true,
                ..team_item_with_points_and_assignee("item1", "t1", Some(20), Some("member1"))
            })
        });
        items
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: true,
                assigned_to_user_id: Some("member1".to_string()),
                points: Some(20),
                ..Default::default()
            },
        )
        .await
        .expect("no-op resubmit of an already-complete item should succeed with no award");
    }

    #[tokio::test]
    async fn update_team_item_reversal_reads_logged_delta_not_items_current_points() {
        // The item's own `points` has since been changed by an admin to 999, but the
        // activity log entry recorded at completion time still says 20 — reversal
        // must claw back 20, not 999 (see CLAUDE.md's Points plan, Stage 6).
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                complete: true,
                ..team_item_with_points_and_assignee("item1", "t1", Some(999), Some("member1"))
            })
        });
        items
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));

        let mut activity_log = MockActivityLogRepo::new();
        activity_log
            .expect_most_recent_unreversed()
            .withf(|item_id, user_id| item_id == "item1" && user_id == "member1")
            .times(1)
            .returning(|_, _| {
                Ok(Some(ActivityLogEntry {
                    id: "entry1".to_string(),
                    team_id: "t1".to_string(),
                    project_id: Some("p1".to_string()),
                    user_id: "member1".to_string(),
                    item_id: "item1".to_string(),
                    item_name: "Mow the lawn".to_string(),
                    points_delta: 20,
                    reversed: false,
                    created_at: chrono::Utc::now(),
                }))
            });
        activity_log
            .expect_mark_reversed()
            .withf(|id| id == "entry1")
            .times(1)
            .returning(|_| Ok(()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        // The logged entry already carries `project_id: Some("p1")`, so reversal
        // reads it straight off the entry — no `get_by_team` fallback needed here.
        let mut projects_mock = project_with_role("p1", "t1", TeamRole::Member);
        projects_mock
            .expect_add_project_points()
            .withf(|project_id, user_id, delta| {
                project_id == "p1" && user_id == "member1" && *delta == -20
            })
            .times(1)
            .returning(|_, _, _| Ok(0));

        update_team_item(
            &items,
            &UpdateTeamItemContext {
                teams,
                projects: Arc::new(projects_mock),
                activity_log: Arc::new(activity_log),
            },
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: false,
                assigned_to_user_id: Some("member1".to_string()),
                points: Some(999),
                ..Default::default()
            },
        )
        .await
        .expect("should reverse using the logged delta");
    }

    #[tokio::test]
    async fn update_team_item_uncomplete_with_no_log_entry_is_a_silent_no_op() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(team_item_with_points_and_assignee(
                "item1",
                "t1",
                None,
                Some("member1"),
            ))
        });
        items
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));

        let mut activity_log = MockActivityLogRepo::new();
        activity_log
            .expect_most_recent_unreversed()
            .returning(|_, _| Ok(None));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        update_team_item(
            &items,
            &UpdateTeamItemContext {
                teams,
                projects: Arc::new(project_with_role("p1", "t1", TeamRole::Member)),
                activity_log: Arc::new(activity_log),
            },
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: false,
                assigned_to_user_id: Some("member1".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("uncompleting an item with no logged points should silently no-op");
    }

    #[tokio::test]
    async fn update_team_item_awards_points_correctly_even_when_recurrence_also_fires() {
        let due_date = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(move |_, _| {
            Ok(with_due_date_and_recurrence(
                team_item_with_points_and_assignee("item1", "t1", Some(20), Some("member1")),
                due_date,
                "every day",
            ))
        });
        // Called once by the parent-gating check (has_incomplete_children), once
        // more by clone_children's own recursive walk over the (empty) subtree —
        // same double-call shape Stage 5's analogous personal-item test hit.
        items
            .expect_list_children()
            .withf(|parent_id: &str| parent_id == "item1")
            .times(2)
            .returning(|_| Ok(vec![]));
        items
            .expect_create()
            .times(1)
            .returning(|_| Ok("item1-next".to_string()));
        // The just-completed occurrence is kept as history (not deleted) with its own
        // recurrence config stripped so it can't independently re-fire.
        items
            .expect_update_by_project()
            .withf(|item: &Item| {
                item.id == "item1"
                    && item.complete
                    && item.recurrence_pattern().is_none()
                    && item.recurrence_basis().is_none()
            })
            .times(1)
            .returning(|_| Ok(()));

        let mut activity_log = MockActivityLogRepo::new();
        activity_log
            .expect_log_activity()
            .withf(|team_id, _project_id, user_id, item_id, _item_name, points_delta| {
                // Must be logged against the *old* item's id, never the
                // not-yet-created successor's.
                team_id == "t1" && user_id == "member1" && item_id == "item1" && *points_delta == 20
            })
            .times(1)
            .returning(|_, _, _, _, _, _| Ok("entry1".to_string()));

        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        // Same no-fallback-needed shape as
        // `update_team_item_awards_points_on_genuine_completion` above.
        let mut projects_mock = project_with_role("p1", "t1", TeamRole::Member);
        projects_mock
            .expect_add_project_points()
            .withf(|project_id, user_id, delta| {
                project_id == "p1" && user_id == "member1" && *delta == 20
            })
            .times(1)
            .returning(|_, _, _| Ok(20));

        update_team_item(
            &items,
            &UpdateTeamItemContext {
                teams,
                projects: Arc::new(projects_mock),
                activity_log: Arc::new(activity_log),
            },
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Mow the lawn".to_string(),
                complete: true,
                recurrence: Some("every day".to_string()),
                assigned_to_user_id: Some("member1".to_string()),
                points: Some(20),
                ..Default::default()
            },
        )
        .await
        .expect("should award points even though recurrence also fires");
    }

    #[tokio::test]
    async fn create_team_item_dual_writes_team_id_from_threaded_param() {
        // Stage 4 of docs/team-id-removal-plan.md made `project_id` (not `team_id`)
        // the primary key `create_team_item` builds items against — `project_id`
        // comes straight from `CreateTeamItemParams` now, not resolved via
        // `get_by_team`. `team_id` is still dual-written onto the created row (via
        // the explicit `team_id` parameter) purely to keep `items.team_id`
        // populated for the Stage 6 read sites that haven't migrated off it yet
        // (see this function's own doc comment).
        let mut items = MockItemRepo::new();
        items
            .expect_create()
            .withf(|item: &Item| {
                item.project_id.as_deref() == Some("p1") && item.team_id.as_deref() == Some("t1")
            })
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        let items: Arc<dyn ItemRepo> = Arc::new(items);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        create_team_item(
            &items,
            &teams,
            &projects,
            "member1",
            "t1",
            CreateTeamItemParams {
                project_id: "p1".to_string(),
                name: "Mow the lawn".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create team item");
    }

    #[tokio::test]
    async fn update_team_item_uses_params_project_id() {
        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "item1".to_string(),
                team_id: Some("t1".to_string()),
                project_id: Some("p1".to_string()),
                name: "Same name".to_string(),
                ..Item::default()
            })
        });
        items
            .expect_update_by_project()
            .withf(|item: &Item| item.project_id.as_deref() == Some("p1"))
            .times(1)
            .returning(|_| Ok(()));
        let items: Arc<dyn ItemRepo> = Arc::new(items);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> =
            Arc::new(project_with_role("p1", "t1", TeamRole::Member));

        update_team_item(
            &items,
            &ctx_with(teams, projects),
            "member1",
            "t1",
            UpdateTeamItemParams {
                project_id: "p1".to_string(),
                item_id: "item1".to_string(),
                name: "Renamed".to_string(),
                complete: false,
                ..Default::default()
            },
        )
        .await
        .expect("should update using params.project_id");
    }
}
