use crate::domain::item::{Item, ItemType};
use crate::service::items::ItemError;
use crate::storage::sqlite::ItemRepo;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct CreateTemplateParams {
    pub user_id: String,
    pub name: String,
    pub source_item_id: Option<String>,
    pub event_type: Option<String>,
}

/// Moved from `json_api::templates::create_template`.
pub async fn create_template(
    repo: &Arc<dyn ItemRepo>,
    params: CreateTemplateParams,
) -> Result<String, ItemError> {
    let mut item = Item::new_user_item(&params.user_id, &params.name);
    item.item_type = ItemType::Template;

    if let Some(source_id) = params.source_item_id {
        let source = repo.get(&params.user_id, &source_id).await?;
        item.name = source.name;
        item.recurrence = source.recurrence;
        item.recurrence_basis = source.recurrence_basis;
        item.has_due_time = source.has_due_time;
        item.event_type = source.event_type;
        item.due_offset_days = source.due_offset_days;
        // deadline intentionally not copied — templates have no dates
    }
    if params.event_type.is_some() {
        item.event_type = params.event_type;
    }

    let template_id = repo.create(&item).await?;
    Ok(template_id)
}
