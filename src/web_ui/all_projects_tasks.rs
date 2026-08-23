use axum::response::Html;

/// Stage 1 placeholder for the all-projects Tasks screen (`GET /web/tasks`) — see
/// `docs/all-projects-landing-plan.md` Stage 2, which replaces this with the real
/// cross-project task list.
pub async fn all_projects_tasks_page() -> Html<&'static str> {
    Html("TODO")
}
