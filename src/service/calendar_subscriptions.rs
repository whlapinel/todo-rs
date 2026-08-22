use crate::domain::calendar_subscription::CalendarSubscription;
use crate::service::calendar_sync;
use crate::service::error::ItemError;
use crate::service::projects::{require_project_admin, require_project_member};
use crate::storage::sqlite::{CalendarSubscriptionRepo, ItemRepo, ProjectRepo, TeamRepo, UserRepo};
use std::sync::Arc;

/// Creates a subscription (admin-only, per docs/google-calendar-import-plan.md's
/// explicit "only project admin" requirement) and immediately triggers a sync so the
/// admin sees events populate right away rather than waiting up to 15 minutes for the
/// background sweep. A sync failure (bad URL, unreachable host, etc.) doesn't fail the
/// creation itself — `calendar_sync::sync_subscription` already records the failure on
/// the subscription row (`last_sync_error`) for the web UI to surface, matching how a
/// later background-sweep failure is handled identically (see `sync_all_subscriptions`
/// below).
pub async fn create_calendar_subscription(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    calendar_repo: &Arc<dyn CalendarSubscriptionRepo>,
    item_repo: &Arc<dyn ItemRepo>,
    user_repo: &Arc<dyn UserRepo>,
    project_id: &str,
    requester_user_id: &str,
    ical_url: &str,
) -> Result<String, ItemError> {
    require_project_admin(projects, teams, project_id, requester_user_id).await?;
    let subscription = calendar_repo
        .create(project_id, ical_url, requester_user_id)
        .await?;
    if let Err(e) = calendar_sync::sync_subscription(
        &subscription,
        item_repo.as_ref(),
        calendar_repo.as_ref(),
        user_repo.as_ref(),
    )
    .await
    {
        tracing::warn!(
            subscription_id = %subscription.id,
            error = %e,
            "initial calendar sync failed"
        );
    }
    Ok(subscription.id)
}

/// Any project member can see what's subscribed — matches how project membership
/// already gates read access elsewhere (see `list_project_members`).
pub async fn list_calendar_subscriptions(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    calendar_repo: &Arc<dyn CalendarSubscriptionRepo>,
    project_id: &str,
    requester_user_id: &str,
) -> Result<Vec<CalendarSubscription>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(calendar_repo.list_by_project(project_id).await?)
}

/// Deletes a subscription and cascades to every `Item` it imported (the cascade
/// decision from docs/google-calendar-import-plan.md's Context section — unsubscribing
/// is the only way to bulk-remove imported events short of them disappearing from the
/// upstream calendar). Done here in the service function, not the repo, so it goes
/// through normal item-delete bookkeeping if any exists later.
pub async fn delete_calendar_subscription(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    calendar_repo: &Arc<dyn CalendarSubscriptionRepo>,
    item_repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    requester_user_id: &str,
    subscription_id: &str,
) -> Result<(), ItemError> {
    require_project_admin(projects, teams, project_id, requester_user_id).await?;
    let subscription = calendar_repo.get(subscription_id).await?;
    if subscription.project_id != project_id {
        return Err(ItemError::NotFound);
    }
    let imported_items = item_repo
        .list_by_calendar_subscription(subscription_id)
        .await?;
    for item in imported_items {
        item_repo.delete(&item.id).await?;
    }
    calendar_repo.delete(subscription_id).await?;
    Ok(())
}

