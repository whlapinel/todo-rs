use std::{net::SocketAddr, sync::Arc};
mod auth;
mod domain;
mod recurrence;
mod storage;

use axum::{body::boxed, middleware, routing::get, Extension, Router};
use tower::ServiceBuilder;
use tower_cookies::CookieManagerLayer;
use tower_http::services::{ServeDir, ServeFile};
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime, PeoplesRepublicOfLists, PeoplesRepublicOfListsConfig};
use crate::domain::{item::Item, user::User};
use crate::storage::{
    sqlite::{create_pool, SqliteItemRepo, SqliteUserRepo},
    ItemRepo, RepoError, UserRepo,
};

fn internal(msg: impl ToString) -> error::PeoplesRepublicOfListsError {
    error::PeoplesRepublicOfListsError { message: msg.to_string() }
}

fn not_found() -> error::PeoplesRepublicOfListsError {
    error::PeoplesRepublicOfListsError { message: "not found".to_string() }
}

async fn get_user(
    input: input::GetUserInput,
    server::Extension(repo): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::GetUserOutput, error::GetUserError> {
    let user = repo.get(&input.user_id).await.map_err(|e| match e {
        RepoError::NotFound => error::GetUserError::from(not_found()),
        _ => error::GetUserError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::GetUserOutput {
        user_id: user.id,
        first_name: user.first_name,
        last_name: user.last_name,
    })
}

async fn update_user(
    input: input::UpdateUserInput,
    server::Extension(repo): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::UpdateUserOutput, error::UpdateUserError> {
    let user = User { id: input.user_id, first_name: input.first_name, last_name: input.last_name, email: None, google_id: None };
    repo.update(&user).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateUserError::from(not_found()),
        _ => error::UpdateUserError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateUserOutput {})
}

async fn list_users(
    _input: input::ListUsersInput,
    server::Extension(repo): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::ListUsersOutput, error::ListUsersError> {
    let users = repo.list().await.map_err(|e| internal(format!("{e:?}")))?;
    let users = users
        .into_iter()
        .map(|u| todo_server_sdk::model::UserSummary {
            user_id: u.id,
            first_name: u.first_name,
            last_name: u.last_name,
        })
        .collect();
    Ok(output::ListUsersOutput { users })
}

async fn create_item(
    input: input::CreateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::CreateItemOutput, error::CreateItemError> {
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(|e| internal(e))?;
    }
    let mut item = Item::new(&input.user_id, &input.name);
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    item.complete = input.complete.unwrap_or(false);
    item.recurrence = input.recurrence;
    item.recurrence_basis = input.recurrence_basis;
    item.has_due_time = input.has_due_time.unwrap_or(false);
    item.has_tasks = input.has_tasks.unwrap_or(true);
    item.parent_item_id = input.parent_item_id;

    if item.deadline.is_none() {
        if let Some(ref pattern) = item.recurrence {
            if let Ok(rule) = recurrence::parse(pattern) {
                let tz_offset = input.timezone_offset_minutes.unwrap_or(0);
                let mut deadline = recurrence::next_date(&rule, chrono::Utc::now(), tz_offset);
                if rule.time_override.is_none() {
                    deadline = recurrence::apply_end_of_day(deadline, tz_offset);
                } else {
                    item.has_due_time = true;
                }
                item.deadline = Some(deadline);
            }
        }
    }
    let item_id = repo.create(&item).await.map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateItemOutput { item_id })
}

async fn update_item(
    input: input::UpdateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::UpdateItemOutput, error::UpdateItemError> {
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(|e| internal(e))?;
    }

    let mut item = Item::new(&input.user_id, &input.name);
    item.id = input.item_id.clone();
    item.complete = input.complete;
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    item.recurrence = input.recurrence.clone();
    item.recurrence_basis = input.recurrence_basis.clone();
    item.has_due_time = input.has_due_time.unwrap_or(false);
    item.has_tasks = input.has_tasks.unwrap_or(true);
    item.parent_item_id = input.parent_item_id.clone();

    if item.complete {
        if let Some(ref pattern) = item.recurrence {
            if let Ok(rule) = recurrence::parse(pattern) {
                let reference = if item.recurrence_basis.as_deref() == Some("COMPLETION_DATE") {
                    chrono::Utc::now()
                } else {
                    item.deadline.unwrap_or_else(chrono::Utc::now)
                };
                let tz_offset = input.timezone_offset_minutes.unwrap_or(0);
                let next_deadline = recurrence::next_date(&rule, reference, tz_offset);
                let mut next_item = Item::new(&item.user_id, &item.name);
                next_item.deadline = Some(next_deadline);
                next_item.recurrence = item.recurrence.clone();
                next_item.recurrence_basis = item.recurrence_basis.clone();
                next_item.has_tasks = item.has_tasks;
                next_item.parent_item_id = item.parent_item_id.clone();
                next_item.has_due_time = if rule.time_override.is_some() { true } else { item.has_due_time };
                repo.create(&next_item).await.map_err(|e| internal(format!("{e:?}")))?;
                repo.delete(&item.id).await.map_err(|e| internal(format!("{e:?}")))?;
                return Ok(output::UpdateItemOutput {});
            }
        }
    }

    repo.update(&item).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateItemError::from(not_found()),
        _ => error::UpdateItemError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateItemOutput {})
}

async fn delete_item(
    input: input::DeleteItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::DeleteItemOutput, error::DeleteItemError> {
    // BFS cascade-delete all descendants before deleting this item
    let mut queue = vec![input.item_id.clone()];
    while let Some(parent_id) = queue.first().cloned() {
        queue.remove(0);
        let children = repo.list_children(&parent_id).await.map_err(|e| internal(format!("{e:?}")))?;
        for child in children {
            queue.push(child.id.clone());
            repo.delete(&child.id).await.map_err(|e| internal(format!("{e:?}")))?;
        }
    }
    repo.delete(&input.item_id).await.map_err(|e| match e {
        RepoError::NotFound => error::DeleteItemError::from(not_found()),
        _ => error::DeleteItemError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::DeleteItemOutput {})
}

async fn get_item(
    input: input::GetItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::GetItemOutput, error::GetItemError> {
    let item = repo.get(&input.user_id, &input.item_id).await.map_err(|e| match e {
        RepoError::NotFound => error::GetItemError::from(not_found()),
        _ => error::GetItemError::from(internal(format!("{e:?}"))),
    })?;
    let due_date = item
        .deadline
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()))
        .unwrap_or(SmithyDateTime::from_secs(0));
    Ok(output::GetItemOutput {
        name: item.name,
        due_date,
        complete: item.complete,
        has_due_time: Some(item.has_due_time),
        has_tasks: Some(item.has_tasks),
        parent_item_id: item.parent_item_id,
        has_children: Some(item.has_children),
    })
}

async fn list_items(
    input: input::ListItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListItemsOutput, error::ListItemsError> {
    let items = if let Some(ref parent_id) = input.parent_item_id {
        repo.list_children(parent_id).await.map_err(|e| internal(format!("{e:?}")))?
    } else {
        repo.list(&input.user_id).await.map_err(|e| internal(format!("{e:?}")))?
    };
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::ItemSummary {
            item_id: Some(i.id),
            name: Some(i.name),
            due_date: i.deadline.map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(i.complete),
            recurrence: i.recurrence,
            recurrence_basis: i.recurrence_basis,
            has_due_time: Some(i.has_due_time),
            has_tasks: Some(i.has_tasks),
            parent_item_id: i.parent_item_id,
            has_children: Some(i.has_children),
        })
        .collect();
    Ok(output::ListItemsOutput { items })
}

async fn list_items_due(
    input: input::ListItemsDueInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListItemsDueOutput, error::ListItemsDueError> {
    let after = input.deadline_after.map(|t| t.secs());
    let before = input.deadline_before.map(|t| t.secs());
    let due_items = repo
        .list_due(&input.user_id, after, before)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let items = due_items
        .into_iter()
        .map(|di| todo_server_sdk::model::DueItemSummary {
            item_id: di.item.id,
            name: di.item.name,
            parent_name: Some(di.parent_name),
            due_date: di.item.deadline.map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(di.item.complete),
            recurrence: di.item.recurrence,
            recurrence_basis: di.item.recurrence_basis,
            has_due_time: Some(di.item.has_due_time),
        })
        .collect();
    Ok(output::ListItemsDueOutput { items })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let db_url = std::env::var("TODO_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://todo.db?mode=rwc".to_string());
    let pool = create_pool(&db_url).await.expect("failed to open database");
    let user_repo = Arc::new(SqliteUserRepo(pool.clone())) as Arc<dyn UserRepo>;
    let item_repo = Arc::new(SqliteItemRepo(pool)) as Arc<dyn ItemRepo>;

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
        .build_unchecked();

    let api = ServiceBuilder::new()
        .layer(Extension(user_repo.clone()))
        .layer(Extension(item_repo))
        .map_response(|res: http::Response<_>| res.map(boxed))
        .service(smithy);

    let frontend = ServeDir::new("frontend/dist")
        .fallback(ServeFile::new("frontend/dist/index.html"));

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
