use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::domain::item_series::ItemSeries;
use crate::service::event_series as event_series_service;
use crate::service::event_series::{CreateEventSeriesParams, UpdateEventSeriesParams};
use crate::service::items::ItemError;
use crate::storage::sqlite::{ItemSeriesRepo, ProjectRepo, TeamRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, model, output, server, types::DateTime as SmithyDateTime};

fn to_msg(e: ItemError) -> error::PeoplesRepublicOfListsError {
    match e {
        ItemError::NotFound => not_found(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg),
    }
}

fn anchor_from_input(dt: &SmithyDateTime) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(dt.secs(), 0).unwrap_or_default()
}

fn to_summary(series: ItemSeries) -> model::EventSeriesSummary {
    model::EventSeriesSummary {
        series_id: series.id,
        project_id: series.project_id,
        name: series.name,
        description: series.description,
        event_type: series.event_type,
        recurrence: series.recurrence,
        anchor_date: SmithyDateTime::from_secs(series.anchor_date.timestamp()),
    }
}

pub async fn create_event_series(
    input: input::CreateEventSeriesInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(event_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateEventSeriesOutput, error::CreateEventSeriesError> {
    let series_id = event_series_service::create_series(
        &projects,
        &teams,
        &event_series,
        &auth.user_id,
        CreateEventSeriesParams {
            project_id: input.project_id,
            name: input.name,
            description: input.description,
            event_type: input.event_type,
            recurrence: input.recurrence,
            anchor_date: anchor_from_input(&input.anchor_date),
        },
    )
    .await
    .map_err(|e| error::CreateEventSeriesError::from(to_msg(e)))?;
    Ok(output::CreateEventSeriesOutput { series_id })
}

pub async fn get_event_series(
    input: input::GetEventSeriesInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(event_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::GetEventSeriesOutput, error::GetEventSeriesError> {
    let series = event_series_service::get_series(
        &projects,
        &teams,
        &event_series,
        &auth.user_id,
        &input.series_id,
    )
    .await
    .map_err(|e| error::GetEventSeriesError::from(to_msg(e)))?;
    Ok(output::GetEventSeriesOutput {
        series_id: series.id,
        project_id: series.project_id,
        name: series.name,
        description: series.description,
        event_type: series.event_type,
        recurrence: series.recurrence,
        anchor_date: SmithyDateTime::from_secs(series.anchor_date.timestamp()),
    })
}

pub async fn update_event_series(
    input: input::UpdateEventSeriesInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(event_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateEventSeriesOutput, error::UpdateEventSeriesError> {
    event_series_service::update_series(
        &projects,
        &teams,
        &event_series,
        &auth.user_id,
        &input.series_id,
        UpdateEventSeriesParams {
            name: input.name,
            description: input.description,
            event_type: input.event_type,
            recurrence: input.recurrence,
            anchor_date: anchor_from_input(&input.anchor_date),
        },
    )
    .await
    .map_err(|e| error::UpdateEventSeriesError::from(to_msg(e)))?;
    Ok(output::UpdateEventSeriesOutput {})
}

pub async fn list_event_series_for_project(
    input: input::ListEventSeriesForProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(event_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListEventSeriesForProjectOutput, error::ListEventSeriesForProjectError> {
    let series = event_series_service::list_series_for_project(
        &projects,
        &teams,
        &event_series,
        &auth.user_id,
        &input.project_id,
    )
    .await
    .map_err(|e| error::ListEventSeriesForProjectError::from(to_msg(e)))?
    .into_iter()
    .map(to_summary)
    .collect();
    Ok(output::ListEventSeriesForProjectOutput { series })
}
