use crate::auth::AuthUser;
use crate::handlers::web_ui::item_list_component::ItemListComponent;
use crate::storage::sqlite::{ItemRepo, RepoError};
use askama::Template;
use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::Html;
use std::sync::Arc;

#[derive(Template)]
#[template(path = "item_children_page.html")]
struct ItemChildrenPageTemplate {
    parent_id: String,
    parent_name: String,
    rows: Vec<String>,
}

pub async fn item_children_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, StatusCode> {
    let parent = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => StatusCode::NOT_FOUND,
            RepoError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    let children = repo
        .list_children(&item_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows = children
        .iter()
        .map(|item| ItemListComponent::from_item(item).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let template = ItemChildrenPageTemplate {
        parent_id: item_id,
        parent_name: parent.name,
        rows,
    };

    template
        .render()
        .map(Html)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
