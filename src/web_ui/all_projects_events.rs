use axum::response::Html;

/// Stage 1 placeholder for the all-projects Events screen (`GET /web/events`) — see
/// `docs/all-projects-landing-plan.md` Stage 3, which replaces this with the real
/// cross-project event list.
pub async fn all_projects_events_page() -> Html<&'static str> {
    Html("TODO")
}
