pub mod assigned_items;
pub mod components;
pub mod error;
pub mod login;
pub mod main_dashboard;
pub mod nav;
pub mod project_activity;
pub mod project_dashboard;
pub mod project_events;
pub mod project_item_series;
pub mod project_simple_lists;
pub mod project_tasks;
pub mod project_templates;
pub mod projects;
pub mod teams;

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{Html, IntoResponse, Response};
use std::convert::Infallible;

/// The browser's timezone offset (`new Date().getTimezoneOffset()`). Two sources, in
/// priority order:
/// 1. The `X-Tz-Offset-Minutes` header, sent on every htmx-issued request (see the
///    `htmx:configRequest` listener in `templates/base.html`) — deliberately a header, not a
///    query/form parameter, since it's metadata about the client, not addressable resource
///    state, and must never end up in a URL. It did originally (as a `tzOffsetMinutes`
///    parameter), which boosted links' default `hx-push-url` behavior then pushed into the
///    browser's address bar/history, breaking `hx-select="#page"` navigation.
/// 2. A `tz_offset` cookie, set on every page load by an inline script in
///    `templates/base.html`'s `<head>`, as a fallback for requests that never run any htmx
///    JS at all — a raw full-page load (bookmark, hard refresh, typed URL) has no header,
///    only whatever cookie a *previous* visit already set.
///
/// Missing or unparseable from both defaults to `0` (UTC) — only a page's truly first-ever
/// load (before either mechanism has run once) hits that case.
pub struct TzOffset(pub i32);

#[async_trait]
impl<S> FromRequestParts<S> for TzOffset
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let from_header = parts
            .headers
            .get("X-Tz-Offset-Minutes")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<i32>().ok());

        let from_cookie = from_header
            .is_none()
            .then(|| {
                parts
                    .headers
                    .get(axum::http::header::COOKIE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| {
                        s.split(';').find_map(|part| {
                            part.trim()
                                .strip_prefix("tz_offset=")
                                .map(|v| v.to_string())
                        })
                    })
                    .and_then(|v| v.trim().parse::<i32>().ok())
            })
            .flatten();

        Ok(TzOffset(from_header.or(from_cookie).unwrap_or(0)))
    }
}

/// Converts a UTC instant to the user's local wall-clock time (same `local = utc - offset`
/// convention as `domain::recurrence::apply_end_of_day`) and formats it for display — due
/// dates are stored and computed in UTC, but should always be *shown* in local time.
pub fn to_local(
    dt: chrono::DateTime<chrono::Utc>,
    tz_offset_minutes: i32,
) -> chrono::DateTime<chrono::Utc> {
    dt - chrono::Duration::minutes(tz_offset_minutes as i64)
}

fn hx_redirect(location: String) -> Response {
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            location,
        )],
        Html(String::new()),
    )
        .into_response()
}
