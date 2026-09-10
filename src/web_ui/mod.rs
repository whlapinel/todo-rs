pub mod all_projects_events;
pub mod all_projects_tasks;
pub mod assigned_items;
pub mod components;
pub mod error;
pub mod list_filters;
pub mod login;
pub mod main_calendar;
pub mod nav;
pub mod notifications;
pub mod project_activity;
pub mod project_calendar;
pub mod project_calendar_subscriptions;
pub mod project_events;
pub mod project_item_series;
pub mod project_simple_lists;
pub mod project_tasks;
pub mod project_templates;
pub mod projects;
pub mod push;
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

fn display_date_part(date: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    if date.year() == chrono::Utc::now().year() {
        date.format("%a %-d %b").to_string()
    } else {
        date.format("%a %-d %b %Y").to_string()
    }
}

/// The app-wide human-readable date display format (docs/issues_and_features.md's "Change ALL
/// date displays" item): `"Thu 27 Aug"`, or `"Thu 27 Aug 2027"` when `local`'s year isn't the
/// current one. This is the single place that format lives — every read-only date shown to a
/// user (row dates, detail views, activity feed, series anchors, sync timestamps, calendar
/// drawer titles) should go through this or `format_display_naive_date`, never format a date
/// for display with an ad hoc `.format(...)` call of its own. Editable `<input type="date">`/
/// `<input type="time">` values are a different concern (those need literal `%Y-%m-%d`/`%H:%M`
/// for the browser) and must keep formatting those directly, not through this function.
pub fn format_display_date(local: chrono::DateTime<chrono::Utc>, with_time: bool) -> String {
    let date_part = display_date_part(local.date_naive());
    if with_time {
        format!("{date_part}, {}", local.format("%-I:%M %p"))
    } else {
        date_part
    }
}

/// `format_display_date`'s counterpart for a bare `NaiveDate` (no time-of-day component at
/// all) — e.g. the calendar day-drawer title.
pub fn format_display_naive_date(date: chrono::NaiveDate) -> String {
    display_date_part(date)
}

/// Human-readable labels for an item's reminder rows (`ReminderRepo::list_for_item`) on a
/// read-only detail view, e.g. `"Due reminder: Thu 27 Aug, 5:00 PM"`. Shared by
/// `project_tasks`/`project_events` rather than duplicated per screen — unlike this module's
/// other small per-screen helpers (e.g. `blocked_by_names_for`), the `ReminderKind` → label
/// mapping is identical for both and has nothing screen-specific to vary.
pub fn reminder_labels(reminders: &[crate::domain::reminder::Reminder], tz: i32) -> Vec<String> {
    use crate::domain::reminder::ReminderKind;
    reminders
        .iter()
        .map(|r| {
            let prefix = match r.kind {
                ReminderKind::Due => "Due reminder",
                ReminderKind::ScheduledStart => "Scheduled start reminder",
                ReminderKind::ScheduledEnd => "Scheduled end reminder",
            };
            format!(
                "{prefix}: {}",
                format_display_date(to_local(r.remind_at, tz), true)
            )
        })
        .collect()
}

/// Resolves which project an all-projects "+ New" dialog should target — Stage 3 of
/// `docs/dialog-item-forms-plan.md`. The query-string `project` param if given and it's one of
/// the requester's own projects, else the first project `ProjectRepo::list_for_user` returned.
/// Used to consult `users.personal_project_id` as a middle fallback before falling through to
/// "first project"; dropped per the "Eliminate the 'personal project' concept" entry in
/// `docs/issues_and_features.md` — nothing distinguishes that project from any other team-less
/// one anymore, so it's no better a default than the first project already was. Shared between
/// `all_projects_tasks`/`all_projects_events`'s own "+ New" dialogs since the cascade itself is
/// identical, not screen-specific — unlike the small per-screen row/filter helpers elsewhere in
/// this codebase that are deliberately duplicated instead of shared.
pub(crate) fn resolve_new_item_project<'a>(
    user_projects: &'a [crate::domain::project::Project],
    query_project: Option<&str>,
) -> Option<&'a crate::domain::project::Project> {
    query_project
        .and_then(|id| user_projects.iter().find(|p| p.id == id))
        .or_else(|| user_projects.first())
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

/// Fires `base.html`'s page-level confirmation banner (`#toast-region`) via htmx's `HX-Trigger`
/// response header, which dispatches a `prl:toast` event carrying `message` on the element that
/// made the request.
///
/// This replaced the per-row `Row::confirmation` badge (a `.toast-fade` `<span>` rendered inside
/// the row itself). A row is the wrong place for a confirmation for two reasons that both bit:
/// the confirmation is only visible for as long as the row is, so an action whose whole point is
/// to take the row *off* the list had to keep it artificially alive to say anything at all (the
/// retired `dismiss_after_ms`/`just_completed_item_id` pair); and an action that returns no row —
/// or rebuilds the whole list — had nowhere to put one. A banner outside `#page` has neither
/// problem, and it doesn't block clicks (`pointer-events-none`), so nothing has to wait for it.
pub(crate) fn hx_toast(mut response: Response, message: &str) -> Response {
    let payload = serde_json::json!({ "prl:toast": { "message": message } }).to_string();
    if let Ok(value) = axum::http::HeaderValue::from_str(&payload) {
        response.headers_mut().insert("HX-Trigger", value);
    }
    response
}

/// Overrides the requester's own `hx-swap` with `delete`, wrapped in a View Transition — the
/// response body is ignored and the target element is removed.
///
/// Used for the one case where a successful action means "this row is no longer on this list":
/// completing an item while the screen is hiding completed ones. The row used to linger for
/// 1800ms with a fading badge (`Row::dismiss_after_ms`, a `setTimeout` in `base.html`) before a
/// second, JS-driven removal — which split the removal across two moments and left the keyboard-
/// focus handling guessing which one it was in. Deleting it here, in the swap itself, makes the
/// removal a single event that `base.html`'s row-focus mechanism can hang off, and hands the
/// exit animation to the browser (every row carries a `view-transition-name` — see
/// `components/row.html`) instead of a hand-rolled opacity transition.
///
/// Callers must apply this only when the request actually targeted the row (`HX-Target`), since
/// `HX-Reswap` applies to the whole response and the same handler also serves the detail dialog's
/// own complete-toggle and the edit form's save, whose targets must not be deleted.
pub(crate) fn hx_delete_target(mut response: Response) -> Response {
    response.headers_mut().insert(
        "HX-Reswap",
        axum::http::HeaderValue::from_static("delete transition:true"),
    );
    response
}

/// Whether this request's `hx-target` resolved to `elt_id`'s own row — i.e. the request came
/// from the row (or its row-actions menu), not from a detail dialog or an edit form that happens
/// to PUT to the same URL. See `hx_delete_target`.
pub(crate) fn targets_row(headers: &axum::http::HeaderMap, item_id: &str) -> bool {
    headers
        .get("HX-Target")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t == format!("item-{item_id}"))
}