/// The background sweep's entry point (`src/main.rs`'s `tokio::spawn`ed loop) — syncs
/// every subscription across every project, logging (not propagating) any one
/// subscription's failure so a single bad feed can't stop the rest of the sweep.
pub async fn sync_all_subscriptions(
    calendar_repo: &Arc<dyn CalendarSubscriptionRepo>,
    item_repo: &Arc<dyn ItemRepo>,
    user_repo: &Arc<dyn UserRepo>,
) {
    let subscriptions = match calendar_repo.list_all().await {
        Ok(subs) => subs,
        Err(e) => {
            tracing::error!(error = ?e, "failed to list calendar subscriptions for sync sweep");
            return;
        }
    };
    for subscription in subscriptions {
        if let Err(e) = calendar_sync::sync_subscription(
            &subscription,
            item_repo.as_ref(),
            calendar_repo.as_ref(),
            user_repo.as_ref(),
        )
        .await
        {
            tracing::warn!(
                subscription_id = %subscription.id,
                error = %e,
                "calendar sync failed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::Item;
    use crate::domain::project::Project;
    use crate::domain::team::TeamRole;
    use crate::storage::sqlite::{
        MockCalendarSubscriptionRepo, MockItemRepo, MockProjectRepo, MockTeamRepo, MockUserRepo,
    };
    use chrono::Utc;

    fn personal_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Personal".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: None,
        }
    }

    fn subscription() -> CalendarSubscription {
        CalendarSubscription {
            id: "sub1".to_string(),
            project_id: "p1".to_string(),
            ical_url: "https://calendar.google.com/secret.ics".to_string(),
            created_by_user_id: "owner1".to_string(),
            created_at: Utc::now(),
            last_synced_at: None,
            last_sync_error: None,
        }
    }

    #[tokio::test]
    async fn create_calendar_subscription_rejects_non_admin() {
        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let calendar_repo: Arc<dyn CalendarSubscriptionRepo> =
            Arc::new(MockCalendarSubscriptionRepo::new());
        let item_repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let user_repo: Arc<dyn UserRepo> = Arc::new(MockUserRepo::new());

        let err = create_calendar_subscription(
            &projects,
            &teams,
            &calendar_repo,
            &item_repo,
            &user_repo,
            "p1",
            "not_the_owner",
            "https://example.com/feed.ics",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn create_calendar_subscription_allows_admin_and_triggers_sync() {
        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(personal_project()));
        projects
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut calendar_repo = MockCalendarSubscriptionRepo::new();
        calendar_repo
            .expect_create()
            .withf(|project_id, ical_url, created_by| {
                project_id == "p1" && ical_url == "not a valid url" && created_by == "owner1"
            })
            .returning(|_, _, _| Ok(subscription()));
        // "not a valid url" fails at URL-parse time inside reqwest, before any network
        // I/O happens — sync_subscription records that failure via record_sync_result
        // rather than propagating it, so create_calendar_subscription still succeeds.
        // This keeps the test hermetic (no real network access) while still exercising
        // the "creation succeeds even though the immediate sync failed" behavior.
        calendar_repo
            .expect_record_sync_result()
            .returning(|_, _, _| Ok(()));
        let calendar_repo: Arc<dyn CalendarSubscriptionRepo> = Arc::new(calendar_repo);
        let item_repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        // "not a valid url" fails inside `sync_subscription`'s `fetch_ical` call, before
        // it ever reaches `resolve_all_day_tz` — so this mock needs no expectations.
        let user_repo: Arc<dyn UserRepo> = Arc::new(MockUserRepo::new());

        let id = create_calendar_subscription(
            &projects,
            &teams,
            &calendar_repo,
            &item_repo,
            &user_repo,
            "p1",
            "owner1",
            "not a valid url",
        )
        .await
        .unwrap();
        assert_eq!(id, "sub1");
    }

    #[tokio::test]
    async fn list_calendar_subscriptions_rejects_non_member() {
        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let calendar_repo: Arc<dyn CalendarSubscriptionRepo> =
            Arc::new(MockCalendarSubscriptionRepo::new());

        let err =
            list_calendar_subscriptions(&projects, &teams, &calendar_repo, "p1", "someone_else")
                .await
                .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn list_calendar_subscriptions_allows_member() {
        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut calendar_repo = MockCalendarSubscriptionRepo::new();
        calendar_repo
            .expect_list_by_project()
            .withf(|project_id| project_id == "p1")
            .returning(|_| Ok(vec![subscription()]));
        let calendar_repo: Arc<dyn CalendarSubscriptionRepo> = Arc::new(calendar_repo);

        let subs = list_calendar_subscriptions(&projects, &teams, &calendar_repo, "p1", "owner1")
            .await
            .unwrap();
        assert_eq!(subs.len(), 1);
    }

    #[tokio::test]
    async fn delete_calendar_subscription_rejects_non_admin() {
        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let calendar_repo: Arc<dyn CalendarSubscriptionRepo> =
            Arc::new(MockCalendarSubscriptionRepo::new());
        let item_repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());

        let err = delete_calendar_subscription(
            &projects,
            &teams,
            &calendar_repo,
            &item_repo,
            "p1",
            "not_the_owner",
            "sub1",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn delete_calendar_subscription_rejects_subscription_from_another_project() {
        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(personal_project()));
        projects
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut calendar_repo = MockCalendarSubscriptionRepo::new();
        calendar_repo.expect_get().returning(|_| {
            Ok(CalendarSubscription {
                project_id: "some_other_project".to_string(),
                ..subscription()
            })
        });
        let calendar_repo: Arc<dyn CalendarSubscriptionRepo> = Arc::new(calendar_repo);
        let item_repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());

        let err = delete_calendar_subscription(
            &projects,
            &teams,
            &calendar_repo,
            &item_repo,
            "p1",
            "owner1",
            "sub1",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ItemError::NotFound));
    }

    #[tokio::test]
    async fn delete_calendar_subscription_cascades_imported_items() {
        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(personal_project()));
        projects
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let mut calendar_repo = MockCalendarSubscriptionRepo::new();
        calendar_repo.expect_get().returning(|_| Ok(subscription()));
        calendar_repo
            .expect_delete()
            .withf(|id| id == "sub1")
            .returning(|_| Ok(()));
        let calendar_repo: Arc<dyn CalendarSubscriptionRepo> = Arc::new(calendar_repo);

        let mut item_repo = MockItemRepo::new();
        item_repo
            .expect_list_by_calendar_subscription()
            .returning(|_| {
                Ok(vec![
                    Item {
                        id: "i1".to_string(),
                        ..Item::new_project_item("p1", "Imported event 1")
                    },
                    Item {
                        id: "i2".to_string(),
                        ..Item::new_project_item("p1", "Imported event 2")
                    },
                ])
            });
        let mut deleted_ids = std::collections::HashSet::new();
        item_repo
            .expect_delete()
            .withf(move |id| id == "i1" || id == "i2")
            .returning(move |id| {
                deleted_ids.insert(id.to_string());
                Ok(())
            });
        let item_repo: Arc<dyn ItemRepo> = Arc::new(item_repo);

        delete_calendar_subscription(
            &projects,
            &teams,
            &calendar_repo,
            &item_repo,
            "p1",
            "owner1",
            "sub1",
        )
        .await
        .unwrap();
    }
}
