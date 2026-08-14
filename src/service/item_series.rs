use crate::domain::item::{Item, ItemKind};
use crate::domain::item_series::ItemSeries;
use crate::domain::recurrence;
use crate::service::error::ItemError;
use crate::service::project_items::{self, CreateProjectItemParams};
use crate::service::projects::require_project_member;
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
pub async fn get_or_materialize_occurrence(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
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
            ..Default::default()
        },
    )
    .await?;

    event_series
        .record_materialized_occurrence(series_id, occurrence_date, &item_id)
        .await?;

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
pub async fn skip_occurrence(
    event_series: &Arc<dyn ItemSeriesRepo>,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
) -> Result<(), ItemError> {
    let series = event_series.get_series(series_id).await?;
    event_series.mark_exdate(series_id, occurrence_date).await?;
    // Stage 9: skipping settles the occurrence exactly like completing one does — see
    // record_task_completion's doc comment below for why this is symmetric. Meaningless
    // for an Event-typed series (no completion/cursor concept), so left untouched there.
    if series.item_type == ItemKind::Task {
        event_series.advance_cursor(series_id, occurrence_date).await?;
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

/// Stage 9: called after `service::project_items::update_project_item` successfully
/// transitions an item to `complete: true`. A cheap no-op for the overwhelmingly common
/// case (the item never came from a series, or came from an Event-typed one, which has
/// no completion/cursor concept) — same shape as `unlink_deleted_item_occurrence`.
///
/// Deliberately *not* gated on "is this the series' current occurrence" — completing any
/// materialized Task-series occurrence, in any order, advances the cursor to (at least)
/// that occurrence's date, via `advance_cursor`'s own forward-only max. Completing a
/// future occurrence early therefore does abandon whatever backlog sat between the old
/// cursor and the new one; that's the deliberate, simpler rule chosen over tracking
/// "current" separately from "whatever just got completed."
pub async fn record_task_completion(
    event_series: &Arc<dyn ItemSeriesRepo>,
    item_id: &str,
) -> Result<(), ItemError> {
    if let Some(occurrence) = event_series.find_occurrence_by_item_id(item_id).await? {
        let series = event_series.get_series(&occurrence.series_id).await?;
        if series.item_type == ItemKind::Task {
            event_series
                .advance_cursor(&occurrence.series_id, occurrence.occurrence_date)
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

/// Stage 7c: `event_type` only means anything on an `Event`-typed series — a Task series has
/// no `event_type` slot on the `Item` it materializes (`ItemType::Task` carries no such
/// field, see `domain::item::build_item_type`/`ItemType`), so `get_or_materialize_occurrence`
/// would silently drop it forever rather than erroring. `ItemSeries` is a flat struct rather
/// than `Item`'s data-carrying `ItemType` enum, so this has to be an explicit runtime check
/// here instead of the structural impossibility that already rules this out on `Item` itself
/// — same rejection pattern as `validate_series_item_type` above.
fn validate_series_event_type(item_type: ItemKind, event_type: &Option<String>) -> Result<(), ItemError> {
    if event_type.is_some() && item_type != ItemKind::Event {
        return Err(ItemError::Invalid(
            "event_type is only valid on an EVENT series".to_string(),
        ));
    }
    Ok(())
}

pub async fn create_series(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    event_series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    params: CreateItemSeriesParams,
) -> Result<String, ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    validate_series_item_type(params.item_type)?;
    validate_series_event_type(params.item_type, &params.event_type)?;
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
}

pub async fn update_series(
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
    validate_series_event_type(params.item_type, &params.event_type)?;
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
        let mut candidates = recurrence::occurrences_between(
            &rule,
            series.anchor_date,
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
        let current_date = current_occurrence_date(series, &rule, tz_offset_minutes);
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
        )
        .await
        .expect("should materialize a task occurrence");

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

        skip_occurrence(&event_series, "s1", occurrence_date())
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

        skip_occurrence(&event_series, "s1", occurrence_date())
            .await
            .expect("should mark the occurrence as skipped");
    }

    #[tokio::test]
    async fn skip_occurrence_advances_cursor_for_a_task_series() {
        let mut task_series = series("p1");
        task_series.item_type = ItemKind::Task;
        let mut series_mock = MockItemSeriesRepo::new();
        series_mock.expect_get_series().returning(move |_| Ok(task_series.clone()));
        series_mock.expect_mark_exdate().returning(|_, _| Ok(()));
        series_mock
            .expect_advance_cursor()
            .withf(|series_id: &str, date: &DateTime<Utc>| series_id == "s1" && *date == occurrence_date())
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn ItemSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&event_series, "s1", occurrence_date())
            .await
            .expect("should mark the occurrence as skipped and advance the cursor");
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

        let result = skip_occurrence(&event_series, "bogus", occurrence_date()).await;
        assert!(matches!(result, Err(ItemError::NotFound)));
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
        }
    }

    fn update_params() -> UpdateItemSeriesParams {
        UpdateItemSeriesParams {
            name: "Retro".to_string(),
            description: Some("Weekly retro".to_string()),
            event_type: Some("meeting".to_string()),
            recurrence: "every friday".to_string(),
            anchor_date: occurrence_date(),
            item_type: ItemKind::Event,
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

        let id = create_series(&projects, &teams, &event_series, "owner1", create_params("p1"))
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
        let id = create_series(&projects, &teams, &event_series, "owner1", params)
            .await
            .expect("owner should be able to create a task-typed series");
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
        let result = create_series(&projects, &teams, &event_series, "owner1", params).await;
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
        let result = create_series(&projects, &teams, &event_series, "owner1", params).await;
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
        let result = create_series(&projects, &teams, &event_series, "owner1", params).await;
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
        let result = create_series(&projects, &teams, &event_series, "owner1", params).await;
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
        // update_params() defaults event_type to Some("meeting") — left as-is here
        // deliberately, since that's exactly the combination being rejected.
        let result = update_series(&projects, &teams, &event_series, "owner1", "s1", params).await;
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
        let result = update_series(&projects, &teams, &event_series, "owner1", "s1", params).await;
        assert!(matches!(result, Err(ItemError::Invalid(_))));
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

        update_series(&projects, &teams, &event_series, "owner1", "s1", update_params())
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
