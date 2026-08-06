pub mod dashboard;
pub mod hello_world;
pub mod items;

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::convert::Infallible;

/// The browser's timezone offset (`new Date().getTimezoneOffset()`), sent as the
/// `X-Tz-Offset-Minutes` header on every htmx-issued request (see the `htmx:configRequest`
/// listener in `templates/base.html`). Deliberately a header, not a query/form parameter —
/// it's metadata about the client, not addressable resource state, so it must never end up
/// in a URL. It did originally (as a `tzOffsetMinutes` parameter), which boosted links'
/// default `hx-push-url` behavior then pushed into the browser's address bar/history,
/// breaking `hx-select="#page"` navigation. Missing or unparseable defaults to `0` (UTC) —
/// every real htmx request sends it, so this only matters for non-JS/non-htmx requests.
pub struct TzOffset(pub i32);

#[async_trait]
impl<S> FromRequestParts<S> for TzOffset
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let offset = parts
            .headers
            .get("X-Tz-Offset-Minutes")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0);
        Ok(TzOffset(offset))
    }
}
