pub mod invites;
pub mod items;
pub mod team_items;
pub mod team_templates;
pub mod teams;
pub mod templates;
pub mod users;
use todo_server_sdk::error;

fn internal(msg: impl ToString) -> error::PeoplesRepublicOfListsError {
    error::PeoplesRepublicOfListsError {
        message: msg.to_string(),
    }
}

fn not_found() -> error::PeoplesRepublicOfListsError {
    error::PeoplesRepublicOfListsError {
        message: "not found".to_string(),
    }
}

fn to_domain_item_type(
    t: Option<todo_server_sdk::model::ItemType>,
) -> Option<crate::domain::item::ItemType> {
    t.map(|t| match t {
        todo_server_sdk::model::ItemType::Task => crate::domain::item::ItemType::Task,
        todo_server_sdk::model::ItemType::Event => crate::domain::item::ItemType::Event,
        todo_server_sdk::model::ItemType::Template => crate::domain::item::ItemType::Template,
        todo_server_sdk::model::ItemType::Simple => crate::domain::item::ItemType::Simple,
    })
}

fn to_sdk_item_type(t: crate::domain::item::ItemType) -> todo_server_sdk::model::ItemType {
    match t {
        crate::domain::item::ItemType::Task => todo_server_sdk::model::ItemType::Task,
        crate::domain::item::ItemType::Event => todo_server_sdk::model::ItemType::Event,
        crate::domain::item::ItemType::Template => todo_server_sdk::model::ItemType::Template,
        crate::domain::item::ItemType::Simple => todo_server_sdk::model::ItemType::Simple,
    }
}
