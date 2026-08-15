use crate::domain::item::{Item, ItemKind};
use crate::domain::item_series::ItemSeries;
use crate::domain::recurrence;
use crate::service::error::ItemError;
use crate::service::project_items::{self, CreateProjectItemParams};
use crate::service::projects::{require_project_admin, require_project_member, resolve_project_assignee};
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Arc;

/// Stage 3 of docs/recurring-events-virtual-occurrences-rough-plan.md's staged
/// breakdown. Returns the already-materialized `Item` for `(series_id,
/// occurrence_date)` if one exists, otherwise creates it (via the existing
/// `project_items::create_project_item` — not a hand-rolled personal/team dispatch
/// of its own) and records the mapping so future calls hit the cache-read branch.
/// This is what a caller resolving a virtual occurrence into something addressable
/// (a detail page, an edit, a `sourceEventId` link) calls into; it does not run on
/// every read of a series, only when a specific occurrence is actually touched.
///
/// Stage 10 gap 3: if `series.template_item_id` is set, the newly materialized
/// occurrence's children are copied from that template via the existing
/// `service::items::copy_template_children` — the same function
/// `project_templates/`'s "Use" flow and the event-trigger mechanism already call, so
/// this reuses proven, non-destructive subtree-copy logic rather than adding new. Only
/// runs on the "create new" branch — an already-materialized occurrence already has
/// whatever children it was given the first time.
pub async fn get_or_materialize_occurrence(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<Item, ItemError> {
    let series = event_series.get_series(series_id).await?;
    let existing = event_series.get_occurrence(series_id, occurrence_date).await?;

    if let Some(occurrence) = existing {
        if let Some(item_id) = occurrence.item_id {
            return project_items::get_project_item(
                repo,
                projects,
                teams,
                &series.project_id,
                requester_user_id,
                &item_id,
            )
            .await;
        }
    }

    let item_id = project_items::create_project_item(
        repo,
        projects,
        teams,
        requester_user_id,
        CreateProjectItemParams {
            project_id: series.project_id.clone(),
            name: series.name.clone(),
            description: series.description.clone(),
            item_type: Some(series.item_type),
            event_type: series.event_type.clone(),
            scheduled_date: Some(occurrence_date),
            has_scheduled_time: Some(true),
            // Only ever Some on a Task-typed series on a team-backed project —
            // resolve_series_assignment already enforced that at create/update time,
            // so this is a plain carry-forward, not a re-validation.
            assigned_to_user_id: series.assigned_to_user_id.clone(),
            points: series.points,
            ..Default::default()
        },
    )
    .await?;

    event_series
        .record_materialized_occurrence(series_id, occurrence_date, &item_id)
        .await?;

    if let Some(template_id) = &series.template_item_id {
        crate::service::items::copy_template_children(
            repo,
            template_id,
            &item_id,
            Some(occurrence_date),
            tz_offset_minutes,
        )
        .await?;
    }

    project_items::get_project_item_unchecked(repo, &series.project_id, &item_id).await
}

/// Marks `occurrence_date` as skipped (the EXDATE-equivalent) for `series_id`. This is
/// the web UI's explicit "Skip" action, wired only onto genuinely virtual occurrences
/// (see `list_virtual_occurrences_for_project_unchecked` — the only source the skip
/// button's URL is ever built from), so `occurrence_date` never already has a
/// materialized `item_id` behind it in practice; deleting a *materialized* occurrence's
/// item goes through `unlink_deleted_item_occurrence` below instead, called from the
/// item's own delete path. `mark_exdate` clears `item_id` unconditionally, so even a
/// (deliberately unhandled) direct call against an already-materialized date would just
/// orphan that item rather than corrupt the occurrence row.
///
/// Stage 10a: rejects (via `require_current_occurrence`) skipping anything but a
/// Task-typed series' current occurrence, before either write below runs — see that
/// function's doc comment for why this replaces Stage 9's forward-jumping behavior.
pub async fn skip_occurrence(
    event_series: &Arc<dyn ItemSeriesRepo>,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    let series = event_series.get_series(series_id).await?;
    require_current_occurrence(&series, occurrence_date, tz_offset_minutes)?;
    event_series.mark_exdate(series_id, occurrence_date).await?;
    // Stage 9: skipping settles the occurrence exactly like completing one does — see
    // record_task_completion's doc comment below for why this is symmetric. Meaningless
    // for an Event-typed series (no completion/cursor concept), so left untouched there.
    // Stage 10 gap 1: cursor_value_for_settlement uses Utc::now() here too, for a
    // completion-basis series — the same symmetry.
    if series.item_type == ItemKind::Task {
        let cursor_value = cursor_value_for_settlement(&series, occurrence_date);
        event_series.advance_cursor(series_id, cursor_value).await?;
    }
    Ok(())
}

/// Stage 10a: rejects settling (completing or skipping) anything but a Task-typed
/// series' current occurrence — see `docs/recurring-events-virtual-occurrences-rough-plan.md`'s
/// Stage 10 planning notes, cross-cutting decision. Reverses Stage 9's shipped
/// behavior (commit `652724c`), which let the cursor forward-jump to whatever
/// occurrence was completed/skipped, in any order; occurrences now settle strictly
/// one at a time, in order, via `current_occurrence_date`'s cursor-derived value.
/// "Current" can validly be in the future, present, or past — only settling
/// something *beyond* current is disallowed. Always `Ok` for an Event-typed series
/// (no cursor/current concept, unchanged from today).
fn require_current_occurrence(
    series: &ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    if series.item_type != ItemKind::Task {
        return Ok(());
    }
    let rule = recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
    let current = current_occurrence_date(series, &rule, tz_offset_minutes);
    if occurrence_date != current {
        return Err(ItemError::Invalid(format!(
            "cannot settle this occurrence out of order — the series' current \
             occurrence is {current}; occurrences must be completed or skipped \
             one at a time, in order"
        )));
    }
    Ok(())
}

/// Stage 10a: the Complete-side counterpart to `require_current_occurrence`, called
/// from `project_items::update_project_item` *before* it persists a `complete: true`
/// request — unlike `record_task_completion` below (a post-persistence cursor-advance
/// hook), this one can actually reject the request outright, so it has to run first.
/// Cheap no-op for the overwhelmingly common case (item never came from a series),
/// same shape as `record_task_completion`/`unlink_deleted_item_occurrence`.
pub async fn validate_completable(
    event_series: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    if let Some(occurrence) = event_series.find_occurrence_by_item_id(item_id).await? {
        let series = event_series.get_series(&occurrence.series_id).await?;
        require_current_occurrence(&series, occurrence.occurrence_date, tz_offset_minutes)?;
    }
    Ok(())
}

/// Stage 6's resolution of stage 3's deferred "what happens to a materialized
/// occurrence's item when it's skipped" question: skipping a materialized occurrence
/// deletes its `items` row (via `project_items::delete_project_item`, the same shared
/// delete path every item goes through — CLI/MCP/web alike) and *then* marks the
/// occurrence exdate, so it neither reappears as virtual nor keeps pointing at a
/// deleted item.
///
/// Called from `project_items::delete_project_item` itself, after every item delete,
/// not from a series-specific route — a materialized occurrence's item has no visible
/// marker distinguishing it from an ordinary Event, so "delete this item" (wherever
/// that action already lives) is the only skip affordance a materialized occurrence
/// gets in this stage; see docs/recurring-events-virtual-occurrences-rough-plan.md's
/// stage 6 write-up for why a dedicated materialized-occurrence "Skip" button was
/// deliberately left out of scope. A `None` result (the overwhelmingly common case —
/// most deleted items never came from a series) is a normal, cheap no-op.
pub async fn unlink_deleted_item_occurrence(
    event_series: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
) -> Result<(), ItemError> {
    if let Some(occurrence) = event_series.find_occurrence_by_item_id(item_id).await? {
        event_series
            .mark_exdate(&occurrence.series_id, occurrence.occurrence_date)
            .await?;
    }
    Ok(())
}

/// Stage 10 gap 1: whether `series` measures its next occurrence from *actual
/// settlement time* rather than the fixed schedule — see `ItemSeries::basis`'s doc
/// comment. A plain literal-string check, following the `Item::recurrence_basis`
/// precedent CLAUDE.md documents (`ItemType` is the deliberate exception to that
/// norm, not this).
pub fn is_completion_basis(series: &ItemSeries) -> bool {
    series.basis.as_deref() == Some("COMPLETION")
}

/// Stage 10 gap 1: the date to advance a Task-typed series' cursor to when settling
/// (completing or skipping) `occurrence_date` — `Utc::now()` for a completion-basis
/// series (measuring the next occurrence from when it was actually settled, not its
/// nominal date), otherwise `occurrence_date` itself (today's only behavior,
/// unchanged). Shared by `record_task_completion` and `skip_occurrence` so Complete
/// and Skip stay symmetric, matching Stage 9's existing "settling is settling" design.
fn cursor_value_for_settlement(series: &ItemSeries, occurrence_date: DateTime<Utc>) -> DateTime<Utc> {
    if is_completion_basis(series) {
        Utc::now()
    } else {
        occurrence_date
    }
}

/// Stage 9: called after `service::project_items::update_project_item` successfully
/// transitions an item to `complete: true`. A cheap no-op for the overwhelmingly common
/// case (the item never came from a series, or came from an Event-typed one, which has
/// no completion/cursor concept) — same shape as `unlink_deleted_item_occurrence`.
///
/// Stage 10a: by the time this runs, `validate_completable` has already rejected any
/// attempt to complete anything but the series' current occurrence, so this always
/// advances the cursor by exactly one step — `advance_cursor`'s own forward-only max
/// is now a pure idempotency guard against a redundant call, not something resolving a
/// genuine out-of-order jump (that possibility no longer exists).
///
/// Stage 10 gap 1: for a completion-basis series, advances the cursor to `Utc::now()`
/// (via `cursor_value_for_settlement`) instead of the occurrence's nominal date.
pub async fn record_task_completion(
    event_series: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
) -> Result<(), ItemError> {
    if let Some(occurrence) = event_series.find_occurrence_by_item_id(item_id).await? {
        let series = event_series.get_series(&occurrence.series_id).await?;
        if series.item_type == ItemKind::Task {
            let cursor_value = cursor_value_for_settlement(&series, occurrence.occurrence_date);
            event_series
                .advance_cursor(&occurrence.series_id, cursor_value)
                .await?;
        }
    }
    Ok(())
}

/// Stage 9: the single "next occurrence to work on" date for a Task-typed series —
/// tracked via `cursor_date` rather than derived by scanning every past occurrence back
/// to the anchor, which doesn't scale for an old, faithfully-completed series. Unlike
/// `recurrence::next_date` (which the legacy single-row mechanism still uses), this can
/// legitimately land in the past: a backlogged series' current occurrence stays exactly
/// there until it's explicitly completed or skipped, one step at a time. A fresh series
/// (`cursor_date: None`) starts at its own `anchor_date` — the very first occurrence,
/// not one step past it.
///
/// `rule` is taken pre-parsed rather than re-parsing `series.recurrence` here, since
/// every caller already has it (a series with an unparseable `recurrence` has no
/// well-defined current occurrence at all — callers skip such series entirely rather
/// than calling this).
pub fn current_occurrence_date(
    series: &ItemSeries,
    rule: &recurrence::RecurrenceRule,
    tz_offset_minutes: i32,
) -> DateTime<Utc> {
    match series.cursor_date {
        Some(cursor) => recurrence::advance_once(rule, cursor, tz_offset_minutes),
        None => series.anchor_date,
    }
}

/// Stage 4a's plain CRUD passthroughs, gated by project *membership* (not admin) —
/// a series is project-scoped content like a template, not a role/points-authority
/// action, so it follows `create_project_template`'s auth level rather than
/// `update_project`'s.
#[derive(Debug, Default)]
pub struct CreateItemSeriesParams {
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    pub recurrence: String,
    pub anchor_date: DateTime<Utc>,
    pub item_type: ItemKind,
    pub basis: Option<String>,
    pub template_item_id: Option<String>,
    pub assigned_to_user_id: Option<String>,
    pub points: Option<i32>,
}

/// Stage 7b: a series can only ever materialize Task or Event occurrences —
/// mirrors the recurrence+parentItemId rejection precedent in `service::items`.
fn validate_series_item_type(item_type: ItemKind) -> Result<(), ItemError> {
    if item_type != ItemKind::Task && item_type != ItemKind::Event {
        return Err(ItemError::Invalid(
            "series item_type must be TASK or EVENT".to_string(),
        ));
    }
    Ok(())
}

/// Stage 10 gap 1: `basis: Some("COMPLETION")` is only valid on a `Task`-typed series
/// (Event-typed series have no completion/cursor concept — see `ItemSeries::basis`'s
/// doc comment), and only for "every N days/weeks/months/years" `recurrence` patterns —
/// a fixed weekday or day-of-month has no well-defined "N units after actual
/// completion" interpretation. `recurrence` is re-parsed here rather than threaded in
/// pre-parsed, since `create_series`/`update_series` don't otherwise need a parsed
/// `RecurrenceRule` for anything else.
fn validate_series_basis(item_type: ItemKind, basis: &Option<String>, recurrence: &str) -> Result<(), ItemError> {
    if basis.as_deref() != Some("COMPLETION") {
        return Ok(());
    }
    if item_type != ItemKind::Task {
        return Err(ItemError::Invalid(
            "basis: COMPLETION is only valid on a TASK series".to_string(),
        ));
    }
    let rule = recurrence::parse(recurrence).map_err(ItemError::Invalid)?;
    if !matches!(
        rule.unit,
        recurrence::RecurrenceUnit::Days
            | recurrence::RecurrenceUnit::Weeks
            | recurrence::RecurrenceUnit::Months
            | recurrence::RecurrenceUnit::Years
    ) {
        return Err(ItemError::Invalid(
            "basis: COMPLETION is only valid for \"every N days/weeks/months/years\" \
             patterns, not a fixed weekday or day-of-month"
                .to_string(),
        ));
    }
    Ok(())
}

/// Stage 7c originally let `event_type` through on an `Event`-typed series (rejecting it
/// only on a `Task` series, whose materialized `Item` has no `event_type` slot to begin
/// with — `ItemType::Task` carries no such field, see `domain::item::build_item_type`/
/// `ItemType`). As of 2026-08-15, `event_type` is unconditionally unsupported on *any*
/// series, Task or Event: `get_or_materialize_occurrence` routes through the same
/// `create_project_item` call path the legacy per-creation template-trigger mechanism
/// (CLAUDE.md's Events section — matching `event_type` to auto-copy a template's children
/// onto a newly created `Event`) hooks into, so an `Event`-typed series with `event_type`
/// set would refire that legacy trigger on *every* materialization, independently of and
/// potentially conflicting with the series' own child-carry-forward design (Stage 10 gap
/// 3, see `docs/recurring-events-virtual-occurrences-rough-plan.md`). A `Task` series
/// already covers "I want this to recur" without that indirection. Deliberately left as a
/// plain unconditional rejection rather than removed from the model entirely — revisiting
/// it (e.g. once gap 3 lands, or if these two mechanisms are made to cooperate on purpose)
/// only means loosening this one check, not a schema change.
fn validate_series_event_type(event_type: &Option<String>) -> Result<(), ItemError> {
    if event_type.is_some() {
        return Err(ItemError::Invalid(
            "event_type is not currently supported on an item series".to_string(),
        ));
    }
    Ok(())
}

/// Stage 10 gap 3: `template_item_id` is only ever settable on a `Task`-typed series —
/// `copy_template_children` (called from `get_or_materialize_occurrence` when this is
/// set) creates children via `repo.create()` directly, bypassing `create_item`'s own
/// "Events cannot have children" check, so allowing this on an `Event` series would let
/// materialization silently create orphaned children nested under an Event. When set,
/// the referenced item must exist in the same project and actually be a `Template` —
/// mirrors `validate_series_basis`'s "reject early, at the input boundary" precedent.
async fn validate_series_template_item(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item_type: ItemKind,
    template_item_id: &Option<String>,
) -> Result<(), ItemError> {
    let Some(id) = template_item_id else {
        return Ok(());
    };
    if item_type != ItemKind::Task {
        return Err(ItemError::Invalid(
            "template_item_id is only valid on a TASK series".to_string(),
        ));
    }
    let item = repo.get_by_project(project_id, id).await.map_err(ItemError::from)?;
    if item.kind() != ItemKind::Template {
        return Err(ItemError::Invalid(
            "template_item_id must reference a Template item".to_string(),
        ));
    }
    Ok(())
}

/// Points/assignment are only meaningful on a `Task`-typed series on a team-backed
/// project — mirrors `TeamAssignment`'s item-level restriction (CLAUDE.md's Points
/// section: personal-project items never carry a `TeamAssignment` at all; an
/// `Event`-typed item has no such concept either). Neither field ever applies to an
/// `Event` series, matching `validate_series_template_item`'s "reject early, at the
/// input boundary" precedent, rather than silently dropping — a caller explicitly
/// requesting assignment/points on the wrong kind of series is almost certainly a
/// mistake worth surfacing, not a value worth quietly discarding.
///
/// `points`, once past that boundary check, follows `create_team_item`'s existing
/// authority convention instead: settable only by that project's admin, with a
/// non-admin's requested value silently dropped rather than rejecting the whole
/// request (name/recurrence/etc. are still perfectly valid on their own).
/// `assigned_to_user_id` has no such authority gate — any project member may set who
/// a series' occurrences go to — but is validated via `resolve_project_assignee` the
/// same way an item's own `assignedToUserId` is, so it must actually be a member of
/// the series' project.
async fn resolve_series_assignment(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    item_type: ItemKind,
    assigned_to_user_id: Option<String>,
    points: Option<i32>,
) -> Result<(Option<String>, Option<i32>), ItemError> {
    if assigned_to_user_id.is_none() && points.is_none() {
        return Ok((None, None));
    }
    if item_type != ItemKind::Task {
        return Err(ItemError::Invalid(
            "assignedToUserId/points are only valid on a TASK series".to_string(),
        ));
    }
    let project = projects.get(project_id).await?;
    if project.team_id.is_none() {
        return Err(ItemError::Invalid(
            "assignedToUserId/points require a team-backed project".to_string(),
        ));
    }
    let resolved_assignee = resolve_project_assignee(projects, project_id, assigned_to_user_id).await?;
    let resolved_points = if points.is_some()
        && require_project_admin(projects, teams, project_id, requester_user_id)
            .await
            .is_ok()
    {
        points
    } else {
        None
    };
    Ok((resolved_assignee, resolved_points))
}

pub async fn create_series(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    params: CreateItemSeriesParams,
) -> Result<String, ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    validate_series_item_type(params.item_type)?;
    validate_series_event_type(&params.event_type)?;
    validate_series_basis(params.item_type, &params.basis, &params.recurrence)?;
    validate_series_template_item(
        repo,
        &params.project_id,
        params.item_type,
        &params.template_item_id,
    )
    .await?;
    let (assigned_to_user_id, points) = resolve_series_assignment(
        projects,
        teams,
        &params.project_id,
        requester_user_id,
        params.item_type,
        params.assigned_to_user_id,
        params.points,
    )
    .await?;
    Ok(event_series
        .create_series(&ItemSeries {
            id: String::new(),
            project_id: params.project_id,
            name: params.name,
            description: params.description,
            event_type: params.event_type,
            recurrence: params.recurrence,
            anchor_date: params.anchor_date,
            item_type: params.item_type,
            // A new series has never settled an occurrence yet — its "current" one is
            // its own anchor_date (see current_occurrence_date below).
            cursor_date: None,
            basis: params.basis,
            template_item_id: params.template_item_id,
            assigned_to_user_id,
            points,
        })
        .await?)
}

