use std::{net::SocketAddr, sync::Arc};
mod auth;
mod domain;
mod email;
mod service;
mod storage;
mod web_ui;
mod json_api;

use crate::storage::sqlite::{
    ActivityLogRepo, EventSeriesRepo, ItemRepo, ProjectRepo, TeamRepo, UserRepo,
    activity_log::SqliteActivityLogRepo, create_pool, event_series::SqliteEventSeriesRepo,
    items::SqliteItemRepo, projects::SqliteProjectRepo, teams::SqliteTeamRepo,
    users::SqliteUserRepo,
};
use axum::{
    Extension, Router,
    body::boxed,
    middleware,
    response::Redirect,
    routing::{get, post, put},
};
use json_api::activity_log::{list_team_activity_log, undo_activity_log_entry};
use json_api::event_series::{
    create_event_series, get_event_series, list_event_series_for_project, update_event_series,
};
use json_api::invites::send_app_invite;
use json_api::item_import::{get_item_import_template, import_project_items};
use json_api::items::{list_assigned_items, list_items_due};
use json_api::project_items::{
    create_project_item, delete_project_item, get_project_item, list_project_items,
    update_project_item,
};
use json_api::projects::{
    attach_team_to_project, create_project, delete_project, detach_team_from_project,
    get_project, list_project_members, list_projects, set_project_member_role, update_project,
};
use json_api::team_templates::{create_team_template, list_team_templates};
use json_api::teams::{
    accept_team_invite, create_team, get_team, invite_team_member, leave_team, list_team_members,
    list_teams, set_team_member_role, update_team,
};
use json_api::templates::{create_template, list_templates};
use json_api::users::{get_user, list_users, update_user};
use web_ui::assigned_items::assigned_items_page;
use web_ui::login::login_page;
use web_ui::project_activity::*;
use web_ui::project_dashboard::*;
use web_ui::project_event_series::handlers::*;
use web_ui::project_events::handlers::*;
use web_ui::project_simple_lists::handlers::*;
use web_ui::project_tasks::handlers::*;
use web_ui::project_templates::handlers::*;
use web_ui::projects::projects_page;
use web_ui::teams::*;
use todo_server_sdk::{PeoplesRepublicOfLists, PeoplesRepublicOfListsConfig};
use tower::ServiceBuilder;
use tower_cookies::CookieManagerLayer;
use tower_http::services::ServeDir;

fn build_web_router() -> Router {
    Router::new()
        .route("/projects", get(projects_page))
        .route(
            "/projects/:project_id/tasks",
            get(project_tasks_page).post(create_project_task_form),
        )
        .route("/projects/:project_id/tasks/new", get(new_project_task_page))
        .route(
            "/projects/:project_id/tasks/calendar",
            get(project_tasks_calendar_page),
        )
        .route(
            "/projects/:project_id/tasks/batch",
            post(create_project_tasks_batch),
        )
        .route(
            "/projects/:project_id/tasks/:item_id",
            get(project_task_detail_page)
                .put(update_project_task_form)
                .delete(delete_project_task_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/edit",
            get(project_task_edit_page),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/children",
            get(project_task_children_fragment),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/save-as-template",
            post(save_project_task_as_template),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/promote",
            post(promote_project_task_form),
        )
        .route(
            "/projects/:project_id/tasks/:item_id/subordinate",
            post(subordinate_project_task_form),
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
            "/projects/:project_id/events/calendar",
            get(project_events_calendar_page),
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
            "/projects/:project_id/events/:item_id/children",
            get(project_event_children_fragment).post(create_project_event_child_form),
        )
        .route(
            "/projects/:project_id/events/:item_id/save-as-template",
            post(save_project_event_as_template),
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
            "/projects/:project_id/simple-lists/:item_id/children",
            get(project_simple_item_children_fragment),
        )
        .route(
            "/projects/:project_id/simple-lists/:item_id/promote",
            post(promote_project_simple_item_form),
        )
        .route(
            "/projects/:project_id/simple-lists/:item_id/subordinate",
            post(subordinate_project_simple_item_form),
        )
        .route("/assigned-items", get(assigned_items_page))
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
            get(project_event_series_page).post(create_project_event_series_form),
        )
        .route(
            "/projects/:project_id/series/new",
            get(new_project_event_series_page),
        )
        .route(
            "/projects/:project_id/dashboard",
            get(project_dashboard_page),
        )
        .route(
            "/projects/:project_id/dashboard/calendar",
            get(project_dashboard_calendar_page),
        )
        .route(
            "/projects/:project_id/dashboard/items/:item_id",
            put(toggle_project_dashboard_item_complete),
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
}

/// Routes that must stay reachable without a session — kept out of `build_web_router()` so
/// the auth middleware layered onto that router in `main()` never wraps this one too.
fn build_public_web_router() -> Router {
    Router::new().route("/login", get(login_page))
}

async fn web_not_found() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
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
    let event_series_repo = Arc::new(SqliteEventSeriesRepo(pool.clone())) as Arc<dyn EventSeriesRepo>;
    let activity_log_repo = Arc::new(SqliteActivityLogRepo(pool)) as Arc<dyn ActivityLogRepo>;

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
        .create_event_series(create_event_series)
        .get_event_series(get_event_series)
        .update_event_series(update_event_series)
        .list_event_series_for_project(list_event_series_for_project)
        .create_project_item(create_project_item)
        .get_project_item(get_project_item)
        .update_project_item(update_project_item)
        .delete_project_item(delete_project_item)
        .list_project_items(list_project_items)
        .import_project_items(import_project_items)
        .get_item_import_template(get_item_import_template)
        .build_unchecked();

    let api = ServiceBuilder::new()
        .layer(Extension(user_repo.clone()))
        .layer(Extension(item_repo.clone()))
        .layer(Extension(team_repo.clone()))
        .layer(Extension(project_repo.clone()))
        .layer(Extension(event_series_repo.clone()))
        .layer(Extension(activity_log_repo.clone()))
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
                .layer(Extension(event_series_repo.clone()))
                .layer(Extension(activity_log_repo.clone()))
                .layer(middleware::from_fn(auth::caddy_header_middleware));
            let public_web_router = build_public_web_router();

            Router::new()
                .route("/", get(|| async { Redirect::to("/web/projects") }))
                .nest("/api", api_router)
                .nest("/auth", auth_router)
                .nest("/web", web_router.merge(public_web_router))
                .nest_service("/web/static", web_static)
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
                .layer(Extension(event_series_repo))
                .layer(Extension(activity_log_repo))
                .layer(middleware::from_fn(auth::web_auth_middleware));
            let public_web_router = build_public_web_router();

            Router::new()
                .route("/", get(|| async { Redirect::to("/web/projects") }))
                .nest("/auth", auth_router)
                .nest("/api", api_router)
                .nest("/web", web_router.merge(public_web_router))
                .nest_service("/web/static", web_static)
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
