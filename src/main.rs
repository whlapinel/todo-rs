use std::{net::SocketAddr, sync::Arc};
mod auth;
mod domain;
mod handlers;
mod storage;

use crate::storage::{
    ItemRepo, TeamRepo, UserRepo,
    sqlite::{SqliteItemRepo, SqliteTeamRepo, SqliteUserRepo, create_pool},
};
use axum::{Extension, Router, body::boxed, middleware, routing::get};
use handlers::items::{
    create_item, delete_item, get_item, list_assigned_items, list_items, list_items_due,
    update_item,
};
use handlers::teams::{
    accept_team_invite, create_team, get_team, invite_team_member, leave_team,
    list_team_members, list_teams,
};
use handlers::templates::{create_template, list_templates};
use handlers::users::{get_user, list_users, update_user};
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
        .build_unchecked();

    let api = ServiceBuilder::new()
        .layer(Extension(user_repo.clone()))
        .layer(Extension(item_repo))
        .layer(Extension(team_repo))
        .map_response(|res: http::Response<_>| res.map(boxed))
        .service(smithy);

    let frontend =
        ServeDir::new("frontend/dist").fallback(ServeFile::new("frontend/dist/index.html"));

    let auth_mode = std::env::var("TODO_AUTH_MODE").unwrap_or_else(|_| "internal".to_string());
    tracing::info!(auth_mode, "auth mode");

    let app = match auth_mode.as_str() {
        "caddy" => {
            let api_router = Router::new()
                .route_service("/users", api.clone())
                .route_service("/users/*path", api.clone())
                .layer(middleware::from_fn(auth::caddy_header_middleware));
            Router::new()
                .nest("/api", api_router)
                .layer(Extension(user_repo))
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
            let base_url = std::env::var("TODO_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3000".to_string());

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
                .layer(middleware::from_fn(auth::jwt_auth_middleware));

            Router::new()
                .nest("/auth", auth_router)
                .nest("/api", api_router)
                .layer(Extension(app_state))
                .layer(CookieManagerLayer::new())
                .fallback_service(frontend)
        }
    };

    let bind: SocketAddr = std::env::var("TODO_BIND")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
        .parse()
        .expect("invalid BIND address");
    tracing::info!("listening on {}", bind);
    axum::Server::bind(&bind)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
