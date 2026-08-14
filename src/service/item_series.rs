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
    event_series.get_series(series_id).await?;
    event_series.mark_exdate(series_id, occurrence_date).await?;
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
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualOccurrence {
    pub series_id: String,
    pub series_name: String,
    pub event_type: Option<String>,
    pub occurrence_date: DateTime<Utc>,
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
pub async fn list_virtual_occurrences_for_project_unchecked(
    event_series: &Arc<dyn ItemSeriesRepo>,
    project_id: &str,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<Vec<VirtualOccurrence>, ItemError> {
    let all_series = event_series.list_series_for_project(project_id).await?;
    let mut result = Vec::new();
    for series in &all_series {
        let Ok(rule) = recurrence::parse(&series.recurrence) else {
            continue;
        };
        let candidates = recurrence::occurrences_between(
            &rule,
            series.anchor_date,
            range_start,
            range_end,
            tz_offset_minutes,
        );
        if candidates.is_empty() {
            continue;
        }
        let existing = event_series
            .list_occurrences_between(&series.id, range_start, range_end)
            .await?;
        let taken: HashSet<i64> = existing.iter().map(|o| o.occurrence_date.timestamp()).collect();
        for date in candidates {
            if !taken.contains(&date.timestamp()) {
                result.push(VirtualOccurrence {
                    series_id: series.id.clone(),
                    series_name: series.name.clone(),
                    event_type: series.event_type.clone(),
                    occurrence_date: date,
                });
            }
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
            recurrence: "every weekday".to_string(),
            anchor_date: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            item_type: ItemKind::Event,
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
