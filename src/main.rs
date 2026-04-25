use std::{net::SocketAddr, sync::Arc};
mod domain;
mod recurrence;
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
    let mut list = List::new(&input.user_id, &input.name);
    list.has_tasks = input.has_tasks.unwrap_or(true);
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
    Ok(output::GetListOutput { name: Some(list.name), has_tasks: Some(list.has_tasks) })
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
            has_tasks: Some(l.has_tasks),
        })
        .collect();
    Ok(output::ListListsOutput { lists })
}

async fn update_list(
    input: input::UpdateListInput,
    server::Extension(repo): server::Extension<Arc<dyn ListRepo>>,
) -> Result<output::UpdateListOutput, error::UpdateListError> {
    let list = List {
        id: input.list_id,
        user_id: input.user_id,
        name: input.name,
        has_tasks: input.has_tasks.unwrap_or(true),
    };
    repo.update(&list).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateListError::from(not_found()),
        _ => error::UpdateListError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateListOutput {})
}

async fn delete_list(
    input: input::DeleteListInput,
    server::Extension(list_repo): server::Extension<Arc<dyn ListRepo>>,
    server::Extension(item_repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::DeleteListOutput, error::DeleteListError> {
    item_repo.delete_by_list(&input.list_id).await.map_err(|e| internal(format!("{e:?}")))?;
    list_repo.delete(&input.list_id).await.map_err(|e| match e {
        RepoError::NotFound => error::DeleteListError::from(not_found()),
        _ => error::DeleteListError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::DeleteListOutput {})
}

async fn create_item(
    input: input::CreateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::CreateItemOutput, error::CreateItemError> {
    // Validate recurrence phrase if provided
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(|e| internal(e))?;
    }
    let mut item = Item::new(&input.user_id, &input.list_id, &input.name);
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    item.complete = input.complete.unwrap_or(false);
    item.recurrence = input.recurrence;
    item.recurrence_basis = input.recurrence_basis;
    item.has_due_time = input.has_due_time.unwrap_or(false);
    let item_id = repo.create(&item).await.map_err(|e| internal(format!("{e:?}")))?;
    Ok(output::CreateItemOutput { item_id })
}

async fn update_item(
    input: input::UpdateItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
) -> Result<output::UpdateItemOutput, error::UpdateItemError> {
    // Validate recurrence phrase if provided
    if let Some(ref r) = input.recurrence {
        recurrence::parse(r).map_err(|e| internal(e))?;
    }

    let mut item = Item::new(&input.user_id, &input.list_id, &input.name);
    item.id = input.item_id.clone();
    item.complete = input.complete;
    if let Some(dt) = input.due_date {
        item.deadline = chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
            .map(|d| d.with_timezone(&chrono::Utc));
    }
    item.recurrence = input.recurrence.clone();
    item.recurrence_basis = input.recurrence_basis.clone();
    item.has_due_time = input.has_due_time.unwrap_or(false);

    // If completing a recurring item, spawn the next occurrence and delete this one.
    if item.complete {
        if let Some(ref pattern) = item.recurrence {
            if let Ok(rule) = recurrence::parse(pattern) {
                let reference = if item.recurrence_basis.as_deref() == Some("COMPLETION_DATE") {
                    chrono::Utc::now()
                } else {
                    item.deadline.unwrap_or_else(chrono::Utc::now)
                };
                let next_deadline = recurrence::next_date(&rule, reference);
                let mut next_item = Item::new(&item.user_id, &item.list_id, &item.name);
                next_item.deadline = Some(next_deadline);
                next_item.recurrence = item.recurrence.clone();
                next_item.recurrence_basis = item.recurrence_basis.clone();
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
    Ok(output::GetItemOutput { name: item.name, due_date, complete: item.complete, has_due_time: Some(item.has_due_time) })
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
            list_id: di.item.list_id,
            list_name: di.list_name,
            name: di.item.name,
            due_date: di.item.deadline.map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(di.item.complete),
            recurrence: di.item.recurrence,
            recurrence_basis: di.item.recurrence_basis,
            has_due_time: Some(di.item.has_due_time),
        })
        .collect();
    Ok(output::ListItemsDueOutput { items })
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
            complete: Some(i.complete),
            recurrence: i.recurrence,
            recurrence_basis: i.recurrence_basis,
            has_due_time: Some(i.has_due_time),
        })
        .collect();
    Ok(output::ListItemsOutput { items })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://todo.db?mode=rwc".to_string());
    let pool = create_pool(&db_url).await.expect("failed to open database");
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
        .delete_list(delete_list)
        .list_lists(list_lists)
        .create_item(create_item)
        .get_item(get_item)
        .update_item(update_item)
        .delete_item(delete_item)
        .list_items(list_items)
        .list_items_due(list_items_due)
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

    let bind: SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
        .parse()
        .expect("invalid BIND address");
    tracing::info!("listening on {}", bind);
    axum::Server::bind(&bind)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
