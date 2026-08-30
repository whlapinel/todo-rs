use std::{net::SocketAddr, sync::Arc};
mod auth;
mod domain;
mod email;
mod json_api;
mod push;
mod service;
mod storage;
mod web_ui;

use crate::storage::attachment_store::{AttachmentStore, LocalFsAttachmentStore};
use crate::storage::sqlite::{
    ActivityLogRepo, AttachmentRepo, CalendarSubscriptionRepo, CommentRepo, ItemDependencyRepo,
    ItemRepo, ItemSeriesRepo, ProjectRepo, PushSubscriptionRepo, ReminderRepo, TeamRepo, UserRepo,
    activity_log::SqliteActivityLogRepo, attachments::SqliteAttachmentRepo,
    calendar_subscriptions::SqliteCalendarSubscriptionRepo, comments::SqliteCommentRepo,
    create_pool, item_dependencies::SqliteItemDependencyRepo, item_series::SqliteItemSeriesRepo,
    items::SqliteItemRepo, projects::SqliteProjectRepo,
    push_subscriptions::SqlitePushSubscriptionRepo, reminders::SqliteReminderRepo,
    teams::SqliteTeamRepo, users::SqliteUserRepo,
};
use axum::{
    Extension, Router,
    body::boxed,
    middleware,
    response::Redirect,
    routing::{delete, get, post, put},
};
use json_api::activity_log::{list_team_activity_log, undo_activity_log_entry};
use json_api::calendar_subscriptions::{
    create_calendar_subscription, delete_calendar_subscription, list_calendar_subscriptions,
};
use json_api::comments::{
    create_item_comment, delete_item_comment, list_item_comments, update_item_comment,
};
use json_api::invites::send_app_invite;
use json_api::item_import::{get_item_import_template, import_project_items};
use json_api::item_series::{
    create_item_series, delete_item_series, get_item_series, list_item_series_for_project,
    update_item_series,
};
use json_api::items::{list_assigned_items, list_items_due};
use json_api::project_items::{
    create_project_item, delete_project_item, get_project_item, list_project_items,
    update_project_item,
};
use json_api::projects::{
    attach_team_to_project, create_project, delete_project, detach_team_from_project, get_project,
    list_project_members, list_projects, set_project_member_role, update_project,
};
use json_api::team_templates::{create_team_template, list_team_templates};
use json_api::teams::{
    accept_team_invite, create_team, get_team, invite_team_member, leave_team, list_team_members,
    list_teams, set_team_member_role, update_team,
};
use json_api::templates::{create_template, list_templates};
use json_api::users::{get_user, list_users, update_user};
use todo_server_sdk::{PeoplesRepublicOfLists, PeoplesRepublicOfListsConfig};
use tower::ServiceBuilder;
use tower_cookies::CookieManagerLayer;
use tower_http::services::ServeDir;
use web_ui::all_projects_events::{all_projects_events_page, new_all_projects_event_dialog};
use web_ui::all_projects_tasks::{
    all_projects_tasks_page, new_all_projects_task_dialog, toggle_all_projects_task_complete,
};
use web_ui::assigned_items::assigned_items_page;
use web_ui::login::login_page;
use web_ui::main_calendar::*;
use web_ui::notifications::{dismiss_notification_form, notifications_badge, notifications_list};
use web_ui::project_activity::*;
use web_ui::project_calendar::*;
use web_ui::project_calendar_subscriptions::*;
use web_ui::project_events::handlers::*;
use web_ui::project_item_series::handlers::*;
use web_ui::project_simple_lists::handlers::*;
use web_ui::project_tasks::handlers::*;
use web_ui::project_templates::handlers::*;
use web_ui::projects::{
    create_project_form, delete_project_dialog, delete_project_form, projects_page,
};
use web_ui::push::{push_public_key, push_subscribe, push_unsubscribe};
use web_ui::teams::*;

