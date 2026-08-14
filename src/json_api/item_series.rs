use super::{internal, not_found, to_domain_item_type, to_sdk_item_type};
use crate::auth::AuthUser;
use crate::domain::item_series::ItemSeries;
use crate::service::items::ItemError;
use crate::service::item_series as item_series_service;
use crate::service::item_series::{CreateItemSeriesParams, UpdateItemSeriesParams};
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

fn to_summary(series: ItemSeries) -> model::ItemSeriesSummary {
    model::ItemSeriesSummary {
        series_id: series.id,
        project_id: series.project_id,
        name: series.name,
        description: series.description,
        event_type: series.event_type,
        recurrence: series.recurrence,
        anchor_date: SmithyDateTime::from_secs(series.anchor_date.timestamp()),
        item_type: to_sdk_item_type(series.item_type),
    }
}

pub async fn create_item_series(
    input: input::CreateItemSeriesInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateItemSeriesOutput, error::CreateItemSeriesError> {
    let series_id = item_series_service::create_series(
        &projects,
        &teams,
        &item_series,
        &auth.user_id,
        CreateItemSeriesParams {
            project_id: input.project_id,
            name: input.name,
            description: input.description,
            event_type: input.event_type,
            recurrence: input.recurrence,
            anchor_date: anchor_from_input(&input.anchor_date),
            item_type: to_domain_item_type(Some(input.item_type)).unwrap(),
        },
    )
    .await
    .map_err(|e| error::CreateItemSeriesError::from(to_msg(e)))?;
    Ok(output::CreateItemSeriesOutput { series_id })
}

pub async fn get_item_series(
    input: input::GetItemSeriesInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::GetItemSeriesOutput, error::GetItemSeriesError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth.user_id,
        &input.series_id,
    )
    .await
    .map_err(|e| error::GetItemSeriesError::from(to_msg(e)))?;
    Ok(output::GetItemSeriesOutput {
        series_id: series.id,
        project_id: series.project_id,
        name: series.name,
        description: series.description,
        event_type: series.event_type,
        recurrence: series.recurrence,
        anchor_date: SmithyDateTime::from_secs(series.anchor_date.timestamp()),
        item_type: to_sdk_item_type(series.item_type),
    })
}

pub async fn update_item_series(
    input: input::UpdateItemSeriesInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateItemSeriesOutput, error::UpdateItemSeriesError> {
    item_series_service::update_series(
        &projects,
        &teams,
        &item_series,
        &auth.user_id,
        &input.series_id,
        UpdateItemSeriesParams {
            name: input.name,
            description: input.description,
            event_type: input.event_type,
            recurrence: input.recurrence,
            anchor_date: anchor_from_input(&input.anchor_date),
            item_type: to_domain_item_type(Some(input.item_type)).unwrap(),
        },
    )
    .await
    .map_err(|e| error::UpdateItemSeriesError::from(to_msg(e)))?;
    Ok(output::UpdateItemSeriesOutput {})
}

pub async fn list_item_series_for_project(
    input: input::ListItemSeriesForProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListItemSeriesForProjectOutput, error::ListItemSeriesForProjectError> {
    let series = item_series_service::list_series_for_project(
        &projects,
        &teams,
        &item_series,
        &auth.user_id,
        &input.project_id,
    )
    .await
    .map_err(|e| error::ListItemSeriesForProjectError::from(to_msg(e)))?
    .into_iter()
    .map(to_summary)
    .collect();
    Ok(output::ListItemSeriesForProjectOutput { series })
}
