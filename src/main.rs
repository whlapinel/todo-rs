use std::{net::SocketAddr, sync::Arc};
mod domain;
mod storage;

use axum::{body::boxed, Extension, Router};
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime, Listeria, ListeriaConfig};

use crate::domain::{item::Item, list::List, user::User};
use crate::storage::{
    sqlite::{create_pool, SqliteItemRepo, SqliteListRepo, SqliteUserRepo},
    ItemRepo, ListRepo, RepoError, UserRepo,
};

fn internal(msg: impl ToString) -> error::ListeriaError {
    error::ListeriaError { message: msg.to_string() }
}

fn not_found() -> error::ListeriaError {
    error::ListeriaError { message: "not found".to_string() }
}

async fn create_user(
    input: input::CreateUserInput,
    server::Extension(repo): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::CreateUserOutput, error::CreateUserError> {
    let user = User::new(&input.first_name, &input.last_name);
    let user_id = repo.create(&user).await.map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateUserOutput { user_id })
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
    let user = User { id: input.user_id, first_name: input.first_name, last_name: input.last_name };
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

async fn create_list(
    input: input::CreateListInput,
    server::Extension(repo): server::Extension<Arc<dyn ListRepo>>,
) -> Result<output::CreateListOutput, error::CreateListError> {
    let list = List::new(&input.user_id, &input.name);
    let list_id = repo.create(&list).await.map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateListOutput { list_id })
}

async fn get_list(
    input: input::GetListInput,
    server::Extension(repo): server::Extension<Arc<dyn ListRepo>>,
) -> Result<output::GetListOutput, error::GetListError> {
    let list = repo.get(&input.user_id, &input.list_id).await.map_err(|e| match e {
        RepoError::NotFound => error::GetListError::from(not_found()),
        _ => error::GetListError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::GetListOutput { name: Some(list.name) })
}

async fn list_lists(
    input: input::ListListsInput,
    server::Extension(repo): server::Extension<Arc<dyn ListRepo>>,
) -> Result<output::ListListsOutput, error::ListListsError> {
    let lists = repo.list(&input.user_id).await.map_err(|e| internal(format!("{e:?}")))?;
    let lists = lists
        .into_iter()
        .map(|l| todo_server_sdk::model::ListSummary {
            list_id: l.id,
            user_id: l.user_id,
            name: l.name,
        })
        .collect();
    Ok(output::ListListsOutput { lists })
}

async fn update_list(
    input: input::UpdateListInput,
    server::Extension(repo): server::Extension<Arc<dyn ListRepo>>,
) -> Result<output::UpdateListOutput, error::UpdateListError> {
    let list = List { id: input.list_id, user_id: input.user_id, name: input.name };
    repo.update(&list).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateListError::from(not_found()),
        _ => error::UpdateListError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateListOutput {})
}

async fn create_item(
    input: input::CreateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::CreateItemOutput, error::CreateItemError> {
    let mut item = Item::new(&input.user_id, &input.list_id, &input.name);
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    let item_id = repo.create(&item).await.map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateItemOutput { item_id })
}

async fn update_item(
    input: input::UpdateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::UpdateItemOutput, error::UpdateItemError> {
    let mut item = Item::new(&input.user_id, &input.list_id, &input.name);
    item.id = input.item_id;
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    repo.update(&item).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateItemError::from(not_found()),
        _ => error::UpdateItemError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateItemOutput {})
}

async fn get_item(
    input: input::GetItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::GetItemOutput, error::GetItemError> {
    let item = repo
        .get(&input.user_id, &input.list_id, &input.item_id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => error::GetItemError::from(not_found()),
            _ => error::GetItemError::from(internal(format!("{e:?}"))),
        })?;
    let due_date = item
        .deadline
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()))
        .unwrap_or(SmithyDateTime::from_secs(0));
    Ok(output::GetItemOutput { name: item.name, due_date })
}

async fn list_items(
    input: input::ListItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListItemsOutput, error::ListItemsError> {
    let items = repo
        .list(&input.user_id, &input.list_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::ItemSummary {
            item_id: Some(i.id),
            name: Some(i.name),
            due_date: i.deadline.map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
        })
        .collect();
    Ok(output::ListItemsOutput { items })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let pool = create_pool("sqlite://todo.db?mode=rwc")
        .await
        .expect("failed to open database");
    let user_repo = Arc::new(SqliteUserRepo(pool.clone())) as Arc<dyn UserRepo>;
    let list_repo = Arc::new(SqliteListRepo(pool.clone())) as Arc<dyn ListRepo>;
    let item_repo = Arc::new(SqliteItemRepo(pool)) as Arc<dyn ItemRepo>;

    let config = ListeriaConfig::builder().build();
    let smithy = Listeria::builder(config)
        .create_user(create_user)
        .get_user(get_user)
        .update_user(update_user)
        .list_users(list_users)
        .create_list(create_list)
        .get_list(get_list)
        .update_list(update_list)
        .list_lists(list_lists)
        .create_item(create_item)
        .get_item(get_item)
        .update_item(update_item)
        .list_items(list_items)
        .build_unchecked();

    let api = ServiceBuilder::new()
        .layer(Extension(user_repo))
        .layer(Extension(list_repo))
        .layer(Extension(item_repo))
        .map_response(|res: http::Response<_>| res.map(boxed))
        .service(smithy);

    let api_router = Router::new()
        .route_service("/users", api.clone())
        .route_service("/users/*path", api.clone());

    let app = Router::new()
        .nest("/api", api_router)
        .fallback_service(
            ServeDir::new("frontend/dist")
                .fallback(ServeFile::new("frontend/dist/index.html")),
        );

    let bind: SocketAddr = "127.0.0.1:3000".parse().unwrap();
    tracing::info!("listening on {}", bind);
    axum::Server::bind(&bind)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