pub async fn get_series(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
) -> Result<ItemSeries, ItemError> {
    let series = event_series.get_series(series_id).await?;
    require_project_member(projects, teams, &series.project_id, requester_user_id).await?;
    Ok(series)
}

#[derive(Debug, Default)]
pub struct UpdateItemSeriesParams {
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    pub recurrence: String,
    pub anchor_date: DateTime<Utc>,
    pub item_type: ItemKind,
    pub basis: Option<String>,
    pub template_item_id: Option<String>,
    pub assigned_to_user_id: Option<String>,
    pub points: Option<i32>,
}

pub async fn update_series(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
    params: UpdateItemSeriesParams,
) -> Result<(), ItemError> {
    let current = event_series.get_series(series_id).await?;
    require_project_member(projects, teams, &current.project_id, requester_user_id).await?;
    validate_series_item_type(params.item_type)?;
    validate_series_event_type(&params.event_type)?;
    validate_series_basis(params.item_type, &params.basis, &params.recurrence)?;
    validate_series_template_item(
        repo,
        &current.project_id,
        params.item_type,
        &params.template_item_id,
    )
    .await?;
    let (assigned_to_user_id, points) = resolve_series_assignment(
        projects,
        teams,
        &current.project_id,
        requester_user_id,
        params.item_type,
        params.assigned_to_user_id,
        params.points,
    )
    .await?;
    event_series
        .update_series(
            series_id,
            &ItemSeries {
                id: series_id.to_string(),
                project_id: current.project_id,
                name: params.name,
                description: params.description,
                event_type: params.event_type,
                recurrence: params.recurrence,
                anchor_date: params.anchor_date,
                // itemType joins the rest of this endpoint's full-replace fields at
                // stage 7b — no longer carried over from `current`.
                item_type: params.item_type,
                // Not a settable field of this endpoint — cursor_date only ever moves via
                // ItemSeriesRepo::advance_cursor, and update_series's own SQL leaves the
                // column untouched regardless of what's passed here; carried forward only
                // so this struct literal is complete.
                cursor_date: current.cursor_date,
                // basis is a normal round-trip field, same category as recurrence/
                // anchor_date — omitting it does not preserve current.basis.
                basis: params.basis,
                // Same round-trip convention as basis — omitting it clears it, not
                // preserves current.template_item_id.
                template_item_id: params.template_item_id,
                // Same round-trip convention — omitting either clears it, not preserves
                // current.assigned_to_user_id/points. Already re-validated above (a prior
                // admin's points value doesn't survive a non-admin's edit of anything else).
                assigned_to_user_id,
                points,
            },
        )
        .await?;
    Ok(())
}

