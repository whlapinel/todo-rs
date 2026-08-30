use crate::domain::item::{self, Item, ItemKind};
use crate::domain::item_series::{ItemOccurrence, ItemSeries};
use crate::domain::recurrence;
use crate::service::error::ItemError;
use crate::service::project_items::{self, CreateProjectItemParams};
use crate::service::projects::{
    require_project_admin, require_project_member, resolve_project_assignee,
};
use crate::storage::sqlite::{
    ItemDependencyRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, ReminderRepo, TeamRepo, UserRepo,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
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
    series_repo: &Arc<dyn ItemSeriesRepo>,
    reminders: &Arc<dyn ReminderRepo>,
    requester_user_id: &str,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<Item, ItemError> {
    let series = series_repo.get_series(series_id).await?;
    let existing = series_repo
        .get_occurrence(series_id, occurrence_date)
        .await?;

    if let Some(occurrence) = existing
        && let Some(item_id) = occurrence.item_id
    {
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
    // Only ever Some on a Task-typed series on a team-backed project —
    // resolve_series_assignment already enforced that at create/update time, so this
    // is a plain resolve, not a re-validation. Fixed assignee or rotation member,
    // whichever the series is set up for (docs/assignment-rotation-plan.md, Stage 2).
    let occurrence_assignee =
        resolve_occurrence_assignee(series_repo, &series, occurrence_date, tz_offset_minutes)
            .await?;
    // Due-date-basis materializes onto due_date instead of scheduled_date (see
    // ItemSeries::basis's doc comment) — everything else about the created item is
    // identical between the two branches.
    let params = if is_due_date_basis(&series) {
        CreateProjectItemParams {
            project_id: series.project_id.clone(),
            name: series.name.clone(),
            description: series.description.clone(),
            item_type: Some(series.item_type),
            event_type: series.event_type.clone(),
            due_date: Some(occurrence_date),
            has_due_time: Some(true),
            assigned_to_user_id: occurrence_assignee,
            points: series.points,
            priority: series.priority,
            series_id: Some(series.id.clone()),
            ..Default::default()
        }
    } else {
        CreateProjectItemParams {
            project_id: series.project_id.clone(),
            name: series.name.clone(),
            description: series.description.clone(),
            item_type: Some(series.item_type),
            event_type: series.event_type.clone(),
            scheduled_date: Some(occurrence_date),
            has_scheduled_time: Some(true),
            assigned_to_user_id: occurrence_assignee,
            points: series.points,
            priority: series.priority,
            series_id: Some(series.id.clone()),
            ..Default::default()
        }
    };
    let item_id = project_items::create_project_item(
        repo,
        projects,
        teams,
        reminders,
        requester_user_id,
        params,
    )
    .await?;

    series_repo
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

/// Marks `occurrence_date` as skipped (the EXDATE-equivalent) for `series_id`. Historically
/// (Stage 6) this was wired only onto genuinely virtual occurrences, so `occurrence_date`
/// never already had a materialized `item_id` behind it in practice; as of Stage B of
/// `docs/unify-virtual-materialized-occurrences-plan.md`, the web UI's Skip button calls
/// `skip_or_delete_series_occurrence` below instead, which deletes a materialized
/// occurrence's item first — so by the time this function itself runs, `occurrence_date`
/// is still guaranteed not to have a materialized `item_id` behind it, just via composition
/// rather than by construction. `mark_exdate` clears `item_id` unconditionally regardless,
/// so even a direct call against an already-materialized date would just orphan that item
/// rather than corrupt the occurrence row.
///
/// Stage 10a: rejects (via `require_current_occurrence`) skipping anything but a
/// Task-typed series' current occurrence, before either write below runs — see that
/// function's doc comment for why this replaces Stage 9's forward-jumping behavior.
pub async fn skip_occurrence(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    let series = series_repo.get_series(series_id).await?;
    require_current_occurrence(series_repo, &series, occurrence_date, tz_offset_minutes).await?;
    series_repo.mark_exdate(series_id, occurrence_date).await?;
    // Stage 9: skipping settles the occurrence exactly like completing one does — see
    // record_task_completion's doc comment below for why this is symmetric. Meaningless
    // for an Event-typed series (no completion/cursor concept), so left untouched there.
    // Stage 10 gap 1: cursor_value_for_settlement uses Utc::now() here too, for a
    // completion-basis series — the same symmetry.
    if series.item_type == ItemKind::Task {
        let cursor_value = cursor_value_for_settlement(&series, occurrence_date);
        series_repo.advance_cursor(series_id, cursor_value).await?;
    }
    Ok(())
}

/// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — the unified Skip
/// action, wired onto the one Skip button/route regardless of whether `occurrence_date`
/// is still virtual or already materialized. For a materialized occurrence, deletes its
/// item first via the existing `project_items::delete_project_item` (whose own
/// `unlink_deleted_item_occurrence` hook un-materializes the occurrence — deletes its
/// `item_occurrences` row rather than marking it excluded), then runs `skip_occurrence`
/// unchanged. This is a composition of two already-correct primitives, not new mutation
/// logic: a materialized occurrence ends up in exactly the state a virtual one would after
/// the same call — un-materialized, then marked exdate.
pub async fn skip_or_delete_series_occurrence(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    series_repo: &Arc<dyn ItemSeriesRepo>,
    reminders: &Arc<dyn ReminderRepo>,
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
    requester_user_id: &str,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    let series = series_repo.get_series(series_id).await?;
    if let Some(occurrence) = series_repo
        .get_occurrence(series_id, occurrence_date)
        .await?
        && let Some(item_id) = occurrence.item_id
    {
        project_items::delete_project_item(
            repo,
            projects,
            teams,
            series_repo,
            reminders,
            item_dependencies,
            requester_user_id,
            &series.project_id,
            &item_id,
        )
        .await?;
    }
    skip_occurrence(series_repo, series_id, occurrence_date, tz_offset_minutes).await
}

/// The counterpart to Skip — reverses an exdate-marked occurrence back to virtual. Mirrors
/// `record_task_uncompletion`'s cursor-safety rigor for a Task-typed series: only the
/// occurrence at `cursor_date` (the series' most recently settled one) can be unskipped, the
/// same "one at a time, in order" rule `require_cursor_occurrence` enforces for uncompleting
/// — deliberately does not self-heal past an out-of-order request the way
/// `require_current_occurrence` does for settling, since un-settling has no forward direction
/// to self-heal toward. An Event-typed series has no cursor/current concept, so any
/// exdate-marked occurrence may be unskipped unconditionally. Rejects outright if
/// `occurrence_date` isn't actually marked exdate — nothing to unskip.
pub async fn unskip_occurrence(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    let series = series_repo.get_series(series_id).await?;
    let occurrence = series_repo
        .get_occurrence(series_id, occurrence_date)
        .await?;
    if !occurrence.map(|o| o.is_exdate).unwrap_or(false) {
        return Err(ItemError::Invalid(
            "this occurrence is not skipped".to_string(),
        ));
    }
    if series.item_type == ItemKind::Task {
        if series.cursor_date != Some(occurrence_date) {
            return Err(ItemError::Invalid(format!(
                "cannot unskip this occurrence out of order — only the series' most \
                 recently settled occurrence ({:?}) can be unskipped",
                series.cursor_date
            )));
        }
        // Same shape as record_task_uncompletion's cursor restore: retreat one step, or
        // clear back to the pre-anything-settled None state if this was the anchor.
        if occurrence_date == series.anchor_date {
            series_repo.clear_cursor(series_id, occurrence_date).await?;
        } else {
            let rule = recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
            let previous = recurrence::retreat_once(&rule, occurrence_date, tz_offset_minutes);
            series_repo.retreat_cursor(series_id, previous).await?;
        }
    }
    series_repo
        .delete_occurrence(series_id, occurrence_date)
        .await?;
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
///
/// **2026-08-16: self-heals a "current" that's already marked exdate.** A cursor
/// landing exactly on an excluded date should never happen under normal settlement
/// (`skip_occurrence`/`record_task_completion` both always advance one full step past
/// whatever they settle), but it can happen out of band — e.g. deleting a materialized
/// *non-current* future occurrence marks it exdate without touching the cursor (by
/// design — that deletion never happened to be "current" at the time), and the cursor
/// can later walk forward into that same date through entirely normal, one-step-at-a-
/// time settlement. Rather than leaving the series permanently wedged there (the shape
/// of bug that blocked a real production series for six days — see
/// `unlink_deleted_item_occurrence`'s doc comment above), walk forward past any
/// consecutive already-exdate dates before comparing, persisting each step via
/// `advance_cursor` exactly like an automatic skip, so the correction sticks instead of
/// being silently recomputed (and rejected on) every call.
async fn require_current_occurrence(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    series: &ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    if series.item_type != ItemKind::Task {
        return Ok(());
    }
    let rule = recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
    let mut current = current_occurrence_date(series, &rule, tz_offset_minutes);
    while let Some(occurrence) = series_repo.get_occurrence(&series.id, current).await? {
        if !occurrence.is_exdate {
            break;
        }
        series_repo.advance_cursor(&series.id, current).await?;
        current = recurrence::advance_once(&rule, current, tz_offset_minutes);
    }
    if occurrence_date != current {
        return Err(ItemError::Invalid(format!(
            "cannot settle this occurrence out of order — the series' current \
             occurrence is {current}; occurrences must be completed or skipped \
             one at a time, in order"
        )));
    }
    Ok(())
}

/// The Uncomplete-side counterpart to `require_current_occurrence`: `cursor_date`
/// holds the most recently *settled* occurrence's own date directly (not one step
/// past it — see `current_occurrence_date`), so uncompleting is only ever valid
/// against `occurrence_date == series.cursor_date` exactly. Deliberately does not
/// self-heal past an exdate the way `require_current_occurrence` does: if the
/// cursor's own occurrence is exdate, the most recent settlement was a Skip, not a
/// completion, so there's nothing here to uncomplete — the user needs to unskip that
/// occurrence first (issues.md's still-open "unskip" item), not have this silently
/// walk further back to some earlier completion instead.
async fn require_cursor_occurrence(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    series: &ItemSeries,
    occurrence_date: DateTime<Utc>,
) -> Result<(), ItemError> {
    if series.item_type != ItemKind::Task {
        return Ok(());
    }
    let Some(cursor) = series.cursor_date else {
        return Err(ItemError::Invalid(
            "cannot uncomplete this occurrence — the series has no settled occurrence yet"
                .to_string(),
        ));
    };
    if let Some(occurrence) = series_repo.get_occurrence(&series.id, cursor).await?
        && occurrence.is_exdate
    {
        return Err(ItemError::Invalid(format!(
            "cannot uncomplete this occurrence — the series' most recently settled \
             occurrence ({cursor}) was skipped, not completed; unskip it before \
             uncompleting an earlier occurrence"
        )));
    }
    if occurrence_date != cursor {
        return Err(ItemError::Invalid(format!(
            "cannot uncomplete this occurrence out of order — the series' most recently \
             completed occurrence is {cursor}; occurrences must be uncompleted one at a \
             time, in order"
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
    series_repo: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    if let Some(occurrence) = series_repo.find_occurrence_by_item_id(item_id).await? {
        let series = series_repo.get_series(&occurrence.series_id).await?;
        require_current_occurrence(
            series_repo,
            &series,
            occurrence.occurrence_date,
            tz_offset_minutes,
        )
        .await?;
    }
    Ok(())
}

/// The Uncomplete-side counterpart to `validate_completable`, called from
/// `project_items::update_project_item` before it persists a `complete: false`
/// request on an item that's currently complete — see `require_cursor_occurrence`'s
/// doc comment for the rule it enforces. `record_task_uncompletion` below is this
/// function's post-persistence counterpart, mirroring `record_task_completion`.
pub async fn validate_uncompletable(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
) -> Result<(), ItemError> {
    if let Some(occurrence) = series_repo.find_occurrence_by_item_id(item_id).await? {
        let series = series_repo.get_series(&occurrence.series_id).await?;
        require_cursor_occurrence(series_repo, &series, occurrence.occurrence_date).await?;
    }
    Ok(())
}

/// Stage 6's original resolution of stage 3's deferred "what happens to a materialized
/// occurrence's item when it's skipped" question was to make item-delete double as Skip
/// for a materialized occurrence (mark it exdate). **2026-08-16, second pass:** reversed
/// that — deleting an item is not the same intent as explicitly skipping a series
/// occurrence (see `mark_exdate`'s doc comment: Skip is now the *only* path that sets
/// `is_exdate`), so this un-materializes the occurrence instead, by deleting its
/// `item_occurrences` row outright rather than marking it excluded. The date goes back to
/// being a plain virtual occurrence — re-materializable, and if it happened to be the
/// series' current occurrence, it's simply current-and-itemless again rather than
/// current-and-permanently-stuck.
///
/// That reversal is also what fixes the real bug that motivated this: a series whose
/// *current* occurrence's item gets deleted used to leave `cursor_date` untouched behind
/// a now-exdate'd date, and since Stage 10a made settling strictly one-at-a-time/in-order,
/// the series got permanently stuck believing that dead date was still current — every
/// later occurrence, even ones already materialized and worked on, became uncompletable
/// with "cannot settle this occurrence out of order". (An earlier same-day fix patched
/// this by conditionally advancing the cursor at delete time — since removed, because
/// un-materializing needs no such special case: `current_occurrence_date` is derived
/// purely from `cursor_date`/`anchor_date`, never from `item_occurrences` rows, so leaving
/// the cursor untouched and just deleting the row already produces the right outcome —
/// the same date stays current, just re-materializable instead of dead.) Real-world case:
/// a family's daily dog-walk/poop-pickup series each had their very first materialized
/// occurrence's item deleted early on; both series sat frozen on that stale date for six
/// days until someone tried to complete today's and got rejected.
///
/// Called from `project_items::delete_project_item` itself, after every item delete, not
/// from a series-specific route — a materialized occurrence's item has no visible marker
/// distinguishing it from an ordinary Event, and there's still no dedicated
/// materialized-occurrence "un-skip"/delete UI, so plain item-delete is the mechanism. A
/// `None` result (the overwhelmingly common case — most deleted items never came from a
/// series) is a normal, cheap no-op.
pub async fn unlink_deleted_item_occurrence(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
) -> Result<(), ItemError> {
    if let Some(occurrence) = series_repo.find_occurrence_by_item_id(item_id).await? {
        series_repo
            .delete_occurrence(&occurrence.series_id, occurrence.occurrence_date)
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

/// Whether `series` materializes each occurrence with the occurrence date written to
/// the item's `due_date` (and `has_due_time`) instead of `scheduled_date` — see
/// `ItemSeries::basis`'s doc comment and `get_or_materialize_occurrence`. Orthogonal to
/// `is_completion_basis`: this only changes which field a materialized occurrence's date
/// lands on, not how the cursor advances (a due-date-basis series still advances on the
/// fixed schedule, same as the default).
pub fn is_due_date_basis(series: &ItemSeries) -> bool {
    series.basis.as_deref() == Some("DUE_DATE")
}

/// Stage 2 of docs/assignment-rotation-plan.md: index of `occurrence_date` within
/// `rule`'s sequence starting at `anchor`, 0-based. `occurrence_date` is always itself
/// a member of that sequence (every caller derives it from the same rule/anchor), so
/// the `occurrences_between` count this reads is never empty in practice. O(occurrences
/// so far) rather than O(1) — accepted tradeoff, see the design doc's "Known accepted
/// tradeoff" note.
fn occurrence_index(
    rule: &recurrence::RecurrenceRule,
    anchor: DateTime<Utc>,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> usize {
    recurrence::occurrences_between(rule, anchor, anchor, occurrence_date, tz_offset_minutes).len()
        - 1
}

/// Pure: same inputs always produce the same assignee, regardless of materialization
/// order, skip history, or which occurrence is touched first — no stored "whose turn"
/// cursor, per docs/assignment-rotation-plan.md's stateless-rotation decision.
fn rotation_assignee(rotation: &[String], index: usize) -> Option<&String> {
    if rotation.is_empty() {
        None
    } else {
        Some(&rotation[index % rotation.len()])
    }
}

/// Stage 2: resolves a materializing occurrence's assignee — the series' fixed
/// `assigned_to_user_id` if set, otherwise (when the series is rotating) whichever
/// rotation member is up for `occurrence_date`, computed fresh via `occurrence_index`/
/// `rotation_assignee`. The rotation-membership query only runs on the "series has no
/// fixed assignee" path, since a fixed-assignee series never has rotation members to
/// begin with (`resolve_series_assignment` enforces the two are mutually exclusive).
///
/// `pub(crate)` as of Stage 4 (`docs/assignment-rotation-plan.md`) — also called
/// directly by `project_tasks::handlers` to resolve a still-virtual occurrence's
/// preview/edit-form assignee before it's materialized, not just at materialization
/// time itself.
pub(crate) async fn resolve_occurrence_assignee(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    series: &ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<Option<String>, ItemError> {
    if series.assigned_to_user_id.is_some() {
        return Ok(series.assigned_to_user_id.clone());
    }
    let rotation = series_repo.list_rotation_members(&series.id).await?;
    if rotation.is_empty() {
        return Ok(None);
    }
    let rule = recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
    let index = occurrence_index(
        &rule,
        series.anchor_date,
        occurrence_date,
        tz_offset_minutes,
    );
    Ok(rotation_assignee(&rotation, index).cloned())
}

/// Stage 4 of docs/assignment-rotation-plan.md — resolves the series row/list view's
/// "who's up now" display (open question 4, resolved 2026-08-20): the series' fixed
/// assignee if any, otherwise (for a rotating Task series) the *current* occurrence's
/// resolved rotation assignee — the same "next thing to work on" occurrence
/// `current_occurrence_date` already surfaces elsewhere, not the whole rotation set and
/// not the next occurrence. An Event-typed series can never rotate (`resolve_series_
/// assignment`'s Task-only gate), so this falls straight through to the plain
/// `assigned_to_user_id` for anything but a Task series, identical to today's display.
pub async fn current_series_assignee(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    series: &ItemSeries,
    tz_offset_minutes: i32,
) -> Result<Option<String>, ItemError> {
    if series.item_type != ItemKind::Task || series.assigned_to_user_id.is_some() {
        return Ok(series.assigned_to_user_id.clone());
    }
    let rule = recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
    let occurrence_date = current_occurrence_date(series, &rule, tz_offset_minutes);
    resolve_occurrence_assignee(series_repo, series, occurrence_date, tz_offset_minutes).await
}

/// Stage 10 gap 1: the date to advance a Task-typed series' cursor to when settling
/// (completing or skipping) `occurrence_date` — `Utc::now()` for a completion-basis
/// series (measuring the next occurrence from when it was actually settled, not its
/// nominal date), otherwise `occurrence_date` itself (today's only behavior,
/// unchanged). Shared by `record_task_completion` and `skip_occurrence` so Complete
/// and Skip stay symmetric, matching Stage 9's existing "settling is settling" design.
fn cursor_value_for_settlement(
    series: &ItemSeries,
    occurrence_date: DateTime<Utc>,
) -> DateTime<Utc> {
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
    series_repo: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
) -> Result<(), ItemError> {
    if let Some(occurrence) = series_repo.find_occurrence_by_item_id(item_id).await? {
        let series = series_repo.get_series(&occurrence.series_id).await?;
        if series.item_type == ItemKind::Task {
            let cursor_value = cursor_value_for_settlement(&series, occurrence.occurrence_date);
            series_repo
                .advance_cursor(&occurrence.series_id, cursor_value)
                .await?;
        }
    }
    Ok(())
}

/// Post-persistence counterpart to `record_task_completion`, called after a successful
/// `complete: false` update. `validate_uncompletable` has already rejected anything but
/// the series' most recently settled occurrence by the time this runs, so restoring the
/// cursor is always exactly one step back: `retreat_once`, computed from the occurrence's
/// own nominal date (not `cursor_value_for_settlement`/`Utc::now()` — a completion-basis
/// series' cursor value at settlement time is unrecoverable, but it doesn't matter here,
/// since any cursor value X with `advance_once(X) == occurrence_date` makes the just-
/// uncompleted occurrence current again, and `retreat_once` gives exactly that X by
/// construction). The one exception is uncompleting the series' very first (anchor)
/// occurrence — there's no earlier occurrence for `retreat_once` to land on, so the
/// cursor goes back to its pre-anything-settled `None` state instead.
pub async fn record_task_uncompletion(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
    tz_offset_minutes: i32,
) -> Result<(), ItemError> {
    if let Some(occurrence) = series_repo.find_occurrence_by_item_id(item_id).await? {
        let series = series_repo.get_series(&occurrence.series_id).await?;
        if series.item_type == ItemKind::Task {
            if occurrence.occurrence_date == series.anchor_date {
                series_repo
                    .clear_cursor(&series.id, occurrence.occurrence_date)
                    .await?;
            } else {
                let rule = recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
                let previous =
                    recurrence::retreat_once(&rule, occurrence.occurrence_date, tz_offset_minutes);
                series_repo.retreat_cursor(&series.id, previous).await?;
            }
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
    /// Stage 2 of docs/assignment-rotation-plan.md — mutually exclusive with
    /// `assigned_to_user_id`, validated in `resolve_series_assignment`.
    pub rotation_user_ids: Option<Vec<String>>,
    pub points: Option<i32>,
    pub priority: Option<i32>,
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
fn validate_series_basis(
    item_type: ItemKind,
    basis: &Option<String>,
    recurrence: &str,
) -> Result<(), ItemError> {
    match basis.as_deref() {
        Some("COMPLETION") => {
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
        Some("DUE_DATE") => {
            if item_type != ItemKind::Task {
                return Err(ItemError::Invalid(
                    "basis: DUE_DATE is only valid on a TASK series".to_string(),
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
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

/// Task-only, same shape as `validate_series_item_type`'s own rejection — but unlike
/// `resolve_series_assignment` (points/assignment), never gated on a team-backed
/// project or project-admin authority. Mirrors `Item::validate`'s own 1–4 range check
/// (root CLAUDE.md's Priority section).
fn validate_series_priority(item_type: ItemKind, priority: Option<i32>) -> Result<(), ItemError> {
    if let Some(p) = priority {
        if item_type != ItemKind::Task {
            return Err(ItemError::Invalid(
                "priority is only valid on a TASK series".to_string(),
            ));
        }
        if !(1..=4).contains(&p) {
            return Err(ItemError::Invalid(
                "priority must be between 1 and 4".to_string(),
            ));
        }
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
    let item = repo
        .get_by_project(project_id, id)
        .await
        .map_err(ItemError::from)?;
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
///
/// Stage 2 of docs/assignment-rotation-plan.md: `rotation_user_ids` is the rotating
/// alternative to `assigned_to_user_id` — the two are mutually exclusive (setting one
/// clears the other), validated here rather than schema-side (Smithy has no clean
/// "exactly one of" constraint). Each rotation member is validated via
/// `resolve_project_assignee` exactly like the fixed case, one at a time (there's no
/// bulk variant — a rotation is realistically a handful of people, not worth a new
/// query shape). An explicitly-provided but empty list is rejected rather than treated
/// as "no rotation" — ambiguous with "clear it", same precedent as the item_type/
/// event_type either-or checks elsewhere in this file.
async fn resolve_series_assignment(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    item_type: ItemKind,
    assigned_to_user_id: Option<String>,
    rotation_user_ids: Option<Vec<String>>,
    points: Option<i32>,
) -> Result<(Option<String>, Vec<String>, Option<i32>), ItemError> {
    if assigned_to_user_id.is_some() && rotation_user_ids.is_some() {
        return Err(ItemError::Invalid(
            "assignedToUserId and rotationUserIds are mutually exclusive".to_string(),
        ));
    }
    if let Some(ids) = &rotation_user_ids
        && ids.is_empty()
    {
        return Err(ItemError::Invalid(
            "rotationUserIds cannot be explicitly empty — omit it to clear the rotation"
                .to_string(),
        ));
    }
    if assigned_to_user_id.is_none() && rotation_user_ids.is_none() && points.is_none() {
        return Ok((None, Vec::new(), None));
    }
    if item_type != ItemKind::Task {
        return Err(ItemError::Invalid(
            "assignedToUserId/rotationUserIds/points are only valid on a TASK series".to_string(),
        ));
    }
    let project = projects.get(project_id).await?;
    if project.team_id.is_none() {
        return Err(ItemError::Invalid(
            "assignedToUserId/rotationUserIds/points require a team-backed project".to_string(),
        ));
    }
    let resolved_assignee =
        resolve_project_assignee(projects, project_id, assigned_to_user_id).await?;
    let mut resolved_rotation = Vec::new();
    for user_id in rotation_user_ids.into_iter().flatten() {
        // `resolve_project_assignee` only returns `None` when given `None` — we always
        // pass `Some`, so the result is always `Some` too.
        if let Some(resolved) =
            resolve_project_assignee(projects, project_id, Some(user_id)).await?
        {
            resolved_rotation.push(resolved);
        }
    }
    let resolved_points = if points.is_some()
        && require_project_admin(projects, teams, project_id, requester_user_id)
            .await
            .is_ok()
    {
        points
    } else {
        None
    };
    Ok((resolved_assignee, resolved_rotation, resolved_points))
}

pub async fn create_series(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    series_repo: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    params: CreateItemSeriesParams,
) -> Result<String, ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    validate_series_item_type(params.item_type)?;
    validate_series_event_type(&params.event_type)?;
    validate_series_basis(params.item_type, &params.basis, &params.recurrence)?;
    validate_series_priority(params.item_type, params.priority)?;
    validate_series_template_item(
        repo,
        &params.project_id,
        params.item_type,
        &params.template_item_id,
    )
    .await?;
    let (assigned_to_user_id, rotation_user_ids, points) = resolve_series_assignment(
        projects,
        teams,
        &params.project_id,
        requester_user_id,
        params.item_type,
        params.assigned_to_user_id,
        params.rotation_user_ids,
        params.points,
    )
    .await?;
    let series_id = series_repo
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
            priority: params.priority,
        })
        .await?;
    if !rotation_user_ids.is_empty() {
        series_repo
            .set_rotation_members(&series_id, &rotation_user_ids)
            .await?;
    }
    Ok(series_id)
}

pub async fn get_series(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    item_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
) -> Result<ItemSeries, ItemError> {
    let series = item_series.get_series(series_id).await?;
    require_project_member(projects, teams, &series.project_id, requester_user_id).await?;
    Ok(series)
}

pub async fn duplicate_series(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    item_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
) -> Result<(), ItemError> {
    let mut series = item_series.get_series(series_id).await?;
    require_project_member(projects, teams, &series.project_id, requester_user_id).await?;
    series.name = format!("{} (copy)", series.name);
    let new_series_id = item_series.create_series(&series).await?;
    let rotation = item_series.list_rotation_members(series_id).await?;
    if !rotation.is_empty() {
        item_series
            .set_rotation_members(&new_series_id, &rotation)
            .await?;
    }
    Ok(())
}

/// Orphan, not cascade — deletes the series and its `item_occurrences` rows only, never
/// touches `items`. Every already-materialized occurrence survives as a plain standalone
/// item, matching `unlink_source_event_tasks`'s precedent for an independent dependent.
/// See item_series.smithy's `DeleteItemSeries` doc comment. Gated by project membership,
/// same authority level as create/update/list above (a series is project-scoped content
/// like a template, not a role/points-authority action).
pub async fn delete_series(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    series_repo: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
) -> Result<(), ItemError> {
    let series = series_repo.get_series(series_id).await?;
    require_project_member(projects, teams, &series.project_id, requester_user_id).await?;
    series_repo.delete_series(series_id).await?;
    Ok(())
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
    /// Stage 2 of docs/assignment-rotation-plan.md — mutually exclusive with
    /// `assigned_to_user_id`, validated in `resolve_series_assignment`.
    pub rotation_user_ids: Option<Vec<String>>,
    pub points: Option<i32>,
    pub priority: Option<i32>,
}

pub async fn update_series(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    series_repo: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
    params: UpdateItemSeriesParams,
) -> Result<(), ItemError> {
    let current = series_repo.get_series(series_id).await?;
    require_project_member(projects, teams, &current.project_id, requester_user_id).await?;
    validate_series_item_type(params.item_type)?;
    validate_series_event_type(&params.event_type)?;
    validate_series_basis(params.item_type, &params.basis, &params.recurrence)?;
    validate_series_priority(params.item_type, params.priority)?;
    validate_series_template_item(
        repo,
        &current.project_id,
        params.item_type,
        &params.template_item_id,
    )
    .await?;
    let (assigned_to_user_id, rotation_user_ids, points) = resolve_series_assignment(
        projects,
        teams,
        &current.project_id,
        requester_user_id,
        params.item_type,
        params.assigned_to_user_id,
        params.rotation_user_ids,
        params.points,
    )
    .await?;
    series_repo
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
                priority: params.priority,
            },
        )
        .await?;
    // Same round-trip convention as assigned_to_user_id/points above — a full-replace
    // write regardless of whether rotation_user_ids is empty, so switching a series
    // from rotating back to fixed (or to neither) actually clears its prior members
    // rather than leaving them stranded in the join table.
    series_repo
        .set_rotation_members(series_id, &rotation_user_ids)
        .await?;
    Ok(())
}

pub async fn list_series_for_project(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    series_repo: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    project_id: &str,
) -> Result<Vec<ItemSeries>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(series_repo.list_series_for_project(project_id).await?)
}

/// Stage 5 of docs/recurring-events-virtual-occurrences-rough-plan.md, superseded by Stage B
/// of `docs/unify-virtual-materialized-occurrences-plan.md` — every `web_ui` call site that
/// used to call a since-removed `list_virtual_occurrences_for_project_unchecked` (which
/// dropped any date that already had an `item_occurrences` row, materialized or exdate) now
/// calls this instead, so a `Materialized`/`Skipped` date is classified rather than silently
/// excluded. This is the single data source unified (virtual-looks-like-materialized) row/
/// calendar rendering builds on, replacing the old per-screen pattern of querying materialized
/// items and virtual occurrences separately and merging them by hand.
///
/// Returns only `item_id` for a materialized date, not a full `Item` — callers batch-fetch
/// real items via their own existing `list_by_project`/`list_due_by_project` calls rather
/// than this function duplicating that fetch.
///
/// New and not yet called from any `web_ui` code — Stage A is additive only, existing
/// screens are untouched until Stage D wires them onto this instead.
#[derive(Debug, Clone, PartialEq)]
pub enum OccurrenceState {
    Materialized { item_id: String },
    Skipped,
    Virtual,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectOccurrence {
    pub series_id: String,
    pub series_name: String,
    pub item_type: ItemKind,
    pub event_type: Option<String>,
    pub occurrence_date: DateTime<Utc>,
    pub is_current: bool,
    pub assigned_to_user_id: Option<String>,
    pub assigned_to_user_name: Option<String>,
    pub state: OccurrenceState,
    /// Mirrors `is_due_date_basis(series)` at the time this occurrence was listed — lets a
    /// still-virtual occurrence's row render its date the same way a materialized one from the
    /// same series would (💀 due-date vs 📅 scheduled-date icon/overdue styling), instead of
    /// every virtual row rendering as a generic undated "Due:" label regardless of the series'
    /// actual basis.
    pub is_due_date_basis: bool,
    /// The series' own `priority` — sourced here (not re-derived by callers) so a
    /// still-virtual/skipped occurrence sorts alongside real items the same way its
    /// eventual materialization would. See root CLAUDE.md's Priority section.
    pub priority: Option<i32>,
}

impl ProjectOccurrence {
    pub fn is_skipped(&self) -> bool {
        matches!(self.state, OccurrenceState::Skipped)
    }

    /// `GET`s the no-side-effect read-only view for a still-virtual/skipped occurrence (see
    /// `project_item_series::handlers::project_item_series_occurrence_detail_page`), and is
    /// the base path every mutation route below nests under. The name predates Stage C, which
    /// changed the `GET` itself to no longer materialize on load — kept for the URL segment
    /// stability, not because a plain `GET` here still creates anything.
    pub fn materialize_url(&self, project_id: &str) -> String {
        format!(
            "/web/projects/{project_id}/series/{}/occurrences/{}",
            self.series_id,
            self.occurrence_date.timestamp(),
        )
    }

    pub fn skip_url(&self, project_id: &str) -> String {
        format!("{}/skip", self.materialize_url(project_id))
    }

    pub fn unskip_url(&self, project_id: &str) -> String {
        format!("{}/unskip", self.materialize_url(project_id))
    }

    /// Row-checkbox target for a still-virtual/skipped Task occurrence — materializes and
    /// completes in one POST (`complete_project_item_series_occurrence_form`), the same
    /// composition `update_project_task_series_occurrence_form` already uses for the detail
    /// page's checkbox. Meaningless for an Event-typed series (no completion concept) — never
    /// rendered there.
    pub fn complete_url(&self, project_id: &str) -> String {
        format!("{}/complete", self.materialize_url(project_id))
    }

    /// Row-actions-menu "Edit" target for a still-virtual/skipped Task occurrence — the no-
    /// side-effect edit form (`project_item_series_occurrence_edit_page`), prefilled from the
    /// series rather than a real `Item`. Docs/issues_and_features.md's "all row actions should
    /// be available and will auto-materialize a virtual row if taken" item: this exposes the
    /// same edit form the Details dialog's own Edit button already links to, directly from the
    /// row's "⋮" menu, so a full round trip through Details is no longer required just to edit.
    pub fn edit_url(&self, project_id: &str) -> String {
        format!("{}/edit", self.materialize_url(project_id))
    }

    /// Row-actions-menu "Add sub-item" target — `GET` opens a materialize-on-save dialog
    /// (`get_project_task_series_occurrence_add_child_dialog`), whose form posts to this same
    /// path (`create_project_task_series_occurrence_child_form`, already existed but had no UI
    /// entry point before this). Task-typed series only, mirroring `complete_url`.
    pub fn add_task_child_url(&self, project_id: &str) -> String {
        format!("{}/task-children", self.materialize_url(project_id))
    }
}

/// Skip-button URL for an already-materialized series occurrence's own item row — Tasks and
/// Events list rows both need this, since `skip_or_delete_series_occurrence` already works
/// identically whether `occurrence_date` is still virtual or already materialized (see its doc
/// comment): it deletes the item first, then marks exdate. Uses the occurrence's true stored
/// date via the `item_occurrences` reverse lookup rather than `item.due_date()`/
/// `scheduled_date()`, which can drift from it after a plain edit of the materialized item —
/// using a stale date here would silently target the wrong `(series_id, occurrence_date)` row.
/// `None` for an item never materialized from a series.
pub async fn skip_url_for_item(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    item: &Item,
    project_id: &str,
) -> Result<Option<String>, ItemError> {
    let Some(series_id) = &item.series_id else {
        return Ok(None);
    };
    let occurrence = series_repo.find_occurrence_by_item_id(&item.id).await?;
    Ok(occurrence.map(|o| {
        format!(
            "/web/projects/{project_id}/series/{series_id}/occurrences/{}/skip",
            o.occurrence_date.timestamp(),
        )
    }))
}

pub async fn list_occurrence_states_for_project(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    users: &Arc<dyn UserRepo>,
    project_id: &str,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<Vec<ProjectOccurrence>, ItemError> {
    let now = Utc::now();
    let all_series = series_repo.list_series_for_project(project_id).await?;
    let mut result = Vec::new();
    let mut names: HashMap<String, String> = HashMap::new();
    for series in &all_series {
        let Ok(rule) = recurrence::parse(&series.recurrence) else {
            continue;
        };
        // Stage 2 of docs/assignment-rotation-plan.md: a rotating series (no fixed
        // assignee, but has rotation members) can't resolve one assignee/name for the
        // whole series up front — each occurrence's assignee depends on its own
        // calendar position, so that resolution moves inside the per-date loop below.
        // Only queried on the "no fixed assignee" path, same as
        // `resolve_occurrence_assignee`.
        let rotation_members = if series.assigned_to_user_id.is_none() {
            series_repo.list_rotation_members(&series.id).await?
        } else {
            Vec::new()
        };

        // Stage 10 gap 1: the predicted list is normally rooted at the series' own
        // anchor_date, but for a completion-basis Task series that drifts silently wrong
        // after the first off-schedule settlement — rooting at current_occurrence_date
        // instead self-corrects every render, since nothing here is cached.
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
        let current_outside_window = series.item_type == ItemKind::Task
            && !(range_start..=range_end).contains(&current_date);
        if current_outside_window {
            candidates.push(current_date);
        }
        if candidates.is_empty() {
            continue;
        }
        let query_start = range_start.min(current_date);
        let query_end = range_end.max(current_date);
        let existing = series_repo
            .list_occurrences_between(&series.id, query_start, query_end)
            .await?;
        let existing_by_ts: HashMap<i64, &ItemOccurrence> = existing
            .iter()
            .map(|o| (o.occurrence_date.timestamp(), o))
            .collect();
        for date in candidates {
            let is_current = series.item_type == ItemKind::Task && date == current_date;
            let existing_occ = existing_by_ts.get(&date.timestamp());
            // The same past-date clamp Stage 9 introduced (Task-typed occurrences before
            // `now` are dropped, with the series' own current occurrence exempted), but a
            // past date that's already settled (materialized or skipped) is real state and
            // still surfaces — only a purely virtual past date (nothing ever happened) is
            // dropped, since it was never actionable and is not history.
            if series.item_type == ItemKind::Task
                && date < now
                && !is_current
                && existing_occ.is_none()
            {
                continue;
            }
            let state = match existing_occ {
                Some(occ) if occ.is_exdate => OccurrenceState::Skipped,
                // ItemOccurrence's invariant (see its own doc comment): the only two writes
                // into this table never produce (item_id: None, is_exdate: false), so a
                // non-exdate row always has an item_id.
                Some(occ) => OccurrenceState::Materialized {
                    item_id: occ.item_id.clone().unwrap_or_default(),
                },
                None => OccurrenceState::Virtual,
            };
            // Fixed assignee if the series has one; otherwise, for a rotating series,
            // whichever member is up for this occurrence's own calendar position — the
            // index is always measured from `series.anchor_date`, per
            // `resolve_occurrence_assignee`, not `root_date` (which can be
            // `current_date` for a completion-basis series).
            let occurrence_assignee = if let Some(user_id) = &series.assigned_to_user_id {
                Some(user_id.clone())
            } else if !rotation_members.is_empty() {
                let index = occurrence_index(&rule, series.anchor_date, date, tz_offset_minutes);
                rotation_assignee(&rotation_members, index).cloned()
            } else {
                None
            };
            // Full "First Last" name, matching `project_tasks::names_for`'s own format — a
            // materialized occurrence's assignee is resolved through that map, so a still-
            // virtual/skipped occurrence rendering only a first name here was exactly the kind
            // of drift docs/issues_and_features.md's "Virtual rows look different from
            // materialized ones" item called out.
            let occurrence_assignee_name = match &occurrence_assignee {
                None => None,
                Some(user_id) => match names.get(user_id) {
                    Some(name) => Some(name.clone()),
                    None => {
                        let user = users
                            .get(user_id)
                            .await
                            .map_err(|_| ItemError::Internal("error fetching user".to_string()))?;
                        let full_name = format!("{} {}", user.first_name, user.last_name);
                        names.insert(user_id.clone(), full_name.clone());
                        Some(full_name)
                    }
                },
            };
            result.push(ProjectOccurrence {
                series_id: series.id.clone(),
                series_name: series.name.clone(),
                item_type: series.item_type,
                event_type: series.event_type.clone(),
                occurrence_date: date,
                is_current,
                assigned_to_user_id: occurrence_assignee,
                assigned_to_user_name: occurrence_assignee_name,
                state,
                is_due_date_basis: is_due_date_basis(series),
                priority: series.priority,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item_series::ItemSeries;
    use crate::domain::project::Project;
    use crate::storage::sqlite::{
        MockItemDependencyRepo, MockItemRepo, MockItemSeriesRepo, MockProjectRepo,
        MockReminderRepo, MockTeamRepo, MockUserRepo, RepoError,
    };

    /// `get_or_materialize_occurrence`/`skip_or_delete_series_occurrence` both resync/clear
    /// reminders as part of the `project_items::create_project_item`/`delete_project_item`
    /// funnel they delegate into — a harmless no-op stub for tests that don't care about
    /// reminder rows.
    fn no_op_reminders() -> Arc<dyn ReminderRepo> {
        let mut mock = MockReminderRepo::new();
        mock.expect_sync_auto_reminders()
            .returning(|_, _, _, _| Ok(()));
        mock.expect_delete_for_item().returning(|_| Ok(()));
        Arc::new(mock)
    }

    fn no_op_item_dependencies() -> Arc<dyn ItemDependencyRepo> {
        let mut mock = MockItemDependencyRepo::new();
        mock.expect_delete_for_item().returning(|_| Ok(()));
        Arc::new(mock)
    }

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
            priority: None,
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

    #[test]
    fn occurrence_index_computes_zero_based_position_in_the_sequence() {
        let rule = recurrence::parse("every 7 days").unwrap();
        let anchor = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(occurrence_index(&rule, anchor, anchor, 0), 0);
        assert_eq!(
            occurrence_index(&rule, anchor, anchor + chrono::Duration::days(7), 0),
            1
        );
        assert_eq!(
            occurrence_index(&rule, anchor, anchor + chrono::Duration::days(21), 0),
            3
        );
    }

    #[test]
    fn rotation_assignee_cycles_through_the_list() {
        let rotation = vec!["alice".to_string(), "bob".to_string(), "carol".to_string()];
        assert_eq!(rotation_assignee(&rotation, 0), Some(&"alice".to_string()));
        assert_eq!(rotation_assignee(&rotation, 1), Some(&"bob".to_string()));
        assert_eq!(rotation_assignee(&rotation, 2), Some(&"carol".to_string()));
        // Wraps back around past the end of the list.
        assert_eq!(rotation_assignee(&rotation, 3), Some(&"alice".to_string()));
        assert_eq!(rotation_assignee(&rotation, 4), Some(&"bob".to_string()));
    }

    #[test]
    fn rotation_assignee_is_none_for_an_empty_rotation() {
        assert_eq!(rotation_assignee(&[], 0), None);
    }

    #[tokio::test]
    async fn returns_existing_item_when_already_materialized() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: Some("existing-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_record_materialized_occurrence().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock.expect_create().times(0);
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| {
                project_id == "p1" && item_id == "existing-item"
            })
            .returning(|_, _| Ok(Item::new_project_item("p1", "Standup")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &series_repo,
            &no_op_reminders(),
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
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        series_mock
            .expect_record_materialized_occurrence()
            .withf(|series_id: &str, _date, item_id: &str| {
                series_id == "s1" && item_id == "new-item-id"
            })
            .times(1)
            .returning(|_, _, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

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
            .withf(|item: &Item| {
                item.kind() == ItemKind::Event
                    && item.scheduled_date() == Some(occurrence_date())
                    && item.has_scheduled_time()
                    && item.series_id == Some("s1".to_string())
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
            &series_repo,
            &no_op_reminders(),
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
    async fn materializes_a_due_date_basis_task_onto_due_date_not_scheduled_date() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.basis = Some("DUE_DATE".to_string());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

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
            .withf(|item: &Item| {
                item.due_date() == Some(occurrence_date())
                    && item.scheduled_date().is_none()
                    && item.has_due_time()
            })
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
            &series_repo,
            &no_op_reminders(),
            "owner1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should materialize a due-date-basis task occurrence");

        assert_eq!(item.name, "Standup");
    }

    #[tokio::test]
    async fn materializes_a_task_when_series_is_task_typed() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

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
            &series_repo,
            &no_op_reminders(),
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
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        projects_mock
            .expect_find_personal_project()
            .returning(|_| Ok(None));
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
            &series_repo,
            &no_op_reminders(),
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
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &series_repo,
            &no_op_reminders(),
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
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &series_repo,
            &no_op_reminders(),
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
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
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
            &series_repo,
            &no_op_reminders(),
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
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_mark_exdate()
            .withf(|series_id: &str, _date| series_id == "s1")
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&series_repo, "s1", occurrence_date(), 0)
            .await
            .expect("should mark the occurrence as skipped");
    }

    #[tokio::test]
    async fn skip_occurrence_does_not_advance_cursor_for_an_event_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_mark_exdate().returning(|_, _| Ok(()));
        series_mock.expect_advance_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&series_repo, "s1", occurrence_date(), 0)
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
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        // No exdate row at the current date to self-heal past (2026-08-16 fix).
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock.expect_mark_exdate().returning(|_, _| Ok(()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == occurrence_date()
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&series_repo, "s1", occurrence_date(), 0)
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
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        // No exdate row at the current date to self-heal past (2026-08-16 fix).
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
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
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&series_repo, "s1", occurrence_date(), 0)
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
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        // No exdate row at the (actual) current date to self-heal past (2026-08-16 fix).
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock.expect_mark_exdate().times(0);
        series_mock.expect_advance_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = skip_occurrence(&series_repo, "s1", occurrence_date(), 0).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn skip_or_delete_series_occurrence_deletes_materialized_item_before_marking_exdate() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: Some("materialized-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock
            .expect_mark_exdate()
            .times(1)
            .returning(|_, _| Ok(()));
        // Event-typed series (default from `series()`) — no cursor to advance.
        series_mock.expect_advance_cursor().times(0);
        // `unlink_deleted_item_occurrence`, called from inside `delete_project_item`.
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|item_id| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some(item_id.to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_delete_occurrence()
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

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

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        skip_or_delete_series_occurrence(
            &repo,
            &projects,
            &teams,
            &series_repo,
            &no_op_reminders(),
            &no_op_item_dependencies(),
            "owner1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should delete the materialized item, then mark exdate");
    }

    #[tokio::test]
    async fn skip_or_delete_series_occurrence_skips_delete_when_still_virtual() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_mark_exdate()
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        // No expectations set on these mocks at all — mockall panics on any unmocked
        // call, so this asserts `delete_project_item`'s machinery is never reached when
        // the occurrence has no materialized item behind it.
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        skip_or_delete_series_occurrence(
            &repo,
            &projects,
            &teams,
            &series_repo,
            &no_op_reminders(),
            &no_op_item_dependencies(),
            "owner1",
            "s1",
            occurrence_date(),
            0,
        )
        .await
        .expect("should mark exdate without touching the delete path");
    }

    #[tokio::test]
    async fn unskip_occurrence_rejects_when_not_marked_exdate() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock.expect_delete_occurrence().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = unskip_occurrence(&series_repo, "s1", occurrence_date(), 0).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn unskip_occurrence_is_unconditional_for_an_event_typed_series() {
        // `series("p1")` defaults to ItemKind::Event — no cursor concept.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: None,
                is_exdate: true,
            }))
        });
        series_mock.expect_retreat_cursor().times(0);
        series_mock.expect_clear_cursor().times(0);
        series_mock
            .expect_delete_occurrence()
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unskip_occurrence(&series_repo, "s1", occurrence_date(), 0)
            .await
            .expect("should unskip unconditionally for an Event-typed series");
    }

    #[tokio::test]
    async fn unskip_occurrence_rejects_out_of_order_for_a_task_series() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // cursor_date left at None (nothing settled yet) — occurrence_date() can never
        // equal it, so this is out of order regardless of what settled it.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: None,
                is_exdate: true,
            }))
        });
        series_mock.expect_delete_occurrence().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = unskip_occurrence(&series_repo, "s1", occurrence_date(), 0).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn unskip_occurrence_retreats_cursor_for_a_non_anchor_task_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.cursor_date = Some(occurrence_date());
        // anchor_date differs from occurrence_date(), so this isn't the anchor case.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: None,
                is_exdate: true,
            }))
        });
        let rule = recurrence::parse(&series("p1").recurrence).unwrap();
        let expected_previous = recurrence::retreat_once(&rule, occurrence_date(), 0);
        series_mock
            .expect_retreat_cursor()
            .withf(move |series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == expected_previous
            })
            .times(1)
            .returning(|_, _| Ok(()));
        series_mock.expect_clear_cursor().times(0);
        series_mock
            .expect_delete_occurrence()
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unskip_occurrence(&series_repo, "s1", occurrence_date(), 0)
            .await
            .expect("should retreat the cursor and delete the exdate row");
    }

    #[tokio::test]
    async fn unskip_occurrence_clears_cursor_when_unskipping_the_anchor_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.anchor_date = occurrence_date();
        task_series.cursor_date = Some(occurrence_date());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: None,
                is_exdate: true,
            }))
        });
        series_mock
            .expect_clear_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == occurrence_date()
            })
            .times(1)
            .returning(|_, _| Ok(()));
        series_mock.expect_retreat_cursor().times(0);
        series_mock
            .expect_delete_occurrence()
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unskip_occurrence(&series_repo, "s1", occurrence_date(), 0)
            .await
            .expect("should clear the cursor and delete the exdate row");
    }

    #[tokio::test]
    async fn validate_completable_rejects_a_non_current_task_series_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    // Not the series' anchor/current date.
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        // No exdate row at the (actual) current date to self-heal past (2026-08-16 fix).
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = validate_completable(&series_repo, "completed-item", 0).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    /// 2026-08-16 fix: a "current" that's already marked exdate (out-of-band — e.g. a
    /// non-current materialized occurrence deleted before the cursor ever reached it)
    /// must not permanently wedge the series there. `require_current_occurrence` should
    /// walk forward past it, persisting the correction, and treat the next (non-exdate)
    /// date as current.
    #[tokio::test]
    async fn validate_completable_self_heals_past_an_exdate_current_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // "every 7 days" (series()'s recurrence) — anchor is exdate, one step later
        // (occurrence_date()) is the real, unsettled, completable occurrence.
        let stuck_anchor = occurrence_date() - chrono::Duration::days(7);
        task_series.anchor_date = stuck_anchor;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_get_occurrence()
            .returning(move |_, date| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: date,
                    item_id: None,
                    is_exdate: date == stuck_anchor,
                }))
            });
        series_mock
            .expect_advance_cursor()
            .withf(move |series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == stuck_anchor
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_completable(&series_repo, "completed-item", 0)
            .await
            .expect("should self-heal past the exdate anchor and allow the next occurrence");
    }

    #[tokio::test]
    async fn validate_completable_allows_the_current_task_series_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // A fresh series' current occurrence is its own anchor_date.
        task_series.anchor_date = occurrence_date();
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        // No exdate row at the current date to self-heal past (2026-08-16 fix).
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_completable(&series_repo, "completed-item", 0)
            .await
            .expect("the series' own current occurrence should be completable");
    }

    #[tokio::test]
    async fn validate_completable_allows_any_date_for_an_event_series() {
        // series("p1") defaults to ItemKind::Event — no cursor/current concept, so
        // any occurrence date is fine.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("some-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_completable(&series_repo, "some-item", 0)
            .await
            .expect("an Event-typed series has no current-occurrence restriction");
    }

    #[tokio::test]
    async fn validate_completable_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| Ok(None));
        series_mock.expect_get_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_completable(&series_repo, "some-task", 0)
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    #[tokio::test]
    async fn validate_uncompletable_allows_the_most_recently_completed_task_series_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.cursor_date = Some(occurrence_date());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: Some("completed-item".to_string()),
                is_exdate: false,
            }))
        });
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_uncompletable(&series_repo, "completed-item")
            .await
            .expect("the series' own most recently completed occurrence should be uncompletable");
    }

    #[tokio::test]
    async fn validate_uncompletable_rejects_an_occurrence_earlier_than_the_cursor() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // The cursor has already moved past occurrence_date() — some later occurrence
        // was completed/skipped after it.
        let cursor = occurrence_date() + chrono::Duration::days(7);
        task_series.cursor_date = Some(cursor);
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_get_occurrence()
            .returning(move |_, date| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: date,
                    item_id: Some("later-item".to_string()),
                    is_exdate: false,
                }))
            });
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = validate_uncompletable(&series_repo, "completed-item").await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn validate_uncompletable_rejects_when_the_cursor_occurrence_was_skipped_not_completed() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.cursor_date = Some(occurrence_date());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    // An earlier occurrence than the (skipped) cursor.
                    occurrence_date: occurrence_date() - chrono::Duration::days(7),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(ItemOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: None,
                is_exdate: true,
            }))
        });
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = validate_uncompletable(&series_repo, "completed-item").await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn validate_uncompletable_rejects_when_series_has_no_settled_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.cursor_date = None;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock.expect_get_occurrence().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = validate_uncompletable(&series_repo, "completed-item").await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn validate_uncompletable_allows_any_date_for_an_event_series() {
        // series("p1") defaults to ItemKind::Event — no cursor/current concept, so
        // any occurrence date is fine.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("some-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_uncompletable(&series_repo, "some-item")
            .await
            .expect("an Event-typed series has no cursor-occurrence restriction");
    }

    #[tokio::test]
    async fn validate_uncompletable_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| Ok(None));
        series_mock.expect_get_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        validate_uncompletable(&series_repo, "some-task")
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    #[tokio::test]
    async fn unlink_deleted_item_occurrence_un_materializes_when_item_came_from_a_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|item_id| {
                assert_eq!(item_id, "deleted-item");
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("deleted-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_delete_occurrence()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == occurrence_date()
            })
            .times(1)
            .returning(|_, _| Ok(()));
        series_mock.expect_mark_exdate().times(0);
        series_mock.expect_get_series().times(0);
        series_mock.expect_advance_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unlink_deleted_item_occurrence(&series_repo, "deleted-item")
            .await
            .expect("should un-materialize the occurrence");
    }

    #[tokio::test]
    async fn unlink_deleted_item_occurrence_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| Ok(None));
        series_mock.expect_delete_occurrence().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unlink_deleted_item_occurrence(&series_repo, "some-task")
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    /// 2026-08-16, second pass: deleting the item behind a Task series' *current*
    /// occurrence must leave `cursor_date` untouched — un-materializing needs no
    /// cursor special-case at all, since `current_occurrence_date` is derived purely
    /// from `cursor_date`/`anchor_date`, never from `item_occurrences` rows. The same
    /// date simply stays current, now itemless and re-materializable rather than
    /// stuck (see `unlink_deleted_item_occurrence`'s doc comment for the real bug this
    /// fixes, and why an earlier delete-time cursor-advance was removed as
    /// unnecessary once un-materializing replaced marking exdate).
    #[tokio::test]
    async fn unlink_deleted_item_occurrence_never_touches_the_cursor_even_for_the_current_task_occurrence()
     {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let current_date = task_series.anchor_date;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(move |item_id| {
                assert_eq!(item_id, "deleted-item");
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: current_date,
                    item_id: Some("deleted-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_delete_occurrence()
            .times(1)
            .returning(|_, _| Ok(()));
        series_mock.expect_get_series().times(0);
        series_mock.expect_advance_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        unlink_deleted_item_occurrence(&series_repo, "deleted-item")
            .await
            .expect("should un-materialize without touching the cursor");
    }

    #[tokio::test]
    async fn record_task_completion_advances_cursor_for_a_materialized_task_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|item_id| {
                assert_eq!(item_id, "completed-item");
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == occurrence_date()
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&series_repo, "completed-item")
            .await
            .expect("should advance the cursor");
    }

    #[tokio::test]
    async fn record_task_completion_advances_cursor_to_now_for_a_completion_basis_series() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.basis = Some("COMPLETION".to_string());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("completed-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && (Utc::now() - *date).num_seconds().abs() < 30
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&series_repo, "completed-item")
            .await
            .expect("should advance the cursor to roughly now");
    }

    #[tokio::test]
    async fn record_task_completion_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| Ok(None));
        series_mock.expect_get_series().times(0);
        series_mock.expect_advance_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&series_repo, "some-task")
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    #[tokio::test]
    async fn record_task_completion_does_not_advance_cursor_for_an_event_typed_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("some-event".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_advance_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_completion(&series_repo, "some-event")
            .await
            .expect("should no-op for an Event-typed series");
    }

    #[tokio::test]
    async fn record_task_uncompletion_retreats_cursor_to_the_previous_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // occurrence_date() != task_series.anchor_date, so this isn't the
        // uncomplete-the-anchor case.
        let rule = recurrence::parse(&task_series.recurrence).unwrap();
        let expected_previous = recurrence::retreat_once(&rule, occurrence_date(), 0);
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|item_id| {
                assert_eq!(item_id, "uncompleted-item");
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("uncompleted-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_retreat_cursor()
            .withf(move |series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == expected_previous
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_uncompletion(&series_repo, "uncompleted-item", 0)
            .await
            .expect("should retreat the cursor to the previous occurrence");
    }

    #[tokio::test]
    async fn record_task_uncompletion_clears_cursor_when_uncompleting_the_anchor_occurrence() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        task_series.anchor_date = occurrence_date();
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("uncompleted-item".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_clear_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| {
                series_id == "s1" && *date == occurrence_date()
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_uncompletion(&series_repo, "uncompleted-item", 0)
            .await
            .expect("should clear the cursor back to None");
    }

    #[tokio::test]
    async fn record_task_uncompletion_is_a_no_op_for_a_non_series_item() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| Ok(None));
        series_mock.expect_get_series().times(0);
        series_mock.expect_retreat_cursor().times(0);
        series_mock.expect_clear_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_uncompletion(&series_repo, "some-task", 0)
            .await
            .expect("should no-op for an item with no linked occurrence");
    }

    #[tokio::test]
    async fn record_task_uncompletion_does_not_touch_cursor_for_an_event_typed_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_find_occurrence_by_item_id()
            .returning(|_| {
                Ok(Some(ItemOccurrence {
                    series_id: "s1".to_string(),
                    occurrence_date: occurrence_date(),
                    item_id: Some("some-event".to_string()),
                    is_exdate: false,
                }))
            });
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_retreat_cursor().times(0);
        series_mock.expect_clear_cursor().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        record_task_uncompletion(&series_repo, "some-event", 0)
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
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let result = skip_occurrence(&series_repo, "bogus", occurrence_date(), 0).await;
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
            rotation_user_ids: None,
            points: None,
            priority: None,
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
            rotation_user_ids: None,
            points: None,
            priority: None,
        }
    }

    #[tokio::test]
    async fn create_series_creates_after_confirming_membership() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.project_id == "p1" && s.name == "Standup")
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let id = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
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
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());

        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "not-the-owner",
            create_params("p1"),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_series_creates_a_task_typed_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.item_type == ItemKind::Task)
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        let id = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await
        .expect("owner should be able to create a task-typed series");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_rejects_assignment_on_event_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.assigned_to_user_id = Some("member1".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_points_on_a_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.points = Some(10);
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_honors_assignment_and_points_for_a_team_project_admin() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
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
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.assigned_to_user_id = Some("member1".to_string());
        params.points = Some(10);
        let id = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "admin1",
            params,
        )
        .await
        .expect("admin should be able to set assignment and points");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_drops_points_but_keeps_assignment_for_a_non_admin() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| {
                s.assigned_to_user_id == Some("member1".to_string()) && s.points.is_none()
            })
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.assigned_to_user_id = Some("member1".to_string());
        params.points = Some(10);
        let id = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "member1",
            params,
        )
        .await
        .expect("non-admin member should still be able to set assignment");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_rejects_both_fixed_assignee_and_rotation() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.assigned_to_user_id = Some("member1".to_string());
        params.rotation_user_ids = Some(vec!["member1".to_string(), "member2".to_string()]);
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "member1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_an_explicitly_empty_rotation_list() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.rotation_user_ids = Some(vec![]);
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "member1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_a_rotation_member_who_is_not_a_project_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock.expect_member_role().returning(|_, user_id| {
            if user_id == "member1" {
                Ok(Some(crate::domain::team::TeamRole::Member))
            } else {
                Ok(None)
            }
        });
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.rotation_user_ids = Some(vec!["member1".to_string(), "stranger".to_string()]);
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "member1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_sets_rotation_members_and_clears_fixed_assignee() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.assigned_to_user_id.is_none())
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        series_mock
            .expect_set_rotation_members()
            .withf(|series_id: &str, ids: &[String]| {
                series_id == "new-series-id"
                    && ids == ["member1".to_string(), "member2".to_string()]
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.rotation_user_ids = Some(vec!["member1".to_string(), "member2".to_string()]);
        let id = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "member1",
            params,
        )
        .await
        .expect("member should be able to set up a rotation");
        assert_eq!(id, "new-series-id");
    }

    #[tokio::test]
    async fn create_series_rejects_template_item_type() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Template;
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_simple_item_type() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Simple;
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_event_type_on_task_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = Some("rain".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_event_type_on_event_series() {
        // event_type is currently unsupported on any series, not just Task — see
        // validate_series_event_type's doc comment for why. This was previously allowed.
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.event_type = Some("rain".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_allows_task_series_without_event_type() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = None;
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_series_rejects_completion_basis_on_event_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.basis = Some("COMPLETION".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_due_date_basis_on_event_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.basis = Some("DUE_DATE".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_allows_due_date_basis_on_any_recurrence_pattern_for_a_task_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.basis.as_deref() == Some("DUE_DATE"))
            .times(1)
            .returning(|_| Ok("s1".to_string()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = None;
        // create_params()'s default recurrence is "every weekday" — unlike COMPLETION,
        // DUE_DATE has no "every N units" restriction (it doesn't affect cursor
        // advancement, only which field materialization writes to).
        params.basis = Some("DUE_DATE".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_series_rejects_completion_basis_on_an_ineligible_pattern() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = None;
        // create_params()'s default recurrence is "every weekday" — a WeeklyDay pattern,
        // not an "every N units" one.
        params.basis = Some("COMPLETION".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_allows_completion_basis_on_an_eligible_task_series() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.basis.as_deref() == Some("COMPLETION"))
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.event_type = None;
        params.recurrence = "every 3 days".to_string();
        params.basis = Some("COMPLETION".to_string());
        let result = create_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            params,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_series_rejects_template_item_id_on_an_event_series() {
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());

        let mut params = create_params("p1");
        params.item_type = ItemKind::Event;
        params.template_item_id = Some("template-1".to_string());
        let result = create_series(&repo, &projects, &teams, &series_repo, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_a_non_template_item_as_the_template() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| {
                project_id == "p1" && item_id == "not-a-template"
            })
            .returning(|_, _| Ok(Item::new_project_item("p1", "Just a task")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.template_item_id = Some("not-a-template".to_string());
        let result = create_series(&repo, &projects, &teams, &series_repo, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_series_rejects_a_template_item_from_another_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_create_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Err(RepoError::NotFound));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.template_item_id = Some("other-projects-template".to_string());
        let result = create_series(&repo, &projects, &teams, &series_repo, "owner1", params).await;
        assert!(matches!(result, Err(ItemError::NotFound)));
    }

    #[tokio::test]
    async fn create_series_accepts_a_valid_template_item_id() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_create_series()
            .withf(|s: &ItemSeries| s.template_item_id.as_deref() == Some("template-1"))
            .times(1)
            .returning(|_| Ok("new-series-id".to_string()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "template-1")
            .returning(|project_id, id| Ok(template_item(id, project_id)));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let mut params = create_params("p1");
        params.item_type = ItemKind::Task;
        params.template_item_id = Some("template-1".to_string());
        let result = create_series(&repo, &projects, &teams, &series_repo, "owner1", params).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn update_series_rejects_event_type_on_task_series() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut params = update_params();
        params.item_type = ItemKind::Task;
        params.event_type = Some("meeting".to_string());
        let result = update_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            "s1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_series_rejects_event_type_on_event_series() {
        // event_type is currently unsupported on any series, not just Task — this
        // combination was previously allowed.
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut params = update_params();
        params.item_type = ItemKind::Event;
        params.event_type = Some("meeting".to_string());
        let result = update_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            "s1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_series_rejects_template_item_type() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut params = update_params();
        params.item_type = ItemKind::Template;
        let result = update_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            "s1",
            params,
        )
        .await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_series_accepts_a_valid_template_item_id() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_update_series()
            .withf(|_, s: &ItemSeries| s.template_item_id.as_deref() == Some("template-1"))
            .times(1)
            .returning(|_, _| Ok(()));
        series_mock
            .expect_set_rotation_members()
            .withf(|_, ids: &[String]| ids.is_empty())
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
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
        let result = update_series(
            &repo,
            &projects,
            &teams,
            &series_repo,
            "owner1",
            "s1",
            params,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_series_returns_series_for_a_member() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_series(&projects, &teams, &series_repo, "owner1", "s1")
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
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = get_series(&projects, &teams, &series_repo, "owner1", "bogus").await;
        assert!(matches!(result, Err(ItemError::NotFound)));
    }

    #[tokio::test]
    async fn delete_series_deletes_after_confirming_membership() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_delete_series()
            .withf(|series_id: &str| series_id == "s1")
            .times(1)
            .returning(|_| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        delete_series(&projects, &teams, &series_repo, "owner1", "s1")
            .await
            .expect("owner should be able to delete the series");
    }

    #[tokio::test]
    async fn delete_series_rejects_non_member() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_delete_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = delete_series(&projects, &teams, &series_repo, "not-the-owner", "s1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_series_propagates_not_found() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        series_mock.expect_delete_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = delete_series(&projects, &teams, &series_repo, "owner1", "bogus").await;
        assert!(matches!(result, Err(ItemError::NotFound)));
    }

    #[tokio::test]
    async fn update_series_overwrites_fields_after_confirming_membership() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock
            .expect_update_series()
            .withf(|series_id: &str, s: &ItemSeries| {
                series_id == "s1" && s.project_id == "p1" && s.name == "Retro"
            })
            .times(1)
            .returning(|_, _| Ok(()));
        series_mock
            .expect_set_rotation_members()
            .withf(|_, ids: &[String]| ids.is_empty())
            .returning(|_, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        update_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
            "owner1",
            "s1",
            update_params(),
        )
        .await
        .expect("owner should be able to update the series");
    }

    #[tokio::test]
    async fn update_series_rejects_non_member() {
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Ok(series("p1")));
        series_mock.expect_update_series().times(0);
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = update_series(
            &no_template_repo(),
            &projects,
            &teams,
            &series_repo,
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
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = list_series_for_project(&projects, &teams, &series_repo, "owner1", "p1")
            .await
            .expect("owner should be able to list series");
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn list_series_for_project_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(MockItemSeriesRepo::new());

        let result =
            list_series_for_project(&projects, &teams, &series_repo, "not-the-owner", "p1").await;
        assert!(result.is_err());
    }

    fn series_ex(
        id: &str,
        project_id: &str,
        name: &str,
        recurrence: &str,
        anchor: DateTime<Utc>,
    ) -> ItemSeries {
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
            priority: None,
        }
    }

    fn anchor() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[tokio::test]
    async fn list_occurrence_states_classifies_materialized_skipped_and_virtual_dates() {
        let s = series_ex("s1", "p1", "Standup", "every 7 days", anchor());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![s.clone()]));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| {
                Ok(vec![
                    ItemOccurrence {
                        series_id: "s1".to_string(),
                        occurrence_date: anchor(),
                        item_id: Some("item-a".to_string()),
                        is_exdate: false,
                    },
                    ItemOccurrence {
                        series_id: "s1".to_string(),
                        occurrence_date: anchor() + chrono::Duration::days(7),
                        item_id: None,
                        is_exdate: true,
                    },
                ])
            });
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let users: Arc<dyn UserRepo> = Arc::new(MockUserRepo::new());

        let range_start = anchor() - chrono::Duration::days(1);
        let range_end = anchor() + chrono::Duration::days(20);
        let mut result = list_occurrence_states_for_project(
            &series_repo,
            &users,
            "p1",
            range_start,
            range_end,
            0,
        )
        .await
        .unwrap();
        result.sort_by_key(|o| o.occurrence_date);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].occurrence_date, anchor());
        assert_eq!(
            result[0].state,
            OccurrenceState::Materialized {
                item_id: "item-a".to_string()
            }
        );
        assert_eq!(
            result[1].occurrence_date,
            anchor() + chrono::Duration::days(7)
        );
        assert_eq!(result[1].state, OccurrenceState::Skipped);
        assert_eq!(
            result[2].occurrence_date,
            anchor() + chrono::Duration::days(14)
        );
        assert_eq!(result[2].state, OccurrenceState::Virtual);
    }

    #[tokio::test]
    async fn list_occurrence_states_marks_the_task_series_current_occurrence() {
        let mut s = series_ex("s1", "p1", "Daily chore", "every 7 days", anchor());
        s.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![s.clone()]));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(Vec::new()));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);
        let users: Arc<dyn UserRepo> = Arc::new(MockUserRepo::new());

        // A window that doesn't naturally contain the series' current (anchor)
        // occurrence at all — exercises the same "current is force-injected even
        // outside the caller's window" behavior list_virtual_occurrences_for_project_
        // unchecked already relies on (see its own doc comment).
        let range_start = anchor() + chrono::Duration::days(3);
        let range_end = anchor() + chrono::Duration::days(4);
        let result = list_occurrence_states_for_project(
            &series_repo,
            &users,
            "p1",
            range_start,
            range_end,
            0,
        )
        .await
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].occurrence_date, anchor());
        assert!(result[0].is_current);
        assert_eq!(result[0].state, OccurrenceState::Virtual);
    }

    #[tokio::test]
    async fn list_occurrence_states_rotates_assignee_by_occurrence_position() {
        let s = series_ex("s1", "p1", "Trash day", "every 7 days", anchor());
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_list_series_for_project()
            .returning(move |_| Ok(vec![s.clone()]));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(vec!["alice".to_string(), "bob".to_string()]));
        series_mock
            .expect_list_occurrences_between()
            .returning(|_, _, _| Ok(vec![]));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut users_mock = MockUserRepo::new();
        users_mock.expect_get().returning(|user_id| {
            let first_name = match user_id {
                "alice" => "Alice",
                "bob" => "Bob",
                other => panic!("unexpected user id {other}"),
            };
            Ok(crate::domain::user::User::new(first_name, "Doe"))
        });
        let users: Arc<dyn UserRepo> = Arc::new(users_mock);

        let range_start = anchor() - chrono::Duration::days(1);
        let range_end = anchor() + chrono::Duration::days(20);
        let mut result = list_occurrence_states_for_project(
            &series_repo,
            &users,
            "p1",
            range_start,
            range_end,
            0,
        )
        .await
        .unwrap();
        result.sort_by_key(|o| o.occurrence_date);

        // Index 0 (anchor), 1 (anchor+7d), 2 (anchor+14d) — alternates alice/bob/alice,
        // matching rotation_assignee's `index % rotation.len()`.
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].assigned_to_user_id.as_deref(), Some("alice"));
        assert_eq!(
            result[0].assigned_to_user_name.as_deref(),
            Some("Alice Doe")
        );
        assert_eq!(result[1].assigned_to_user_id.as_deref(), Some("bob"));
        assert_eq!(result[1].assigned_to_user_name.as_deref(), Some("Bob Doe"));
        assert_eq!(result[2].assigned_to_user_id.as_deref(), Some("alice"));
        assert_eq!(
            result[2].assigned_to_user_name.as_deref(),
            Some("Alice Doe")
        );
    }

    #[tokio::test]
    async fn materializes_a_task_with_the_rotation_members_turn_as_assignee() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        // Third occurrence (index 2) after the anchor, on a two-person rotation —
        // 2 % 2 == 0, so it's the first member's ("alice") turn again.
        let occurrence = task_series.anchor_date + chrono::Duration::days(14);
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(move |_| Ok(task_series.clone()));
        series_mock
            .expect_get_occurrence()
            .returning(|_, _| Ok(None));
        series_mock
            .expect_list_rotation_members()
            .returning(|_| Ok(vec!["alice".to_string(), "bob".to_string()]));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let series_repo: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(crate::domain::team::TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);

        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.assigned_to_user_id().as_deref() == Some("alice"))
            .times(1)
            .returning(|_| Ok("new-item-id".to_string()));
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("p1", "Trash day")));
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);

        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let item = get_or_materialize_occurrence(
            &repo,
            &projects,
            &teams,
            &series_repo,
            &no_op_reminders(),
            "owner1",
            "s1",
            occurrence,
            0,
        )
        .await
        .expect("should materialize with the rotation's computed assignee");

        assert_eq!(item.name, "Trash day");
    }
}
