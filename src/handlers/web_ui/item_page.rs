use crate::auth::AuthUser;
use crate::storage::sqlite::{ItemRepo, RepoError};
use askama::Template;
use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::Html;
use std::sync::Arc;

#[derive(Template)]
#[template(path = "item.html")]
struct ItemPageTemplate {
    id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    has_children: bool,
}

pub async fn item_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, StatusCode> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => StatusCode::NOT_FOUND,
            RepoError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    let template = ItemPageTemplate {
        id: item.id,
        name: item.name,
        complete: item.complete,
        due_date: item.due_date.map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()),
        has_children: item.has_children,
    };

    template
        .render()
        .map(Html)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