pub async fn list_series_for_project(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    project_id: &str,
) -> Result<Vec<ItemSeries>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(event_series.list_series_for_project(project_id).await?)
}

/// Stage 5 of docs/recurring-events-virtual-occurrences-rough-plan.md. A series occurrence
/// date with no `event_occurrences` row at all — i.e. neither materialized nor skipped.
/// Never overlaps with what the normal item queries (`list_due_by_project`,
/// `list_project_events`, ...) return: a materialized date always has a row (excluded here),
/// and a real `items` row is what those queries read from directly.
///
/// `item_type` (added Stage 8) is the series' own kind, carried through so callers can
/// filter/label by it — before this field existed, every caller of
/// `list_virtual_occurrences_for_project_unchecked` implicitly assumed Event, so a
/// `TASK`-typed series (possible since Stage 7b) silently leaked its occurrences into
/// every Stage 5 surface unfiltered and unlabeled. Stage 8 is what fixes that.
///
/// `is_current` (added Stage 9) marks the one occurrence, per Task-typed series, that
/// equals `current_occurrence_date` — meaningless (`false`) for Event-typed series, which
/// have no cursor. See this function's own doc comment for how it interacts with the
/// past-date clamp.
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualOccurrence {
    pub series_id: String,
    pub series_name: String,
    pub item_type: ItemKind,
    pub event_type: Option<String>,
    pub occurrence_date: DateTime<Utc>,
    pub is_current: bool,
}

