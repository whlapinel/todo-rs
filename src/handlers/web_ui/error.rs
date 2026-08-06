use crate::service::error::ItemError;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

/// How a service-layer `ItemError` maps onto an HTTP status — a `web_ui`-specific decision
/// (the JSON API doesn't use this at all; Smithy's error serialization goes through
/// `handlers::json_api`'s own `internal()`/`not_found()` helpers instead, never through
/// `axum::response::IntoResponse`).
fn status_code(e: &ItemError) -> StatusCode {
    match e {
        ItemError::NotFound => StatusCode::NOT_FOUND,
        ItemError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ItemError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `askama::Error` only means something to a template-rendering caller — kept here rather
/// than on `ItemError` itself so the service layer never has to depend on askama.
impl From<askama::Error> for ItemError {
    fn from(e: askama::Error) -> Self {
        ItemError::Internal(e.to_string())
    }
}

/// Renders as a small Tailwind-styled HTML fragment carrying the error message, so an
/// htmx-targeted request that fails still shows the caller *why* — a bare status code alone
/// reaches the browser with no body at all. `ItemError` is a type local to this crate, so
/// implementing a foreign trait (`IntoResponse`) for it here, in the one module that's
/// actually allowed to know about HTML, is permitted regardless of where the type itself is
/// defined (`service::error`).
impl IntoResponse for ItemError {
    fn into_response(self) -> Response {
        let status = status_code(&self);
        let message = escape_html(&self.to_string());
        (
            status,
            Html(format!(
                r#"<div class="rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700">{message}</div>"#
            )),
        )
            .into_response()
    }
}
