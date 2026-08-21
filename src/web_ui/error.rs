use crate::service::error::ItemError;
use askama::Template;
use axum::http::{HeaderValue, StatusCode};
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

/// Askama auto-escapes `message` (`.html` extension → the "html" escaper is on by default),
/// so no manual `escape_html` call is needed here the way the old raw-`format!` body needed
/// one.
#[derive(Template)]
#[template(path = "components/error_dialog.html")]
struct ErrorDialog {
    message: String,
}

/// Renders as a small Tailwind-styled modal dialog carrying the error message and an "OK"
/// button, so an htmx-targeted request that fails still shows the caller *why* — and, unlike
/// the old auto-fading toast, waits for the user to acknowledge it rather than disappearing
/// on a timer. `ItemError` is a type local to this crate, so implementing a foreign trait
/// (`IntoResponse`) for it here, in the one module that's actually allowed to know about
/// HTML, is permitted regardless of where the type itself is defined (`service::error`).
///
/// Every caller's own `hx-target`/`hx-select` (e.g. a row checkbox's `#item-{id}`, an edit
/// form's `#item-{id}-fields`) names an element that only exists in a *success* response —
/// this error body has neither, so naively honoring those would select nothing and swap in
/// an empty string. The `HX-Retarget`/`HX-Reswap`/`HX-Reselect` response headers below
/// override both for this response only, redirecting it to `#error-dialog` (`base.html`,
/// always present, outside `#page` so it survives boosted navigation) regardless of what the
/// triggering element requested — `HX-Reselect: unset` (rather than naming a sub-fragment id)
/// takes the whole response body, matching how `#action-dialog`'s own trigger buttons use
/// `hx-select="unset"` (`templates/components/row.html`). `base.html`'s
/// `htmx.config.responseHandling` override is the other half of this — htmx doesn't swap
/// 4xx/5xx bodies into the DOM at all by default. `#error-dialog` is a persistent `<dialog>`
/// (`base.html`), auto-opened by the same generic `htmx:afterSwap` listener that opens
/// `#action-dialog`.
impl IntoResponse for ItemError {
    fn into_response(self) -> Response {
        let status = status_code(&self);
        let message = self.to_string();
        let body = ErrorDialog { message }
            .render()
            .unwrap_or_else(|_| escape_html(&self.to_string()));
        let mut response = (status, Html(body)).into_response();
        let headers = response.headers_mut();
        headers.insert("HX-Retarget", HeaderValue::from_static("#error-dialog"));
        headers.insert("HX-Reswap", HeaderValue::from_static("innerHTML"));
        headers.insert("HX-Reselect", HeaderValue::from_static("unset"));
        response
    }
}
