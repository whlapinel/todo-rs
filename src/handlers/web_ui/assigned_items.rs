use crate::auth::AuthUser;
use crate::domain::item::Item;
use crate::handlers::web_ui::to_local;
use crate::service::error::ItemError;
use crate::storage::sqlite::ItemRepo;
use askama::Template;
use axum::extract::Extension;
use axum::response::Html;
use std::sync::Arc;

use crate::handlers::web_ui::TzOffset;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

#[derive(Template)]
#[template(path = "assigned_items/row.html")]
struct AssignedItemRow {
    id: String,
    team_id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    toggle_complete_json: String,
}

impl AssignedItemRow {
    fn from_item(item: &Item, tz: i32) -> Option<Self> {
        Some(Self {
            id: item.id.clone(),
            team_id: item.team_id.clone()?,
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            toggle_complete_json: (!item.complete).to_string(),
        })
    }
}

#[derive(Template)]
#[template(path = "assigned_items/page.html")]
struct AssignedItemsPageTemplate {
    rows: Vec<String>,
}

pub async fn assigned_items_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let items = repo
        .list_assigned(&auth_user.user_id)
        .await
        .map_err(ItemError::from)?;
    let rows = items
        .iter()
        // Assignment is a team-item-only concept (see CLAUDE.md's Recurrence/domain notes) —
        // every row here has a team_id; the filter_map is just a defensive skip, not an
        // expected case.
        .filter_map(|i| AssignedItemRow::from_item(i, tz))
        .map(|row| row.render())
        .collect::<Result<Vec<_>, _>>()?;
    render(AssignedItemsPageTemplate { rows })
}
