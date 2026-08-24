use super::{internal, not_found};
use crate::auth::AuthUser;
use crate::service::import;
use crate::service::items::ItemError;
use crate::storage::sqlite::{ItemRepo, ProjectRepo, ReminderRepo, TeamRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server};

fn to_import_project_items_error(e: ItemError) -> error::ImportProjectItemsError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

pub async fn import_project_items(
    input: input::ImportProjectItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(reminders): server::Extension<Arc<dyn ReminderRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ImportProjectItemsOutput, error::ImportProjectItemsError> {
    let results = import::import_project_items(
        &repo,
        &projects,
        &teams,
        &reminders,
        &auth.user_id,
        &input.project_id,
        &input.csv,
        input.format.as_deref(),
        input.timezone_offset_minutes,
    )
    .await
    .map_err(to_import_project_items_error)?;
    let results = results
        .into_iter()
        .map(|r| todo_server_sdk::model::ImportItemResult {
            row_number: r.row_number,
            success: r.success,
            item_id: r.item_id,
            error: r.error,
        })
        .collect();
    Ok(output::ImportProjectItemsOutput { results })
}

pub async fn get_item_import_template(
    _input: input::GetItemImportTemplateInput,
) -> Result<output::GetItemImportTemplateOutput, error::GetItemImportTemplateError> {
    Ok(output::GetItemImportTemplateOutput {
        csv: import::item_import_template(),
    })
}
