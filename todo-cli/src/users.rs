use crate::helpers::{require_user, unwrap_or_exit};
use clap::Subcommand;
use todo_client::Client;

#[derive(Subcommand)]
pub enum UsersCommand {
    /// List all users
    List,
    /// Show a user (defaults to configured user)
    Get { user_id: Option<String> },
    /// Send an app invite email
    Invite { email: String },
}

pub async fn cmd_users(client: &Client, cmd: UsersCommand, default_user: Option<String>) {
    match cmd {
        UsersCommand::List => {
            let out = unwrap_or_exit(client.list_users().send().await, "list users");
            println!("{:<36}  {}", "ID", "NAME");
            for u in out.users() {
                println!("{:<36}  {} {}", u.user_id(), u.first_name(), u.last_name());
            }
        }
        UsersCommand::Get { user_id } => {
            let id = require_user(user_id.or(default_user));
            let out = unwrap_or_exit(client.get_user().user_id(id).send().await, "get user");
            println!("id:   {}", out.user_id());
            println!("name: {} {}", out.first_name(), out.last_name());
        }
        UsersCommand::Invite { email } => {
            let uid = require_user(default_user);
            unwrap_or_exit(
                client
                    .send_app_invite()
                    .user_id(uid)
                    .email(&email)
                    .send()
                    .await,
                "send app invite",
            );
            println!("invite sent to {email}");
        }
    }
}