/// Unchecked, matching `list_due_project_items_unchecked`'s naming precedent — every caller
/// (the project dashboard's list/calendar views, the Events month-grid view) already resolves
/// project membership earlier in the same handler.
///
/// A series whose `recurrence` string fails to parse is skipped silently, not propagated as
/// an error — there is no server-side validation guaranteeing `ItemSeries.recurrence` always
/// parses, and `occurrences_between` itself already set this "empty, not an error" precedent;
/// one malformed series must not blank out an entire project's dashboard.
///
/// A date is dropped from the result if `event_occurrences` has *any* row for it, materialized
/// or exdate — the only two writes into that table (`record_materialized_occurrence`,
/// `mark_exdate`) never produce an `(item_id: None, is_exdate: false)` row, so "a row exists"
/// and "not virtual" are the same predicate. This is also what makes a future skip-UI (Stage
/// 6) correctly exclude skipped occurrences from this list today, with no exdate-specific
/// code here at all.
///
/// Stage 8 clamped every Task-typed virtual occurrence to `occurrence_date >= now`,
/// deferring backlog to Stage 9. Stage 9 now lives here: the clamp still applies, with one
/// exemption — a series' own `current_occurrence_date` (see that function's doc comment)
/// is let through even when it's in the past, so a backlogged Task series' one settleable
/// occurrence actually surfaces instead of vanishing. This subsumes the near-identical
/// filter that used to be duplicated in three separate `web_ui` call sites (the dashboard's
/// list/calendar views, the Tasks month-grid) — moved here since only this function has
/// the series' `cursor_date` needed to compute the exemption.
///
/// The current occurrence is also injected outright when it falls *outside*
/// `[range_start, range_end]` entirely, not just when it's merely past-dated within an
/// otherwise-generated candidate list — caught by manual smoke testing of this stage: every
/// caller's own default window starts at `now` (a virtual occurrence was never "missed" in
/// any actionable sense before this stage existed), so a genuinely backlogged current
/// occurrence would otherwise never appear in the one range a caller actually queries by
/// default. A day-bucketing caller (a calendar month-grid) still only renders it if the
/// injected date happens to land within its own visible grid — this only guarantees the date
/// is present in this function's *return value*, not that every caller visualizes it.
pub async fn list_virtual_occurrences_for_project_unchecked(
    event_series: &Arc<dyn ItemSeriesRepo>,
    project_id: &str,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<Vec<VirtualOccurrence>, ItemError> {
    let now = Utc::now();
    let all_series = event_series.list_series_for_project(project_id).await?;
    let mut result = Vec::new();
    for series in &all_series {
        let Ok(rule) = recurrence::parse(&series.recurrence) else {
            continue;
        };
        // Stage 10 gap 1: the predicted list is normally rooted at the series' own
        // anchor_date, but for a completion-basis Task series that drifts silently wrong
        // after the first off-schedule settlement (the fixed anchor-rooted sequence and
        // the real cursor-chained trajectory permanently diverge). Rooting at
        // current_occurrence_date instead self-corrects every render, since nothing here
        // is cached. When cursor_date is None this is identical to anchor_date, so a
        // fresh series behaves the same as before. current_date must be computed before
        // candidates below, since it's now also the root, not only the injected-extra date.
        let current_date = current_occurrence_date(series, &rule, tz_offset_minutes);
        let root_date = if series.item_type == ItemKind::Task && is_completion_basis(series) {
            current_date
        } else {
            series.anchor_date
        };
        let mut candidates = recurrence::occurrences_between(
            &rule,
            root_date,
            range_start,
            range_end,
            tz_offset_minutes,
        );
        // A Task series' current occurrence must surface regardless of the caller's window —
        // callers (the dashboard's default "Today"/preset windows especially) default their
        // own range to start at `now`, since a virtual occurrence was never "missed" in any
        // actionable sense before Stage 9 existed (see `virtual_occurrence_window`'s own doc
        // comment in `project_dashboard.rs`). That reasoning no longer holds for a Task series:
        // its current occurrence can genuinely be backlogged into the past, and if it isn't
        // injected here, it silently never renders on the one view (the dashboard list) users
        // actually look at by default — the whole point of this stage's backlog design would be
        // unreachable in practice. `occurrences_between` only generates dates inside
        // `[range_start, range_end]`, so a current_date outside that window needs adding by hand.
        let current_outside_window =
            series.item_type == ItemKind::Task && !(range_start..=range_end).contains(&current_date);
        if current_outside_window {
            candidates.push(current_date);
        }
        if candidates.is_empty() {
            continue;
        }
        // Widened to cover an injected current_date too, so an already-settled (materialized
        // or skipped) one outside the caller's window is still correctly excluded below rather
        // than reappearing as virtual.
        let query_start = range_start.min(current_date);
        let query_end = range_end.max(current_date);
        let existing = event_series
            .list_occurrences_between(&series.id, query_start, query_end)
            .await?;
        let taken: HashSet<i64> = existing.iter().map(|o| o.occurrence_date.timestamp()).collect();
        for date in candidates {
            if taken.contains(&date.timestamp()) {
                continue;
            }
            let is_current = series.item_type == ItemKind::Task && date == current_date;
            if series.item_type == ItemKind::Task && date < now && !is_current {
                continue;
            }
            result.push(VirtualOccurrence {
                series_id: series.id.clone(),
                series_name: series.name.clone(),
                item_type: series.item_type,
                event_type: series.event_type.clone(),
                occurrence_date: date,
                is_current,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item_series::{ItemOccurrence, ItemSeries};
    use crate::domain::project::Project;
    use crate::storage::sqlite::{
        MockItemSeriesRepo, MockItemRepo, MockProjectRepo, MockTeamRepo, RepoError,
    };

    fn series(project_id: &str) -> ItemSeries {
        ItemSeries {
            id: "s1".to_string(),
            project_id: project_id.to_string(),
            name: "Standup".to_string(),
            description: None,
            event_type: None,
            // A genuinely parseable pattern (recurrence::parse has no "every weekday" form —
            // only specific weekday names like "every monday") — most callers of this helper
            // never parse it, but Stage 9's current_occurrence_date tests do.
            recurrence: "every 7 days".to_string(),
            anchor_date: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            item_type: ItemKind::Event,
            cursor_date: None,
            basis: None,
            template_item_id: None,
            assigned_to_user_id: None,
            points: None,
        }
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

    fn occurrence_date() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_500_000, 0).unwrap()
    }

    #[tokio::test]
    async fn returns_existing_item_when_already_materialized() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: Some("existing-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_record_materialized_occurrence().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock.expect_create().times(0);
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "existing-item")
            .returning(|_, _| Ok(Item::new_project_item("p1", "Standup")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &event_series,
            "owner1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should return existing materialized item");

        assert_eq!(item.name, "Standup");
    }

    #[tokio::test]
    async fn materializes_a_new_event_when_no_occurrence_row_exists() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        series_mock
            .expect_record_materialized_occurrence()
            .withf(|series_id: &str, _date, item_id: &str| series_id == "s1" && item_id == "new-item-id")
            .times(1)
            .returning(|_, _, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        projects_mock.expect_find_personal_project().returning(|_| Ok(None));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| {
                item.kind() == ItemKind::Event
                    && item.scheduled_date() == Some(occurrence_date())
                    && item.has_scheduled_time()
            })
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "new-item-id")
            .returning(|_, _| Ok(Item::new_project_item("p1", "Standup")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &event_series,
            "owner1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should materialize a new occurrence");

        assert_eq!(item.name, "Standup");
    }

    #[tokio::test]
    async fn materializes_a_task_when_series_is_task_typed() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        projects_mock.expect_find_personal_project().returning(|_| Ok(None));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.kind() == ItemKind::Task)
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Standup")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &event_series,
            "owner1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should materialize a task occurrence");

        assert_eq!(item.name, "Standup");
    }

    #[tokio::test]
    async fn materializes_a_task_and_copies_the_linked_templates_children() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.template_item_id = Some("template-1".to_string());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        projects_mock.expect_find_personal_project().returning(|_| Ok(None));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        // The occurrence's own item.
        items_mock
            .expect_create()
            .withf(|item: &Item| item.kind() == ItemKind::Task && item.parent_item_id.is_none())
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        // The template's one child, copied via copy_template_children.
        items_mock
            .expect_create()
            .withf(|item: &Item| {
                item.kind() == ItemKind::Task
                    && item.parent_item_id.as_deref() == Some("new-item-id")
                    && item.due_date().is_some()
            })
            .times(1)
            .returning(|_| Ok("new-child-id".to_string()));
        items_mock
            .expect_list_children()
            .withf(|parent_id: &str| parent_id == "template-1")
            .times(1)
            .returning(|_| {
                let mut child = Item::new_project_item("p1", "Prep agenda");
                child.id = "template-child-1".to_string();
                child.parent_item_id = Some("template-1".to_string());
                if let Some(recurrence) = child.item_type.recurrence_mut() {
                    recurrence.due_offset_days = Some(2);
                }
                Ok(vec![child])
            });
        items_mock
            .expect_list_children()
            .withf(|parent_id: &str| parent_id == "template-child-1")
            .times(1)
            .returning(|_| Ok(vec![]));
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Standup")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &event_series,
            "owner1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should materialize a task occurrence and copy the template's children");

        assert_eq!(item.name, "Standup");
    }

    #[tokio::test]
    async fn propagates_not_found_for_unknown_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        series_mock.expect_get_occurrence().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &event_series,
            "owner1",
            "bogus",
            occurrence_date(),
            0,
        )
        .await;

        assert!(matches!(result, Err(ItemError::NotFound)));
    }

    #[tokio::test]
    async fn rejects_non_member_on_personal_project() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &event_series,
            "not-the-owner",
            "s1",
            occurrence_date(),
            0,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn materializes_on_a_team_backed_project_too() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.project_id.as_deref() == Some("p1") && item.user_id.is_none())
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Standup")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &event_series,
            "member1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should materialize on a team-backed project");

        assert_eq!(item.name, "Standup");
    }

    #[tokio::test]
    async fn skip_occurrence_marks_exdate_after_confirming_series_exists() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock
            .expect_mark_exdate()
            .withf(|series_id: &str, _date| series_id == "s1")
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&event_series, "s1", occurrence_date(), 0)
            .await
            .expect("should mark the occurrence as skipped");
    }

    #[tokio::test]
    async fn skip_occurrence_does_not_advance_cursor_for_an_event_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_mark_exdate().returning(|_, _| Ok(()));
        series_mock.expect_advance_cursor().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&event_series, "s1", occurrence_date(), 0)
            .await
            .expect("should mark the occurrence as skipped");
    }

    #[tokio::test]
    async fn skip_occurrence_advances_cursor_for_a_task_series() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // A fresh series' current occurrence is its own anchor_date (cursor_date: None) —
        // set the anchor to the date this test skips, so it's the current occurrence and
        // require_current_occurrence lets it through.
        task_series.anchor_date = occurrence_date();
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock.expect_mark_exdate().returning(|_, _| Ok(()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| series_id == "s1" && *date == occurrence_date())
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&event_series, "s1", occurrence_date(), 0)
            .await
            .expect("should mark the occurrence as skipped and advance the cursor");
    }

    #[tokio::test]
    async fn skip_occurrence_advances_cursor_to_now_for_a_completion_basis_series() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.basis = Some("COMPLETION".to_string());
        task_series.anchor_date = occurrence_date();
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock.expect_mark_exdate().returning(|_, _| Ok(()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                // occurrence_date() is well in the past (a fixed test timestamp), so a
                // completion-basis skip should advance to something close to "now," not
                // that nominal date — assert a generous tolerance rather than exact
                // equality, since Utc::now() can't be pinned in a unit test.
                series_id == "s1" && (Utc::now() - *date).num_seconds().abs() < 30
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&event_series, "s1", occurrence_date(), 0)
            .await
            .expect("should advance the cursor to roughly now");
    }

    #[tokio::test]
    async fn skip_occurrence_rejects_a_non_current_task_series_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // anchor_date stays the default (not occurrence_date()), so occurrence_date()
        // is not the series' current occurrence.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock.expect_mark_exdate().times(0);
        series_mock.expect_advance_cursor().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = skip_occurrence(&event_series, "s1", occurrence_date(), 0).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn validate_completable_rejects_a_non_current_task_series_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                // Not the series' anchor/current date.
                occurrence_date: occurrence_date(),
                item_id: Some("completed-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = validate_completable(&event_series, "completed-item", 0).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn validate_completable_allows_the_current_task_series_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // A fresh series' current occurrence is its own anchor_date.
        task_series.anchor_date = occurrence_date();
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: occurrence_date(),
                item_id: Some("completed-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_completable(&event_series, "completed-item", 0)
            .await
            .expect("the series' own current occurrence should be completable");
    }

    #[tokio::test]
    async fn validate_completable_allows_any_date_for_an_event_series() {
        // series("p1") defaults to ItemKind::Event — no cursor/current concept, so
        // any occurrence date is fine.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: occurrence_date(),
                item_id: Some("some-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_completable(&event_series, "some-item", 0)
            .await
            .expect("an Event-typed series has no current-occurrence restriction");
    }

    #[tokio::test]
    async fn validate_completable_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| Ok(None));
        series_mock.expect_get_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_completable(&event_series, "some-task", 0)
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    #[tokio::test]
    async fn unlink_deleted_item_occurrence_marks_exdate_when_item_came_from_a_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|item_id| {
            assert_eq!(item_id, "deleted-item");
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: occurrence_date(),
                item_id: Some("deleted-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock
            .expect_mark_exdate()
            .withf(|series_id: &str, date: &DateTime<Utc>| series_id == "s1" && *date == occurrence_date())
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unlink_deleted_item_occurrence(&event_series, "deleted-item")
            .await
            .expect("should mark the occurrence exdate");
    }

    #[tokio::test]
    async fn unlink_deleted_item_occurrence_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| Ok(None));
        series_mock.expect_mark_exdate().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unlink_deleted_item_occurrence(&event_series, "some-task")
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    #[tokio::test]
    async fn record_task_completion_advances_cursor_for_a_materialized_task_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|item_id| {
            assert_eq!(item_id, "completed-item");
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: occurrence_date(),
                item_id: Some("completed-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| series_id == "s1" && *date == occurrence_date())
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&event_series, "completed-item")
            .await
            .expect("should advance the cursor");
    }

    #[tokio::test]
    async fn record_task_completion_advances_cursor_to_now_for_a_completion_basis_series() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.basis = Some("COMPLETION".to_string());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: occurrence_date(),
                item_id: Some("completed-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && (Utc::now() - *date).num_seconds().abs() < 30
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&event_series, "completed-item")
            .await
            .expect("should advance the cursor to roughly now");
    }

    #[tokio::test]
    async fn record_task_completion_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| Ok(None));
        series_mock.expect_get_series().times(0);
        series_mock.expect_advance_cursor().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&event_series, "some-task")
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    #[tokio::test]
    async fn record_task_completion_does_not_advance_cursor_for_an_event_typed_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_find_occurrence_by_item_id().returning(|_| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: occurrence_date(),
                item_id: Some("some-event".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_advance_cursor().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&event_series, "some-event")
            .await
            .expect("should no-op for an Event-typed series");
    }

    #[test]
    fn current_occurrence_date_starts_at_anchor_when_cursor_is_unset() {
        let s = series("p1");
        let rule = recurrence::parse(&s.recurrence).unwrap();

        let current = current_occurrence_date(&s, &rule, 0);

        assert_eq!(current, s.anchor_date);
    }

    #[test]
    fn current_occurrence_date_advances_one_step_past_the_cursor() {
        let mut s = series("p1");
        s.recurrence = "every 3 days".to_string();
        s.cursor_date = Some(s.anchor_date);
        let rule = recurrence::parse(&s.recurrence).unwrap();

        let current = current_occurrence_date(&s, &rule, 0);

        assert_eq!(current, s.anchor_date + chrono::Duration::days(3));
    }

    #[tokio::test]
    async fn skip_occurrence_propagates_not_found_without_marking() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        series_mock.expect_mark_exdate().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = skip_occurrence(&event_series, "bogus", occurrence_date(), 0).await;
        assert!(matches!(result, Err(ItemError::NotFound)));
    }

    fn no_template_repo() -> Arc<dyn ItemRepo> {
        Arc::new(MockItemRepo::new())
    }

    fn template_item(id: &str, project_id: &str) -> Item {
        Item {
            id: id.to_string(),
            project_id: Some(project_id.to_string()),
            name: "Weekly prep".to_string(),
            item_type: crate::domain::item::ItemType::from_kind(ItemKind::Template),
            ..Item::default()
        }
    }

    fn create_params(project_id: &str) -> CreateItemSeriesParams {
        CreateItemSeriesParams {
            project_id: project_id.to_string(),
            name: "Standup".to_string(),
            description: None,
            event_type: None,
            recurrence: "every weekday".to_string(),
            anchor_date: occurrence_date(),
            item_type: ItemKind::Event,
            basis: None,
            template_item_id: None,
            assigned_to_user_id: None,
            points: None,
        }
    }

    fn update_params() -> UpdateItemSeriesParams {
        UpdateItemSeriesParams {
            name: "Retro".to_string(),
            description: Some("Weekly retro".to_string()),
            // event_type is currently unsupported on any series (see
            // validate_series_event_type) — this baseline stays valid by default; tests
            // exercising the rejection set it explicitly.
            event_type: None,
            recurrence: "every friday".to_string(),
            anchor_date: occurrence_date(),
            item_type: ItemKind::Event,
            basis: None,
            template_item_id: None,
            assigned_to_user_id: None,
            points: None,
        }
    }

    #[tokio::test]
    async fn create_series_creates_after_confirming_membership() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.project_id == "p1" && s.name == "Standup")
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let id = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &event_series,
            "owner1",
            create_params("p1"),
        )
        .await
        .expect("owner should be able to create a series");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());

        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &event_series,
            "not-the-owner",
            create_params("p1"),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_series_creates_a_task_typed_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.item_type == ItemKind::Task)
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        let id = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params)
            .await
            .expect("owner should be able to create a task-typed series");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_rejects_assignment_on_event_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.assigned_to_user_id = Some("member1".to_string());
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_points_on_a_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.points = Some(10);
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_honors_assignment_and_points_for_a_team_project_admin() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Admin)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| {
                s.assigned_to_user_id == Some("member1".to_string()) && s.points == Some(10)
            })
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.assigned_to_user_id = Some("member1".to_string());
        params.points = Some(10);
        let id = create_series(&no_template_repo(), &projects, &teams, &event_series, "admin1", params)
            .await
            .expect("admin should be able to set assignment and points");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_drops_points_but_keeps_assignment_for_a_non_admin() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.assigned_to_user_id == Some("member1".to_string()) && s.points.is_none())
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.assigned_to_user_id = Some("member1".to_string());
        params.points = Some(10);
        let id = create_series(&no_template_repo(), &projects, &teams, &event_series, "member1", params)
            .await
            .expect("non-admin member should still be able to set assignment");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_rejects_template_item_type() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Template;
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_simple_item_type() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Simple;
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_event_type_on_task_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = Some("rain".to_string());
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_event_type_on_event_series() {
        // event_type is currently unsupported on any series, not just Task — see
        // validate_series_event_type's doc comment for why. This was previously allowed.
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.event_type = Some("rain".to_string());
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_allows_task_series_without_event_type() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = None;
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_series_rejects_completion_basis_on_event_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.basis = Some("COMPLETION".to_string());
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_completion_basis_on_an_ineligible_pattern() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = None;
        // create_params()'s default recurrence is "every weekday" — a WeeklyDay pattern,
        // not an "every N units" one.
        params.basis = Some("COMPLETION".to_string());
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_allows_completion_basis_on_an_eligible_task_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.basis.as_deref() == Some("COMPLETION"))
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = None;
        params.recurrence = "every 3 days".to_string();
        params.basis = Some("COMPLETION".to_string());
        let result = create_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", params).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_series_rejects_template_item_id_on_an_event_series() {
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.template_item_id = Some("template-1".to_string());
        let result = create_series(&repo, &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_a_non_template_item_as_the_template() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "not-a-template")
            .returning(|_, _| Ok(Item::new_project_item("p1", "Just a task")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.template_item_id = Some("not-a-template".to_string());
        let result = create_series(&repo, &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_a_template_item_from_another_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Err(RepoError::NotFound));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.template_item_id = Some("other-projects-template".to_string());
        let result = create_series(&repo, &projects, &teams, &event_series, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::NotFound)));
    }

    #[tokio::test]
    async fn create_series_accepts_a_valid_template_item_id() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.template_item_id.as_deref() == Some("template-1"))
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "template-1")
            .returning(|project_id, id| Ok(template_item(id, project_id)));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.template_item_id = Some("template-1".to_string());
        let result = create_series(&repo, &projects, &teams, &event_series, "owner1", params).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn update_series_rejects_event_type_on_task_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut params = update_params();
        params.item_type = ItemKind::Task;
        params.event_type = Some("meeting".to_string());
        let result = update_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", "s1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_series_rejects_event_type_on_event_series() {
        // event_type is currently unsupported on any series, not just Task — this
        // combination was previously allowed.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut params = update_params();
        params.item_type = ItemKind::Event;
        params.event_type = Some("meeting".to_string());
        let result = update_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", "s1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_series_rejects_template_item_type() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut params = update_params();
        params.item_type = ItemKind::Template;
        let result = update_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", "s1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_series_accepts_a_valid_template_item_id() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock
            .expect_update_series()
            .withf(|_, s: &ItemSeries| s.template_item_id.as_deref() == Some("template-1"))
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "template-1")
            .returning(|project_id, id| Ok(template_item(id, project_id)));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut params = update_params();
        params.item_type = ItemKind::Task;
        params.template_item_id = Some("template-1".to_string());
        let result = update_series(&repo, &projects, &teams, &event_series, "owner1", "s1", params).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_series_returns_series_for_a_member() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_series(&projects, &teams, &event_series, "owner1", "s1")
            .await
            .expect("owner should be able to read the series");
        assert_eq!(result.name, "Standup");
    }

    #[tokio::test]
    async fn get_series_propagates_not_found() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_series(&projects, &teams, &event_series, "owner1", "bogus").await;
        assert!(matches!(result, Err(ItemError::NotFound)));
    }

    #[tokio::test]
    async fn update_series_overwrites_fields_after_confirming_membership() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock
            .expect_update_series()
            .withf(|series_id: &str, s: &ItemSeries| {
                series_id == "s1" && s.project_id == "p1" && s.name == "Retro"
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        update_series(&no_template_repo(), &projects, &teams, &event_series, "owner1", "s1", update_params())
            .await
            .expect("owner should be able to update the series");
    }

    #[tokio::test]
    async fn update_series_rejects_non_member() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = update_series(
            &no_template_repo(),
            &projects,
            &teams,
            &event_series,
            "not-the-owner",
            "s1",
            update_params(),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_series_for_project_returns_series_for_a_member() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(|_| Ok(vec![series("p1")]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = list_series_for_project(&projects, &teams, &event_series, "owner1", "p1")
            .await
            .expect("owner should be able to list series");
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn list_series_for_project_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());

        let result =
            list_series_for_project(&projects, &teams, &event_series, "not-the-owner", "p1").await;
        assert!(result.is_err());
    }

    fn series_ex(id: &str, project_id: &str, name: &str, recurrence: &str, anchor: DateTime<Utc>) -> ItemSeries {
        ItemSeries {
            id: id.to_string(),
            project_id: project_id.to_string(),
            name: name.to_string(),
            description: None,
            event_type: None,
            recurrence: recurrence.to_string(),
            anchor_date: anchor,
            item_type: ItemKind::Event,
            cursor_date: None,
            basis: None,
            template_item_id: None,
            assigned_to_user_id: None,
            points: None,
        }
    }

    fn anchor() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[tokio::test]
    async fn list_virtual_occurrences_returns_dates_with_no_occurrence_row() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(|_| Ok(vec![series_ex("s1", "p1", "Standup", "every 3 days", anchor())]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        // anchor, +3d, +6d, +9d
        assert_eq!(result.len(), 4);
        assert!(result.iter().all(|o| o.series_id == "s1" && o.series_name == "Standup"));
        assert!(result.iter().all(|o| o.item_type == ItemKind::Event));
    }

    #[tokio::test]
    async fn list_virtual_occurrences_carries_item_type_from_a_task_typed_series() {
        let mut task_series = series_ex("s1", "p1", "Take out trash", "every 3 days", anchor());
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![task_series.clone()]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        assert!(!result.is_empty());
        assert!(result.iter().all(|o| o.item_type == ItemKind::Task));
    }

    #[tokio::test]
    async fn list_virtual_occurrences_task_backlog_current_occurrence_survives_past_clamp() {
        // anchor() is 2023 — every candidate in this window is in the past. With no
        // cursor_date set, current_occurrence_date is the anchor itself, so only that one
        // date should survive Stage 9's clamp; the later ones (+3d, +6d, +9d) are stale
        // backlog behind the current occurrence and should be dropped, per Stage 8's
        // original clamp (now scoped by the current-occurrence exemption).
        let mut task_series = series_ex("s1", "p1", "Take out trash", "every 3 days", anchor());
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![task_series.clone()]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].occurrence_date, anchor());
        assert!(result[0].is_current);
    }

    #[tokio::test]
    async fn list_virtual_occurrences_injects_current_date_outside_the_caller_window() {
        // Caught by manual smoke testing: the dashboard's default window starts at `now`
        // (never extended backward), so a genuinely backlogged current occurrence (anchor()
        // is 2023) would never appear in a caller-requested window that only covers the
        // future — unless it's injected regardless of range_start/range_end. "every 400
        // days" (rather than the usual "every 3 days") deliberately has zero *naturally*
        // generated candidates in a 90-day future window ~3 years after anchor() (2023),
        // isolating the injected entry from the rule's own ordinary future occurrences.
        let mut task_series = series_ex("s1", "p1", "Take out trash", "every 400 days", anchor());
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![task_series.clone()]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let now = Utc::now();
        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            now,
            now + chrono::Duration::days(90),
            0,
        )
        .await
        .expect("should succeed");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].occurrence_date, anchor());
        assert!(result[0].is_current);
    }

    #[tokio::test]
    async fn list_virtual_occurrences_does_not_inject_an_already_settled_current_date() {
        // The current occurrence is already materialized outside the caller's window — must
        // not reappear as a virtual/current entry just because it was injected for the lookup.
        // Same sparse "every 400 days" pattern as the sibling test above, so the only
        // candidate in play is the injected (and here, already-settled) current_date.
        let mut task_series = series_ex("s1", "p1", "Take out trash", "every 400 days", anchor());
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![task_series.clone()]));
        series_mock.expect_list_occurrences_between().returning(|_, _, _| {
            Ok(vec![ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: anchor(),
                item_id: Some("already-materialized".to_string()),
                is_exdate: false,
            }])
        });
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let now = Utc::now();
        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            now,
            now + chrono::Duration::days(90),
            0,
        )
        .await
        .expect("should succeed");

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_virtual_occurrences_task_current_date_derived_from_cursor_not_anchor() {
        // Same series as above, but cursor_date has already settled the anchor occurrence
        // — the "current" one is now one step past the cursor (+3d), not the anchor.
        let mut task_series = series_ex("s1", "p1", "Take out trash", "every 3 days", anchor());
        task_series.item_type = ItemKind::Task;
        task_series.cursor_date = Some(anchor());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![task_series.clone()]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].occurrence_date, anchor() + chrono::Duration::days(3));
        assert!(result[0].is_current);
    }

    #[tokio::test]
    async fn list_virtual_occurrences_completion_basis_task_series_roots_at_current_not_anchor() {
        // Stage 10 gap 1: cursor_date (and so current_date, one step past it) is set in the
        // future relative to Utc::now() — deliberately, so none of the candidate dates below
        // get dropped by the unrelated past-date clamp (Stage 9) that only ever exempts the
        // single current_date, not its future-predicted successors. anchor_date sits 101 days
        // before cursor_date — not a multiple of 3 away from current_date (101 % 3 != 0) — so
        // an (incorrectly) anchor-rooted predicted list would land on different dates
        // (current_date +1d/+4d/+7d/+10d) than the current-date-rooted one this test expects
        // (current_date +0d/+3d/+6d/+9d), distinguishing the two roots either way this fails.
        // Truncated to whole seconds — the `rrule` crate normalizes DTSTART to
        // whole-second precision internally, so a sub-second `Utc::now()` fraction on a
        // range bound can cause boundary comparisons to miss an exact-match date; every
        // other date in this test file avoids this the same way (fixed whole-second
        // constants), see the sibling `occurrences_between` tests in
        // `domain::recurrence`.
        let cursor_date = DateTime::from_timestamp((Utc::now() + chrono::Duration::days(50)).timestamp(), 0).unwrap();
        let anchor_date = cursor_date - chrono::Duration::days(101);
        let mut task_series = series_ex("s1", "p1", "Water plants", "every 3 days", anchor_date);
        task_series.item_type = ItemKind::Task;
        task_series.basis = Some("COMPLETION".to_string());
        task_series.cursor_date = Some(cursor_date);
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![task_series.clone()]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let current_date = cursor_date + chrono::Duration::days(3);
        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            current_date,
            current_date + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        let mut dates: Vec<_> = result.iter().map(|o| o.occurrence_date).collect();
        dates.sort();
        assert_eq!(
            dates,
            vec![
                current_date,
                current_date + chrono::Duration::days(3),
                current_date + chrono::Duration::days(6),
                current_date + chrono::Duration::days(9),
            ]
        );
        assert!(result.iter().find(|o| o.occurrence_date == current_date).unwrap().is_current);
    }

    #[tokio::test]
    async fn list_virtual_occurrences_task_future_dates_are_never_clamped() {
        // Truncated to whole seconds (matching every other fixture's DateTime::from_timestamp
        // convention) — occurrences_between's underlying RRule generation loses sub-second
        // precision, so a raw Utc::now()-derived anchor can end up a few nanoseconds "later"
        // than the range_start built from it, excluding the anchor occurrence from a strictly-
        // after bound.
        let far_future_anchor =
            DateTime::from_timestamp((Utc::now() + chrono::Duration::days(30)).timestamp(), 0).unwrap();
        let mut task_series = series_ex("s1", "p1", "Future task", "every 3 days", far_future_anchor);
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![task_series.clone()]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            far_future_anchor,
            far_future_anchor + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        // anchor, +3d, +6d, +9d — none clamped, since all are in the future.
        assert_eq!(result.len(), 4);
        // Only the anchor (== current_occurrence_date, cursor unset) is marked current.
        assert_eq!(result.iter().filter(|o| o.is_current).count(), 1);
        assert!(result.iter().find(|o| o.is_current).unwrap().occurrence_date == far_future_anchor);
    }

    #[tokio::test]
    async fn list_virtual_occurrences_excludes_materialized_dates() {
        let materialized_date = anchor() + chrono::Duration::days(3);
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(|_| Ok(vec![series_ex("s1", "p1", "Standup", "every 3 days", anchor())]));
        series_mock.expect_list_occurrences_between().returning(move |_, _, _| {
            Ok(vec![ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: materialized_date,
                item_id: Some("item-1".to_string()),
                is_exdate: false,
            }])
        });
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|o| o.occurrence_date != materialized_date));
    }

    #[tokio::test]
    async fn list_virtual_occurrences_excludes_exdate_dates() {
        let skipped_date = anchor() + chrono::Duration::days(6);
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(|_| Ok(vec![series_ex("s1", "p1", "Standup", "every 3 days", anchor())]));
        series_mock.expect_list_occurrences_between().returning(move |_, _, _| {
            Ok(vec![ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: skipped_date,
                item_id: None,
                is_exdate: true,
            }])
        });
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|o| o.occurrence_date != skipped_date));
    }

    #[tokio::test]
    async fn list_virtual_occurrences_skips_a_series_with_unparseable_recurrence() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_list_series_for_project().returning(|_| {
            Ok(vec![
                series_ex("bad", "p1", "Bad", "not a real pattern", anchor()),
                series_ex("good", "p1", "Good", "every 3 days", anchor()),
            ])
        });
        series_mock
            .expect_list_occurrences_between()
            .withf(|series_id: &str, _, _| series_id == "good")
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("a malformed series must not error the whole listing");

        assert!(result.iter().all(|o| o.series_id == "good"));
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn list_virtual_occurrences_returns_empty_for_project_with_no_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_list_series_for_project().returning(|_| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_virtual_occurrences_scopes_to_the_given_range() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(|_| Ok(vec![series_ex("s1", "p1", "Standup", "every 3 days", anchor())]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        // Narrow window: only the anchor itself and +3d should land inside [anchor, anchor+4d].
        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(4),
            0,
        )
        .await
        .expect("should succeed");

        assert_eq!(result.len(), 2);
        for o in &result {
            assert!(o.occurrence_date >= anchor() && o.occurrence_date <= anchor() + chrono::Duration::days(4));
        }
    }

    #[tokio::test]
    async fn list_virtual_occurrences_combines_multiple_series_in_one_project() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_list_series_for_project().returning(|_| {
            Ok(vec![
                series_ex("s1", "p1", "Standup", "every 3 days", anchor()),
                series_ex("s2", "p1", "Retro", "every 5 days", anchor()),
            ])
        });
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = list_virtual_occurrences_for_project_unchecked(
            &event_series,
            "p1",
            anchor(),
            anchor() + chrono::Duration::days(10),
            0,
        )
        .await
        .expect("should succeed");

        let standup: Vec<_> = result.iter().filter(|o| o.series_id == "s1").collect();
        let retro: Vec<_> = result.iter().filter(|o| o.series_id == "s2").collect();
        assert!(standup.iter().all(|o| o.series_name == "Standup"));
        assert!(retro.iter().all(|o| o.series_name == "Retro"));
        // s1: anchor, +3d, +6d, +9d (4); s2: anchor, +5d, +10d (3)
        assert_eq!(standup.len(), 4);
        assert_eq!(retro.len(), 3);
    }
}
