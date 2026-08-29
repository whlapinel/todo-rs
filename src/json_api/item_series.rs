use super::{internal, not_found, to_domain_item_type, to_sdk_item_type};
use crate::auth::AuthUser;
use crate::domain::item_series::ItemSeries;
use crate::service::item_series as item_series_service;
use crate::service::item_series::{CreateItemSeriesParams, UpdateItemSeriesParams};
use crate::service::items::ItemError;
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
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

fn to_summary(series: ItemSeries, rotation_user_ids: Vec<String>) -> model::ItemSeriesSummary {
    model::ItemSeriesSummary {
        series_id: series.id,
        project_id: series.project_id,
        name: series.name,
        description: series.description,
        event_type: series.event_type,
        recurrence: series.recurrence,
        anchor_date: SmithyDateTime::from_secs(series.anchor_date.timestamp()),
        item_type: to_sdk_item_type(series.item_type),
        basis: series.basis,
        template_item_id: series.template_item_id,
        assigned_to_user_id: series.assigned_to_user_id,
        rotation_user_ids: (!rotation_user_ids.is_empty()).then_some(rotation_user_ids),
        points: series.points,
        priority: series.priority,
    }
}

pub async fn create_item_series(
    input: input::CreateItemSeriesInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateItemSeriesOutput, error::CreateItemSeriesError> {
    let series_id = item_series_service::create_series(
        &repo,
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
            basis: input.basis,
            template_item_id: input.template_item_id,
            assigned_to_user_id: input.assigned_to_user_id,
            rotation_user_ids: input.rotation_user_ids,
            points: input.points,
            priority: input.priority,
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
    let rotation_user_ids = item_series
        .list_rotation_members(&series.id)
        .await
        .map_err(|e| error::GetItemSeriesError::from(to_msg(e.into())))?;
    Ok(output::GetItemSeriesOutput {
        series_id: series.id,
        project_id: series.project_id,
        name: series.name,
        description: series.description,
        event_type: series.event_type,
        recurrence: series.recurrence,
        anchor_date: SmithyDateTime::from_secs(series.anchor_date.timestamp()),
        item_type: to_sdk_item_type(series.item_type),
        basis: series.basis,
        template_item_id: series.template_item_id,
        assigned_to_user_id: series.assigned_to_user_id,
        rotation_user_ids: (!rotation_user_ids.is_empty()).then_some(rotation_user_ids),
        points: series.points,
        priority: series.priority,
    })
}

pub async fn update_item_series(
    input: input::UpdateItemSeriesInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateItemSeriesOutput, error::UpdateItemSeriesError> {
    item_series_service::update_series(
        &repo,
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
            basis: input.basis,
            template_item_id: input.template_item_id,
            assigned_to_user_id: input.assigned_to_user_id,
            rotation_user_ids: input.rotation_user_ids,
            points: input.points,
            priority: input.priority,
        },
    )
    .await
    .map_err(|e| error::UpdateItemSeriesError::from(to_msg(e)))?;
    Ok(output::UpdateItemSeriesOutput {})
}

pub async fn delete_item_series(
    input: input::DeleteItemSeriesInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DeleteItemSeriesOutput, error::DeleteItemSeriesError> {
    item_series_service::delete_series(
        &projects,
        &teams,
        &item_series,
        &auth.user_id,
        &input.series_id,
    )
    .await
    .map_err(|e| error::DeleteItemSeriesError::from(to_msg(e)))?;
    Ok(output::DeleteItemSeriesOutput {})
}

pub async fn list_item_series_for_project(
    input: input::ListItemSeriesForProjectInput,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListItemSeriesForProjectOutput, error::ListItemSeriesForProjectError> {
    let series_list = item_series_service::list_series_for_project(
        &projects,
        &teams,
        &item_series,
        &auth.user_id,
        &input.project_id,
    )
    .await
    .map_err(|e| error::ListItemSeriesForProjectError::from(to_msg(e)))?;
    let mut series = Vec::with_capacity(series_list.len());
    for s in series_list {
        let rotation_user_ids = item_series
            .list_rotation_members(&s.id)
            .await
            .map_err(|e| error::ListItemSeriesForProjectError::from(to_msg(e.into())))?;
        series.push(to_summary(s, rotation_user_ids));
    }
    Ok(output::ListItemSeriesForProjectOutput { series })
}