fn build_web_router() -> Router {
    Router::new()
        // Stage 2 of docs/all-projects-landing-plan.md: the real cross-project Tasks screen.
        // `/events` is still Stage 1's placeholder, replaced by Stage 3.
        .route("/tasks", get(all_projects_tasks_page))
        .route("/tasks/new", get(new_all_projects_task_dialog))
        .route(
            "/tasks/projects/:project_id/items/:item_id",
            put(toggle_all_projects_task_complete),
        )
        .route("/events", get(all_projects_events_page))
        .route("/events/new", get(new_all_projects_event_dialog))
        .route("/calendar", get(main_calendar_page))
        .route("/calendar/day", get(main_calendar_day_fragment))
        .route(
            "/calendar/projects/:project_id/items/:item_id",
            put(toggle_main_calendar_item_complete),
        )
        // Stage 8 of docs/calendar-day-drawer-plan.md: legacy "Dashboard"-named paths, kept
        // alive only as redirects (cheap insurance against a stale link/bookmark) — see
        // `redirect_main_dashboard`/`redirect_main_dashboard_list`'s doc comments.
        .route("/dashboard", get(redirect_main_dashboard))
        .route("/dashboard/list", get(redirect_main_dashboard_list))
        .route("/dashboard/calendar", get(redirect_main_dashboard))
        .route("/projects", get(projects_page).post(create_project_form))
        .route("/projects/:project_id/delete", get(delete_project_dialog))
        .route("/projects/:project_id", delete(delete_project_form))
        .route(
            "/projects/:project_id/tasks",
            get(project_tasks_page).post(create_project_task_form),
        )
        .route(
            "/projects/:project_id/tasks/new",
            get(new_project_task_page),
        )
        .route(
            "/projects/:project_id/tasks/batch",
            post(create_project_tasks_batch),
        )
        .route(
            "/projects/:project_id/tasks/batch/priority",
            put(batch_set_priority_form),
        )
        .route(
            "/projects/:project_id/tasks/batch/dates",
            put(batch_set_dates_form),
        )
        .route(
            "/projects/:project_id/tasks/batch/offset",
            put(batch_set_offset_form),
        )
        .route(
            "/projects/:project_id/tasks/batch/assignee",
            put(batch_set_assignee_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id",
            get(project_task_detail_page)
                .put(update_project_task_form)
                .delete(delete_project_task_form),
        )
        .route(
            "/projects/:project_id/tasks/:task_id/reschedule",
            get(get_reschedule_task),
        )
        .route(
            "/projects/:project_id/tasks/:task_id/assign",
            get(get_quick_assign_task),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/add-child",
            get(get_add_child_task),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/edit",
            get(project_task_edit_page),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/comments",
            post(create_project_task_comment_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/comments/:comment_id",
            put(update_project_task_comment_form).delete(delete_project_task_comment_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/comments/:comment_id/edit",
            get(edit_project_task_comment_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/attachments/:attachment_id",
            get(download_project_task_attachment).delete(delete_project_task_attachment_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/children",
            get(project_task_children_fragment),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/duplicate",
            post(duplicate_project_task_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/save-as-template",
            post(save_project_task_as_template),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/move",
            get(get_move_task_dialog).post(move_project_task_form),
        )
        .route(
            "/projects/:project_id/events",
            get(project_events_page).post(create_project_event_form),
        )
        .route(
            "/projects/:project_id/events/new",
            get(new_project_event_page),
        )
        .route(
            "/projects/:project_id/events/:item_id",
            get(project_event_detail_page)
                .put(update_project_event_form)
                .delete(delete_project_event_form),
        )
        .route(
            "/projects/:project_id/events/:item_id/edit",
            get(project_event_edit_page),
        )
        .route(
            "/projects/:project_id/events/:item_id/reschedule",
            get(get_reschedule_event),
        )
        .route(
            "/projects/:project_id/events/:item_id/children",
            get(project_event_children_fragment).post(create_project_event_child_form),
        )
        .route(
            "/projects/:project_id/events/:item_id/add-linked-task",
            get(get_add_linked_task_dialog),
        )
        .route(
            "/projects/:project_id/events/:item_id/duplicate",
            post(duplicate_project_event_form),
        )
        .route(
            "/projects/:project_id/events/:item_id/save-as-template",
            post(save_project_event_as_template),
        )
        .route(
            "/projects/:project_id/calendar-subscriptions",
            get(project_calendar_subscriptions_page)
                .post(create_project_calendar_subscription_form),
        )
        .route(
            "/projects/:project_id/calendar-subscriptions/:subscription_id",
            delete(delete_project_calendar_subscription_form),
        )
        .route(
            "/calendar-subscriptions/ical-help",
            get(ical_help_dialog_fragment),
        )
        .route(
            "/projects/:project_id/simple-lists",
            get(project_simple_lists_page).post(create_project_simple_item_form),
        )
        .route(
            "/projects/:project_id/simple-lists/new",
            get(new_project_simple_item_page),
        )
        .route(
            "/projects/:project_id/simple-lists/batch",
            post(create_project_simple_items_batch),
        )
        .route(
            "/projects/:project_id/simple-lists/:item_id",
            get(project_simple_item_detail_page)
                .put(update_project_simple_item_form)
                .delete(delete_project_simple_item_form),
        )
        .route(
            "/projects/:project_id/simple-lists/:item_id/edit",
            get(project_simple_item_edit_page),
        )
        .route(
            "/projects/:project_id/simple-lists/:item_id/add-child",
            get(get_add_child_simple_item),
        )
        .route(
            "/projects/:project_id/simple-lists/:item_id/children",
            get(project_simple_item_children_fragment),
        )
        .route(
            "/projects/:project_id/simple-lists/:item_id/move",
            get(get_move_simple_item_dialog).post(move_project_simple_item_form),
        )
        .route("/assigned-items", get(assigned_items_page))
        .route("/notifications/badge", get(notifications_badge))
        .route("/notifications", get(notifications_list))
        .route(
            "/notifications/:id/dismiss",
            post(dismiss_notification_form),
        )
        .route("/push/public-key", get(push_public_key))
        .route("/push/subscribe", post(push_subscribe))
        .route("/push/unsubscribe", post(push_unsubscribe))
        .route(
            "/projects/:project_id/templates",
            get(project_templates_page).post(create_project_template_form),
        )
        .route(
            "/projects/:project_id/templates/:template_id",
            get(project_template_detail_page)
                .post(create_project_template_child_form)
                .put(update_project_template_form)
                .delete(delete_project_template_form),
        )
        .route(
            "/projects/:project_id/templates/:template_id/edit",
            get(project_template_edit_page),
        )
        .route(
            "/projects/:project_id/templates/:template_id/items",
            get(project_template_children_fragment),
        )
        .route(
            "/projects/:project_id/templates/:template_id/items/:item_id",
            get(project_template_child_detail_page)
                .put(update_project_template_child_form)
                .delete(delete_project_template_child_form),
        )
        .route(
            "/projects/:project_id/templates/:template_id/items/:item_id/edit",
            get(project_template_child_edit_page),
        )
        .route(
            "/projects/:project_id/templates/:template_id/use",
            post(use_project_template_form),
        )
        .route(
            "/projects/:project_id/series",
            get(project_item_series_page).post(create_project_item_series_form),
        )
        .route(
            "/projects/:project_id/series/new",
            get(new_project_item_series_page),
        )
        .route(
            "/projects/:project_id/series/:series_id/edit",
            get(edit_project_item_series_page),
        )
        .route(
            "/projects/:project_id/series/:series_id",
            put(update_project_item_series_form).delete(delete_project_item_series_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/duplicate",
            post(duplicate_project_item_series_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts",
            get(project_item_series_occurrence_detail_page)
                .post(materialize_project_item_series_occurrence_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/edit",
            get(project_item_series_occurrence_edit_page),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/task",
            put(update_project_task_series_occurrence_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/complete",
            post(complete_project_item_series_occurrence_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/task-children",
            get(get_project_task_series_occurrence_add_child_dialog)
                .post(create_project_task_series_occurrence_child_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/event",
            put(update_project_event_series_occurrence_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/event-children",
            post(create_project_event_series_occurrence_child_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/skip",
            post(skip_project_item_series_occurrence_form),
        )
        .route(
            "/projects/:project_id/series/:series_id/occurrences/:occurrence_ts/unskip",
            post(unskip_project_item_series_occurrence_form),
        )
        .route("/projects/:project_id/calendar", get(project_calendar_page))
        .route(
            "/projects/:project_id/calendar/day",
            get(project_calendar_day_fragment),
        )
        .route(
            "/projects/:project_id/calendar/items/:item_id",
            put(toggle_project_calendar_item_complete),
        )
        // Stage 8 of docs/calendar-day-drawer-plan.md: legacy "Dashboard"-named paths, kept
        // alive only as redirects — see `redirect_project_dashboard`/
        // `redirect_project_dashboard_list`'s doc comments.
        .route(
            "/projects/:project_id/dashboard",
            get(redirect_project_dashboard),
        )
        .route(
            "/projects/:project_id/dashboard/list",
            get(redirect_project_dashboard_list),
        )
        .route(
            "/projects/:project_id/dashboard/calendar",
            get(redirect_project_dashboard),
        )
        .route("/projects/:project_id/activity", get(project_activity_page))
        .route(
            "/projects/:project_id/activity/:entry_id/undo",
            put(undo_project_activity_log_entry_form),
        )
        .route("/teams", get(teams_page).post(create_team_form))
        .route(
            "/teams/:team_id",
            get(team_detail_page).put(update_team_form),
        )
        .route("/teams/:team_id/invite", post(invite_team_member_form))
        .route("/teams/:team_id/accept", post(accept_team_invite_form))
        .route("/teams/:team_id/leave", post(leave_team_form))
        .route(
            "/teams/:team_id/members/:user_id/role",
            put(set_team_member_role_form),
        )
        // Without this, a path under /web/ that doesn't match any route above falls through
        // to the outer router's fallback_service (the SPA's frontend/dist/index.html) — a
        // different document with no #page element, which silently renders blank when a
        // boosted link's inherited hx-select="#page" finds nothing to swap in. A real 404
        // here fails loudly instead, for any not-yet-built /web/* route or plain typo.
        .fallback(web_not_found)
        // axum's own default (2MB, applied automatically to any body-consuming
        // extractor including `Multipart`) is well under a typical photo — raised here,
        // for every /web/* route, to `service::attachments::MAX_ATTACHMENT_SIZE_BYTES`
        // plus headroom for multipart boundary/header overhead, so a large-but-valid
        // attachment upload doesn't get rejected by this layer before ever reaching
        // that service-level check.
        .layer(axum::extract::DefaultBodyLimit::max(
            service::attachments::MAX_ATTACHMENT_SIZE_BYTES + 1024 * 1024,
        ))
}

/// Routes that must stay reachable without a session — kept out of `build_web_router()` so
/// the auth middleware layered onto that router in `main()` never wraps this one too.
fn build_public_web_router() -> Router {
    Router::new().route("/login", get(login_page))
}

async fn web_not_found() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
}

/// Served at `/web/sw.js` rather than relying on the `/web/static` ServeDir mount so it can
/// carry a `Service-Worker-Allowed: /web/` header — without it, `navigator.serviceWorker
/// .register('/web/sw.js', { scope: '/web/' })` (see base.html) would be rejected by the
/// browser, since a script's default scope is limited to its own directory
/// (`/web/static/`) and only that header can widen it to cover the whole app.
async fn service_worker() -> axum::response::Response {
    use axum::response::IntoResponse;
    match tokio::fs::read("static/sw.js").await {
        Ok(bytes) => (
            [
                (http::header::CONTENT_TYPE, "application/javascript"),
                (http::header::CACHE_CONTROL, "no-cache"),
                (
                    http::header::HeaderName::from_static("service-worker-allowed"),
                    "/web/",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => http::StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let db_url = std::env::var("TODO_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://todo.db?mode=rwc".to_string());
    let pool = create_pool(&db_url).await.expect("failed to open database");
    let user_repo = Arc::new(SqliteUserRepo(pool.clone())) as Arc<dyn UserRepo>;
    let item_repo = Arc::new(SqliteItemRepo(pool.clone())) as Arc<dyn ItemRepo>;
    let team_repo = Arc::new(SqliteTeamRepo(pool.clone())) as Arc<dyn TeamRepo>;
    let project_repo = Arc::new(SqliteProjectRepo(pool.clone())) as Arc<dyn ProjectRepo>;
    let series_repo = Arc::new(SqliteItemSeriesRepo(pool.clone())) as Arc<dyn ItemSeriesRepo>;
    let activity_log_repo =
        Arc::new(SqliteActivityLogRepo(pool.clone())) as Arc<dyn ActivityLogRepo>;
    let reminder_repo = Arc::new(SqliteReminderRepo(pool.clone())) as Arc<dyn ReminderRepo>;
    let item_dependency_repo =
        Arc::new(SqliteItemDependencyRepo(pool.clone())) as Arc<dyn ItemDependencyRepo>;
    let push_subscription_repo =
        Arc::new(SqlitePushSubscriptionRepo(pool.clone())) as Arc<dyn PushSubscriptionRepo>;
    let comment_repo = Arc::new(SqliteCommentRepo(pool.clone())) as Arc<dyn CommentRepo>;
    let attachment_repo = Arc::new(SqliteAttachmentRepo(pool.clone())) as Arc<dyn AttachmentRepo>;
    let calendar_repo =
        Arc::new(SqliteCalendarSubscriptionRepo(pool)) as Arc<dyn CalendarSubscriptionRepo>;

    // Where attachment bytes live — see root CLAUDE.md's Attachments section. Defaults to
    // a plain local directory; point it at a mounted network share (Synology SMB/NFS
    // mount, etc.) to keep attachments off this server's own disk.
    let attachment_store = Arc::new(LocalFsAttachmentStore::new(
        std::env::var("TODO_ATTACHMENTS_DIR").unwrap_or_else(|_| "./attachments".to_string()),
    )) as Arc<dyn AttachmentStore>;

    // Background calendar sync sweep — see docs/google-calendar-import-plan.md's Stage 5.
    // This is the first tokio::spawn-based background loop in this codebase: every other
    // piece of work here happens synchronously inside a request handler. Sleep-first
    // (rather than sync-immediately-then-sleep) is deliberate: `create_calendar_subscription`
    // already syncs a subscription inline right after it's created, so the periodic sweep's
    // first useful run is naturally ~15 minutes after startup, avoiding a startup-time
    // thundering-herd fetch against every subscription across every project.
    {
        let calendar_repo = calendar_repo.clone();
        let item_repo = item_repo.clone();
        let user_repo = user_repo.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
                service::calendar_subscriptions::sync_all_subscriptions(
                    &calendar_repo,
                    &item_repo,
                    &user_repo,
                )
                .await;
            }
        });
    }

    // Background push-reminder sweep — see docs/push-notifications-plan.md. Same shape as
    // the calendar sync sweep above, but a shorter interval (reminders are minute-precision,
    // unlike calendar sync's 15-minute cadence) and gated on `PushConfig::from_env()`: if
    // push isn't configured on this server (no `TODO_VAPID_PRIVATE_KEY`/`TODO_VAPID_SUBJECT`),
    // no loop is spawned at all, mirroring `EmailConfig`'s "optional feature, absent if
    // unconfigured" pattern rather than spawning a loop that immediately no-ops every tick.
    if let Some(push_config) = push::PushConfig::from_env() {
        let push_config = Arc::new(push_config);
        let http_client = reqwest::Client::new();
        // Registers the ambient `PushRuntime` (see `service::push`'s doc comment) that
        // `service::team_items::create_team_item`/`update_team_item` reach for directly
        // when notifying on completion/assignment — those are ~30 call sites removed
        // from any handler that could otherwise hand them a `PushConfig`/`PushSubscriptionRepo`
        // via `Extension`.
        service::push::PushRuntime::init(
            push_config.clone(),
            push_subscription_repo.clone(),
            http_client.clone(),
        );
        let reminder_repo = reminder_repo.clone();
        let item_repo = item_repo.clone();
        let user_repo = user_repo.clone();
        let push_subscription_repo = push_subscription_repo.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                service::push::sweep_due_reminders(
                    &reminder_repo,
                    &item_repo,
                    &user_repo,
                    &push_subscription_repo,
                    &http_client,
                    &push_config,
                )
                .await;
            }
        });
    } else {
        tracing::info!(
            "push notifications disabled: set TODO_VAPID_PRIVATE_KEY and TODO_VAPID_SUBJECT to enable"
        );
    }

    let config = PeoplesRepublicOfListsConfig::builder().build();
    let smithy = PeoplesRepublicOfLists::builder(config)
        .get_user(get_user)
        .update_user(update_user)
        .list_users(list_users)
        .list_items_due(list_items_due)
        .list_assigned_items(list_assigned_items)
        .create_template(create_template)
        .list_templates(list_templates)
        .create_team(create_team)
        .get_team(get_team)
        .update_team(update_team)
        .list_teams(list_teams)
        .list_team_members(list_team_members)
        .invite_team_member(invite_team_member)
        .accept_team_invite(accept_team_invite)
        .leave_team(leave_team)
        .set_team_member_role(set_team_member_role)
        .send_app_invite(send_app_invite)
        .create_team_template(create_team_template)
        .list_team_templates(list_team_templates)
        .list_team_activity_log(list_team_activity_log)
        .undo_activity_log_entry(undo_activity_log_entry)
        .create_project(create_project)
        .get_project(get_project)
        .update_project(update_project)
        .delete_project(delete_project)
        .list_projects(list_projects)
        .list_project_members(list_project_members)
        .set_project_member_role(set_project_member_role)
        .attach_team_to_project(attach_team_to_project)
        .detach_team_from_project(detach_team_from_project)
        .create_item_series(create_item_series)
        .get_item_series(get_item_series)
        .update_item_series(update_item_series)
        .delete_item_series(delete_item_series)
        .list_item_series_for_project(list_item_series_for_project)
        .create_project_item(create_project_item)
        .get_project_item(get_project_item)
        .update_project_item(update_project_item)
        .delete_project_item(delete_project_item)
        .list_project_items(list_project_items)
        .import_project_items(import_project_items)
        .get_item_import_template(get_item_import_template)
        .create_calendar_subscription(create_calendar_subscription)
        .list_calendar_subscriptions(list_calendar_subscriptions)
        .delete_calendar_subscription(delete_calendar_subscription)
        .create_item_comment(create_item_comment)
        .list_item_comments(list_item_comments)
        .update_item_comment(update_item_comment)
        .delete_item_comment(delete_item_comment)
        .build_unchecked();

    let api = ServiceBuilder::new()
        .layer(Extension(user_repo.clone()))
        .layer(Extension(item_repo.clone()))
        .layer(Extension(team_repo.clone()))
        .layer(Extension(project_repo.clone()))
        .layer(Extension(series_repo.clone()))
        .layer(Extension(activity_log_repo.clone()))
        .layer(Extension(reminder_repo.clone()))
        .layer(Extension(item_dependency_repo.clone()))
        .layer(Extension(calendar_repo.clone()))
        .layer(Extension(comment_repo.clone()))
        .map_response(|res: http::Response<_>| res.map(boxed))
        .service(smithy);

    let web_static = ServeDir::new("static");

    let auth_mode = std::env::var("TODO_AUTH_MODE").unwrap_or_else(|_| "internal".to_string());
    tracing::info!(auth_mode, "auth mode");

    let app = match auth_mode.as_str() {
        "caddy" => {
            let jwt_secret = std::env::var("TODO_JWT_SECRET")
                .expect("TODO_JWT_SECRET required (used to verify Bearer tokens from the CLI/MCP)");
            let api_router = Router::new()
                .route_service("/users", api.clone())
                .route_service("/users/*path", api.clone())
                .route_service("/teams", api.clone())
                .route_service("/teams/*path", api.clone())
                .route_service("/projects", api.clone())
                .route_service("/projects/*path", api.clone())
                .route_service("/items", api.clone())
                .route_service("/items/*path", api.clone())
                .layer(middleware::from_fn(auth::caddy_header_middleware));
            let auth_router = Router::new()
                .route("/me", get(auth::caddy_auth_me))
                .route("/token", get(auth::caddy_auth_token));

            let web_router = build_web_router()
                .layer(Extension(user_repo.clone()))
                .layer(Extension(item_repo.clone()))
                .layer(Extension(team_repo.clone()))
                .layer(Extension(project_repo.clone()))
                .layer(Extension(series_repo.clone()))
                .layer(Extension(activity_log_repo.clone()))
                .layer(Extension(reminder_repo.clone()))
                .layer(Extension(item_dependency_repo.clone()))
                .layer(Extension(calendar_repo.clone()))
                .layer(Extension(push_subscription_repo.clone()))
                .layer(Extension(comment_repo.clone()))
                .layer(Extension(attachment_repo.clone()))
                .layer(Extension(attachment_store.clone()))
                .layer(middleware::from_fn(auth::caddy_header_middleware));
            let public_web_router = build_public_web_router();

            Router::new()
                .route("/", get(|| async { Redirect::to("/web/tasks") }))
                .nest("/api", api_router)
                .nest("/auth", auth_router)
                .nest("/web", web_router.merge(public_web_router))
                .nest_service("/web/static", web_static)
                .route("/web/sw.js", get(service_worker))
                .layer(Extension(user_repo))
                // Must sit outside (added after) both nested routers' own internal
                // `.layer(middleware::from_fn(caddy_header_middleware))` calls — axum
                // layers added later wrap outer, so an Extension layered *inside*
                // build_web_router()/api_router's own chain never actually runs before
                // caddy_header_middleware's own pre-processing code executes (it only
                // runs once that middleware calls `next.run()`). The narrow admin
                // bootstrap sync in caddy_header_middleware reads TeamRepo from
                // extensions during that pre-processing step, so it needs a copy
                // that's genuinely outer, same as `user_repo` right above.
                .layer(Extension(team_repo))
                .layer(Extension(project_repo))
                .layer(Extension(Arc::new(jwt_secret)))
                .layer(CookieManagerLayer::new())
        }
        _ => {
            let google_client_id = std::env::var("TODO_GOOGLE_CLIENT_ID")
                .expect("TODO_GOOGLE_CLIENT_ID required (or set TODO_AUTH_MODE=caddy)");
            let google_client_secret = std::env::var("TODO_GOOGLE_CLIENT_SECRET")
                .expect("TODO_GOOGLE_CLIENT_SECRET required (or set TODO_AUTH_MODE=caddy)");
            let jwt_secret = std::env::var("TODO_JWT_SECRET")
                .expect("TODO_JWT_SECRET required (or set TODO_AUTH_MODE=caddy)");
            let base_url = std::env::var("TODO_BASE_URL").expect("TODO_BASE_URL required");

            let app_state = Arc::new(auth::AppState::new(
                google_client_id,
                google_client_secret,
                base_url,
                jwt_secret,
                user_repo.clone(),
                project_repo.clone(),
            ));

            let auth_router = Router::new()
                .route("/google", get(auth::auth_login))
                .route("/callback", get(auth::auth_callback))
                .route("/logout", get(auth::auth_logout))
                .route("/me", get(auth::auth_me))
                .route("/token", get(auth::auth_token));

            let api_router = Router::new()
                .route_service("/users", api.clone())
                .route_service("/users/*path", api.clone())
                .route_service("/teams", api.clone())
                .route_service("/teams/*path", api.clone())
                .route_service("/projects", api.clone())
                .route_service("/projects/*path", api.clone())
                .route_service("/items", api.clone())
                .route_service("/items/*path", api.clone())
                .layer(middleware::from_fn(auth::jwt_auth_middleware));

            let web_router = build_web_router()
                .layer(Extension(user_repo))
                .layer(Extension(item_repo.clone()))
                .layer(Extension(team_repo.clone()))
                .layer(Extension(project_repo))
                .layer(Extension(series_repo))
                .layer(Extension(activity_log_repo))
                .layer(Extension(reminder_repo))
                .layer(Extension(item_dependency_repo))
                .layer(Extension(calendar_repo))
                .layer(Extension(push_subscription_repo))
                .layer(Extension(comment_repo))
                .layer(Extension(attachment_repo))
                .layer(Extension(attachment_store))
                .layer(middleware::from_fn(auth::web_auth_middleware));
            let public_web_router = build_public_web_router();

            Router::new()
                .route("/", get(|| async { Redirect::to("/web/tasks") }))
                .nest("/auth", auth_router)
                .nest("/api", api_router)
                .nest("/web", web_router.merge(public_web_router))
                .nest_service("/web/static", web_static)
                .route("/web/sw.js", get(service_worker))
                .layer(Extension(app_state))
                .layer(CookieManagerLayer::new())
        }
    };

    let bind: SocketAddr = std::env::var("TODO_BIND")
        .expect("TODO_BIND required")
        .parse()
        .expect("invalid BIND address");
    tracing::info!("listening on {}", bind);
    axum::Server::bind(&bind)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
