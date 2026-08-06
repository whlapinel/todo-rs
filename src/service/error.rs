use crate::storage::sqlite::RepoError;
use thiserror::Error;

/// The service layer's single error type — covers every business-logic failure across
/// items, team_items, templates, and teams (see the other files in `src/service/`). This
/// type carries no knowledge of any presentation layer (no HTTP status codes, no HTML, no
/// askama) — it's a pure business-error enum. `json_api` handlers convert it into a typed
/// Smithy error via `internal()`/`not_found()`; `web_ui` handlers convert it into an HTTP
/// response via the `IntoResponse` impl in `handlers::web_ui::error`, which is the one place
/// that's allowed to know what an error looks like on a rendered page.
#[derive(Debug, Error)]
pub enum ItemError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Internal(String),
}

impl From<RepoError> for ItemError {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::NotFound => ItemError::NotFound,
            RepoError::Internal(msg) => ItemError::Internal(msg),
        }
    }
}
