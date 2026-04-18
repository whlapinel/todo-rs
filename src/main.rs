use std::{net::SocketAddr, sync::Arc};
mod domain;

mod storage;
use async_trait::async_trait;

use todo_server_sdk::{Listeria, ListeriaConfig, error, input, output, server::Extension};

use crate::storage::{ItemRepo, ListRepo};

async fn get_list(
    input: input::GetListInput,
    Extension(repo): Extension<Arc<dyn ListRepo>>,
) -> Result<output::GetListOutput, error::GetListError> {
    todo!("get_list: {:?}", input)
}

async fn list_lists(
    input: input::ListListsInput,
    Extension(repo): Extension<Arc<dyn ListRepo>>,
) -> Result<output::ListListsOutput, error::ListListsError> {
    todo!("list_lists: {:?}", input)
}

async fn get_item(
    input: input::GetItemInput,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<output::GetItemOutput, error::GetItemError> {
    todo!("get_item: {:?}", input)
}

async fn list_items(
    input: input::ListItemsInput,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<output::ListItemsOutput, error::ListItemsError> {
    todo!("list_items: {:?}", input)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = ListeriaConfig::builder().build();
    let app = Listeria::builder(config)
        .get_list(get_list)
        .list_lists(list_lists)
        .get_item(get_item)
        .list_items(list_items)
        .build()
        .unwrap();

    let bind: SocketAddr = "127.0.0.1:3000".parse().unwrap();
    tracing::info!("listening on {}", bind);
    hyper::Server::bind(&bind)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
