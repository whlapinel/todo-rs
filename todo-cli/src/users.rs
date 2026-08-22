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
    /// Set your IANA timezone (e.g. "America/New_York") — used by Google Calendar
    /// import to resolve all-day event dates correctly.
    SetTimezone { timezone: String },
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
        UsersCommand::SetTimezone { timezone } => {
            let uid = require_user(default_user);
            // UpdateUser requires firstName/lastName be round-tripped — only timezone
            // is preserved when omitted, see root CLAUDE.md's User.smithy notes.
            let current = unwrap_or_exit(
                client.get_user().user_id(&uid).send().await,
                "get user",
            );
            unwrap_or_exit(
                client
                    .update_user()
                    .user_id(&uid)
                    .first_name(current.first_name())
                    .last_name(current.last_name())
                    .timezone(&timezone)
                    .send()
                    .await,
                "set timezone",
            );
            println!("timezone set to {timezone}");
        }
    }
}
