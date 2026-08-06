use std::{net::SocketAddr, sync::Arc};
mod auth;
mod domain;
mod email;
mod handlers;
mod storage;

use crate::storage::sqlite::{
    ItemRepo, TeamRepo, UserRepo, create_pool, items::SqliteItemRepo, teams::SqliteTeamRepo,
    users::SqliteUserRepo,
};
use axum::{Extension, Router, body::boxed, middleware, routing::get};
use handlers::json_api::invites::send_app_invite;
use handlers::json_api::items::{
    create_item, delete_item, get_item, list_assigned_items, list_items, list_items_due,
    update_item,
};
use handlers::json_api::team_items::{
    create_team_item, delete_team_item, get_team_item, list_team_items, update_team_item,
};
use handlers::json_api::teams::{
    accept_team_invite, create_team, get_team, invite_team_member, leave_team, list_team_members,
    list_teams,
};
use handlers::json_api::templates::{create_template, list_templates};
use handlers::json_api::users::{get_user, list_users, update_user};
use todo_server_sdk::{PeoplesRepublicOfLists, PeoplesRepublicOfListsConfig};
use tower::ServiceBuilder;
use tower_cookies::CookieManagerLayer;
use tower_http::services::{ServeDir, ServeFile};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let db_url = std::env::var("TODO_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://todo.db?mode=rwc".to_string());
    let pool = create_pool(&db_url).await.expect("failed to open database");
    let user_repo = Arc::new(SqliteUserRepo(pool.clone())) as Arc<dyn UserRepo>;
    let item_repo = Arc::new(SqliteItemRepo(pool.clone())) as Arc<dyn ItemRepo>;
    let team_repo = Arc::new(SqliteTeamRepo(pool)) as Arc<dyn TeamRepo>;

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
        .list_teams(list_teams)
        .list_team_members(list_team_members)
        .invite_team_member(invite_team_member)
        .accept_team_invite(accept_team_invite)
        .leave_team(leave_team)
        .send_app_invite(send_app_invite)
        .create_team_item(create_team_item)
        .get_team_item(get_team_item)
        .update_team_item(update_team_item)
        .delete_team_item(delete_team_item)
        .list_team_items(list_team_items)
        .build_unchecked();

    let api = ServiceBuilder::new()
        .layer(Extension(user_repo.clone()))
        .layer(Extension(item_repo.clone()))
        .layer(Extension(team_repo.clone()))
        .map_response(|res: http::Response<_>| res.map(boxed))
        .service(smithy);

    let frontend =
        ServeDir::new("frontend/dist").fallback(ServeFile::new("frontend/dist/index.html"));
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

            let web_router = Router::new()
                .route("/test", get(handlers::web_ui::hello_world::hello_world))
                .route("/items/:item_id", get(handlers::web_ui::item_page::item_page))
                .route(
                    "/items/:item_id/children",
                    get(handlers::web_ui::item_children_page::item_children_page),
                )
                .layer(Extension(user_repo.clone()))
                .layer(Extension(item_repo.clone()))
                .layer(Extension(team_repo.clone()))
                .layer(middleware::from_fn(auth::caddy_header_middleware));

            Router::new()
                .nest("/api", api_router)
                .nest("/auth", auth_router)
                .nest("/web", web_router)
                .nest_service("/web/static", web_static)
                .layer(Extension(user_repo))
                .layer(Extension(Arc::new(jwt_secret)))
                .layer(CookieManagerLayer::new())
                .fallback_service(frontend)
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
                user_repo,
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

            let web_router = Router::new()
                .route("/test", get(handlers::web_ui::hello_world::hello_world))
                .route("/items/:item_id", get(handlers::web_ui::item_page::item_page))
                .route(
                    "/items/:item_id/children",
                    get(handlers::web_ui::item_children_page::item_children_page),
                )
                .layer(Extension(item_repo.clone()))
                .layer(middleware::from_fn(auth::jwt_auth_middleware));

            Router::new()
                .nest("/auth", auth_router)
                .nest("/api", api_router)
                .nest("/web", web_router)
                .nest_service("/web/static", web_static)
                .layer(Extension(app_state))
                .layer(CookieManagerLayer::new())
                .fallback_service(frontend)
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
