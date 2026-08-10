use std::{net::SocketAddr, sync::Arc};
mod auth;
mod domain;
mod email;
mod handlers;
mod service;
mod storage;

use crate::storage::sqlite::{
    ActivityLogRepo, ItemRepo, TeamRepo, UserRepo, activity_log::SqliteActivityLogRepo,
    create_pool, items::SqliteItemRepo, teams::SqliteTeamRepo, users::SqliteUserRepo,
};
use axum::{
    Extension, Router,
    body::boxed,
    middleware,
    response::Redirect,
    routing::{get, post, put},
};
use handlers::json_api::activity_log::{list_team_activity_log, undo_activity_log_entry};
use handlers::json_api::invites::send_app_invite;
use handlers::json_api::items::{
    create_item, delete_item, get_item, list_assigned_items, list_items, list_items_due,
    update_item,
};
use handlers::json_api::team_items::{
    create_team_item, delete_team_item, get_team_item, list_team_items, update_team_item,
};
use handlers::json_api::team_templates::{create_team_template, list_team_templates};
use handlers::json_api::teams::{
    accept_team_invite, create_team, get_team, invite_team_member, leave_team, list_team_members,
    list_teams, set_team_member_role, update_team,
};
use handlers::json_api::templates::{create_template, list_templates};
use handlers::json_api::users::{get_user, list_users, update_user};
use handlers::web_ui::assigned_items::assigned_items_page;
use handlers::web_ui::dashboard::*;
use handlers::web_ui::events::*;
use handlers::web_ui::login::login_page;
use handlers::web_ui::simple_lists::*;
use handlers::web_ui::tasks::*;
use handlers::web_ui::team_activity::*;
use handlers::web_ui::team_dashboard::*;
use handlers::web_ui::team_events::*;
use handlers::web_ui::team_simple_lists::*;
use handlers::web_ui::team_tasks::*;
use handlers::web_ui::team_templates::*;
use handlers::web_ui::teams::*;
use handlers::web_ui::templates::*;
use todo_server_sdk::{PeoplesRepublicOfLists, PeoplesRepublicOfListsConfig};
use tower::ServiceBuilder;
use tower_cookies::CookieManagerLayer;
use tower_http::services::ServeDir;

