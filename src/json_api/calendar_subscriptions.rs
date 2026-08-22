use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::domain::calendar_subscription::CalendarSubscription;
use crate::service::calendar_subscriptions as calendar_subscriptions_service;
use crate::service::error::ItemError;
use crate::storage::sqlite::{CalendarSubscriptionRepo, ItemRepo, ProjectRepo, TeamRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, model, output, server, types::DateTime as SmithyDateTime};

fn to_msg(e: ItemError) -> error::PeoplesRepublicOfListsError {
    match e {
        ItemError::NotFound => not_found(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg),
    }
}

fn to_summary(sub: CalendarSubscription) -> model::CalendarSubscriptionSummary {
    model::CalendarSubscriptionSummary {
        id: sub.id,
        project_id: sub.project_id,
        ical_url: sub.ical_url,
        created_by_user_id: sub.created_by_user_id,
        created_at: SmithyDateTime::from_secs(sub.created_at.timestamp()),
        last_synced_at: sub
            .last_synced_at
            .map(|t| SmithyDateTime::from_secs(t.timestamp())),
        last_sync_error: sub.last_sync_error,
    }
}

pub async fn create_calendar_subscription(
    input: input::CreateCalendarSubscriptionInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(calendar_repo): server::Extension<Arc<dyn CalendarSubscriptionRepo>>,
    server::Extension(item_repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateCalendarSubscriptionOutput, error::CreateCalendarSubscriptionError> {
    let id = calendar_subscriptions_service::create_calendar_subscription(
        &projects,
        &teams,
        &calendar_repo,
        &item_repo,
        &input.project_id,
        &auth.user_id,
        &input.ical_url,
    )
    .await
    .map_err(|e| error::CreateCalendarSubscriptionError::from(to_msg(e)))?;
    Ok(output::CreateCalendarSubscriptionOutput { id })
}

pub async fn list_calendar_subscriptions(
    input: input::ListCalendarSubscriptionsInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(calendar_repo): server::Extension<Arc<dyn CalendarSubscriptionRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListCalendarSubscriptionsOutput, error::ListCalendarSubscriptionsError> {
    let subs = calendar_subscriptions_service::list_calendar_subscriptions(
        &projects,
        &teams,
        &calendar_repo,
        &input.project_id,
        &auth.user_id,
    )
    .await
    .map_err(|e| error::ListCalendarSubscriptionsError::from(to_msg(e)))?;
    Ok(output::ListCalendarSubscriptionsOutput {
        subscriptions: subs.into_iter().map(to_summary).collect(),
    })
}

pub async fn delete_calendar_subscription(
    input: input::DeleteCalendarSubscriptionInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(calendar_repo): server::Extension<Arc<dyn CalendarSubscriptionRepo>>,
    server::Extension(item_repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DeleteCalendarSubscriptionOutput, error::DeleteCalendarSubscriptionError> {
    calendar_subscriptions_service::delete_calendar_subscription(
        &projects,
        &teams,
        &calendar_repo,
        &item_repo,
        &input.project_id,
        &auth.user_id,
        &input.id,
    )
    .await
    .map_err(|e| error::DeleteCalendarSubscriptionError::from(to_msg(e)))?;
    Ok(output::DeleteCalendarSubscriptionOutput {})
}
