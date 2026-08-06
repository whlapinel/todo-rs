pub mod invites;
pub mod items;
pub mod team_items;
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