fn build_web_router() -> Router {
    Router::new()
        .route("/events", get(events_page).post(create_event_form))
        .route("/events/new", get(new_event_page))
        .route("/events/calendar", get(events_calendar_page))
        .route(
            "/events/:item_id",
            get(event_detail_page)
                .put(update_event_form)
                .delete(delete_event_form),
        )
        .route("/events/:item_id/edit", get(event_edit_page))
        .route(
            "/events/:item_id/children",
            get(event_children_fragment).post(create_event_child_form),
        )
        .route(
            "/events/:item_id/save-as-template",
            post(save_event_as_template),
        )
        .route(
            "/team-events/:team_id",
            get(team_events_page).post(create_team_event_form),
        )
        .route("/team-events/:team_id/new", get(new_team_event_page))
        .route(
            "/team-events/:team_id/calendar",
            get(team_events_calendar_page),
        )
        .route(
            "/team-events/:team_id/:item_id",
            get(team_event_detail_page)
                .put(update_team_event_form)
                .delete(delete_team_event_form),
        )
        .route(
            "/team-events/:team_id/:item_id/edit",
            get(team_event_edit_page),
        )
        .route(
            "/team-events/:team_id/:item_id/children",
            get(team_event_children_fragment).post(create_team_event_child_form),
        )
        .route("/tasks", get(tasks_page).post(create_task_form))
        .route("/tasks/new", get(new_task_page))
        .route("/tasks/calendar", get(tasks_calendar_page))
        .route("/tasks/batch", post(create_tasks_batch))
        .route(
            "/tasks/:item_id",
            get(task_detail_page)
                .put(update_task_form)
                .delete(delete_task_form),
        )
        .route("/tasks/:item_id/edit", get(task_edit_page))
        .route(
            "/tasks/:item_id/children",
            get(task_children_fragment),
        )
        .route(
            "/tasks/:item_id/save-as-template",
            post(save_task_as_template),
        )
        .route(
            "/simple-lists",
            get(simple_lists_page).post(create_simple_item_form),
        )
        .route("/simple-lists/new", get(new_simple_item_page))
        .route("/simple-lists/batch", post(create_simple_items_batch))
        .route(
            "/simple-lists/:item_id",
            get(simple_item_detail_page)
                .put(update_simple_item_form)
                .delete(delete_simple_item_form),
        )
        .route("/simple-lists/:item_id/edit", get(simple_item_edit_page))
        .route(
            "/simple-lists/:item_id/children",
            get(simple_item_children_fragment),
        )
        .route("/dashboard", get(dashboard_page))
        .route("/dashboard/items/:item_id", put(toggle_item_complete))
        .route(
            "/dashboard/team-items/:team_id/:item_id",
            put(toggle_team_item_complete),
        )
        .route("/assigned-items", get(assigned_items_page))
        .route("/team-dashboard/:team_id", get(team_dashboard_page))
        .route(
            "/team-dashboard/:team_id/items/:item_id",
            put(toggle_team_dashboard_item_complete),
        )
        .route(
            "/templates",
            get(templates_page).post(create_template_form),
        )
        .route(
            "/templates/:template_id",
            get(template_detail_page)
                .post(create_template_child_form)
                .put(update_template_form)
                .delete(delete_template_form),
        )
        .route("/templates/:template_id/edit", get(template_edit_page))
        .route(
            "/templates/:template_id/items",
            get(template_children_fragment),
        )
        .route(
            "/templates/:template_id/items/:item_id",
            get(template_child_detail_page)
                .put(update_template_child_form)
                .delete(delete_template_child_form),
        )
        .route(
            "/templates/:template_id/items/:item_id/edit",
            get(template_child_edit_page),
        )
        .route("/templates/:template_id/use", post(use_template_form))
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
        .route("/team-activity/:team_id", get(team_activity_page))
        .route(
            "/team-activity/:team_id/:entry_id/undo",
            put(undo_activity_log_entry_form),
        )
        .route(
            "/team-simple-lists/:team_id",
            get(team_simple_lists_page).post(create_team_simple_item_form),
        )
        .route(
            "/team-simple-lists/:team_id/new",
            get(new_team_simple_item_page),
        )
        .route(
            "/team-simple-lists/:team_id/batch",
            post(create_team_simple_items_batch),
        )
        .route(
            "/team-simple-lists/:team_id/:item_id",
            get(team_simple_item_detail_page)
                .put(update_team_simple_item_form)
                .delete(delete_team_simple_item_form),
        )
        .route(
            "/team-simple-lists/:team_id/:item_id/edit",
            get(team_simple_item_edit_page),
        )
        .route(
            "/team-simple-lists/:team_id/:item_id/children",
            get(team_simple_item_children_fragment),
        )
        .route(
            "/team-tasks/:team_id",
            get(team_tasks_page).post(create_team_task_form),
        )
        .route("/team-tasks/:team_id/new", get(new_team_task_page))
        .route(
            "/team-tasks/:team_id/batch",
            post(create_team_tasks_batch),
        )
        .route(
            "/team-tasks/:team_id/:item_id",
            get(team_task_detail_page)
                .put(update_team_task_form)
                .delete(delete_team_task_form),
        )
        .route(
            "/team-tasks/:team_id/:item_id/edit",
            get(team_task_edit_page),
        )
        .route(
            "/team-tasks/:team_id/:item_id/children",
            get(team_task_children_fragment),
        )
        .route(
            "/team-tasks/:team_id/:item_id/save-as-template",
            post(save_team_task_as_template),
        )
        .route(
            "/team-events/:team_id/:item_id/save-as-template",
            post(save_team_event_as_template),
        )
        .route(
            "/team-templates/:team_id",
            get(team_templates_page).post(create_team_template_form),
        )
        .route(
            "/team-templates/:team_id/:template_id",
            get(team_template_detail_page)
                .post(create_team_template_child_form)
                .put(update_team_template_form)
                .delete(delete_team_template_form),
        )
        .route(
            "/team-templates/:team_id/:template_id/edit",
            get(team_template_edit_page),
        )
        .route(
            "/team-templates/:team_id/:template_id/items",
            get(team_template_children_fragment),
        )
        .route(
            "/team-templates/:team_id/:template_id/items/:item_id",
            get(team_template_child_detail_page)
                .put(update_team_template_child_form)
                .delete(delete_team_template_child_form),
        )
        .route(
            "/team-templates/:team_id/:template_id/items/:item_id/edit",
            get(team_template_child_edit_page),
        )
        .route(
            "/team-templates/:team_id/:template_id/use",
            post(use_team_template_form),
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
    let activity_log_repo = Arc::new(SqliteActivityLogRepo(pool)) as Arc<dyn ActivityLogRepo>;

    let config = PeoplesRepublicOfListsConfig::builder().build();
    let smithy = PeoplesRepublicOfLists::builder(config)
        .get_user(get_user)
        .update_user(update_user)
        .list_users(list_users)
        .create_item(create_item)
        .get_item(get_item)
        .update_item(update_item)
        .delete_item(delete_item)
        .list_items(list_items)
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
        .create_team_item(create_team_item)
        .get_team_item(get_team_item)
        .update_team_item(update_team_item)
        .delete_team_item(delete_team_item)
        .list_team_items(list_team_items)
        .create_team_template(create_team_template)
        .list_team_templates(list_team_templates)
        .list_team_activity_log(list_team_activity_log)
        .undo_activity_log_entry(undo_activity_log_entry)
        .build_unchecked();

    let api = ServiceBuilder::new()
        .layer(Extension(user_repo.clone()))
        .layer(Extension(item_repo.clone()))
        .layer(Extension(team_repo.clone()))
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
                .layer(middleware::from_fn(auth::caddy_header_middleware));
            let auth_router = Router::new()
                .route("/me", get(auth::caddy_auth_me))
                .route("/token", get(auth::caddy_auth_token));

            let web_router = build_web_router()
                .layer(Extension(user_repo.clone()))
                .layer(Extension(item_repo.clone()))
                .layer(Extension(team_repo.clone()))
                .layer(Extension(activity_log_repo.clone()))
                .layer(middleware::from_fn(auth::caddy_header_middleware));
            let public_web_router = build_public_web_router();

            Router::new()
                .route("/", get(|| async { Redirect::to("/web/dashboard") }))
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
                .layer(middleware::from_fn(auth::jwt_auth_middleware));

            let web_router = build_web_router()
                .layer(Extension(user_repo))
                .layer(Extension(item_repo.clone()))
                .layer(Extension(team_repo.clone()))
                .layer(Extension(activity_log_repo))
                .layer(middleware::from_fn(auth::web_auth_middleware));
            let public_web_router = build_public_web_router();

            Router::new()
                .route("/", get(|| async { Redirect::to("/web/dashboard") }))
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
