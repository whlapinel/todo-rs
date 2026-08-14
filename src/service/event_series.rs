use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::project_items::{self, CreateProjectItemParams};
use crate::storage::sqlite::{EventSeriesRepo, ItemRepo, ProjectRepo, TeamRepo};
use chrono::{DateTime, Utc};
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
    event_series: &Arc<dyn EventSeriesRepo>,
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
            item_type: Some(ItemKind::Event),
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

/// Marks `occurrence_date` as skipped (the EXDATE-equivalent) for `series_id`.
/// Whether an already-materialized occurrence's `items` row should be deleted when
/// skipped is an open call deferred to stage 6 — not decided here, so an
/// already-materialized occurrence's `item_id` is left untouched by this call.
pub async fn skip_occurrence(
    event_series: &Arc<dyn EventSeriesRepo>,
    series_id: &str,
    occurrence_date: DateTime<Utc>,
) -> Result<(), ItemError> {
    event_series.get_series(series_id).await?;
    event_series.mark_exdate(series_id, occurrence_date).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::event_series::{EventOccurrence, EventSeries};
    use crate::domain::project::Project;
    use crate::storage::sqlite::{
        MockEventSeriesRepo, MockItemRepo, MockProjectRepo, MockTeamRepo, RepoError,
    };

    fn series(project_id: &str) -> EventSeries {
        EventSeries {
            id: "s1".to_string(),
            project_id: project_id.to_string(),
            name: "Standup".to_string(),
            description: None,
            event_type: None,
            recurrence: "every weekday".to_string(),
            anchor_date: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
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
        let mut series_mock = MockEventSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, date| {
            Ok(Some(EventOccurrence {
                series_id: "s1".to_string(),
                occurrence_date: date,
                item_id: Some("existing-item".to_string()),
                is_exdate: false,
            }))
        });
        series_mock.expect_record_materialized_occurrence().times(0);
        let event_series: Arc<dyn EventSeriesRepo> = Arc::new(series_mock);

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
        let mut series_mock = MockEventSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        series_mock
            .expect_record_materialized_occurrence()
            .withf(|series_id: &str, _date, item_id: &str| series_id == "s1" && item_id == "new-item-id")
            .times(1)
            .returning(|_, _, _| Ok(()));
        let event_series: Arc<dyn EventSeriesRepo> = Arc::new(series_mock);

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
    async fn propagates_not_found_for_unknown_series() {
        let mut series_mock = MockEventSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        series_mock.expect_get_occurrence().times(0);
        let event_series: Arc<dyn EventSeriesRepo> = Arc::new(series_mock);

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
        let mut series_mock = MockEventSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        let event_series: Arc<dyn EventSeriesRepo> = Arc::new(series_mock);

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
        let mut series_mock = MockEventSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock.expect_get_occurrence().returning(|_, _| Ok(None));
        series_mock
            .expect_record_materialized_occurrence()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let event_series: Arc<dyn EventSeriesRepo> = Arc::new(series_mock);

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
        let mut series_mock = MockEventSeriesRepo::new();
        series_mock.expect_get_series().returning(|_| Ok(series("p1")));
        series_mock
            .expect_mark_exdate()
            .withf(|series_id: &str, _date| series_id == "s1")
            .times(1)
            .returning(|_, _| Ok(()));
        let event_series: Arc<dyn EventSeriesRepo> = Arc::new(series_mock);

        skip_occurrence(&event_series, "s1", occurrence_date())
            .await
            .expect("should mark the occurrence as skipped");
    }

    #[tokio::test]
    async fn skip_occurrence_propagates_not_found_without_marking() {
        let mut series_mock = MockEventSeriesRepo::new();
        series_mock
            .expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        series_mock.expect_mark_exdate().times(0);
        let event_series: Arc<dyn EventSeriesRepo> = Arc::new(series_mock);

        let result = skip_occurrence(&event_series, "bogus", occurrence_date()).await;
        assert!(matches!(result, Err(ItemError::NotFound)));
    }
}
