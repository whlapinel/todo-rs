use crate::helpers::{require_user, unwrap_or_exit};
use clap::Subcommand;
use todo_client::Client;

#[derive(Subcommand)]
pub enum TeamsCommand {
    /// List your teams, including pending invites
    List,
    /// Create a new team (you become its first member)
    Create { name: String },
    /// List a team's members
    Members { team_id: String },
    /// Invite an existing user to a team
    Invite {
        team_id: String,
        invitee_user_id: String,
    },
    /// Accept a pending team invite
    Accept { team_id: String },
    /// Leave a team (or decline a pending invite)
    Leave { team_id: String },
}

pub async fn cmd_teams(client: &Client, cmd: TeamsCommand, user_id: Option<String>) {
    match cmd {
        TeamsCommand::List => {
            let uid = require_user(user_id);
            let out = unwrap_or_exit(client.list_teams().user_id(uid).send().await, "list teams");
            if out.teams().is_empty() {
                println!("(no teams)");
                return;
            }
            println!("{:<36}  {:<8}  {}", "ID", "STATUS", "NAME");
            for t in out.teams() {
                let suffix = t
                    .invited_by_name()
                    .map(|n| format!("  (invited by {n})"))
                    .unwrap_or_default();
                println!("{:<36}  {:<8}  {}{}", t.team_id(), t.status(), t.name(), suffix);
            }
        }
        TeamsCommand::Create { name } => {
            let uid = require_user(user_id);
            let out = unwrap_or_exit(
                client.create_team().user_id(uid).name(name).send().await,
                "create team",
            );
            println!("created team {}", out.team_id());
        }
        TeamsCommand::Members { team_id } => {
            let uid = require_user(user_id);
            let out = unwrap_or_exit(
                client
                    .list_team_members()
                    .user_id(uid)
                    .team_id(team_id)
                    .send()
                    .await,
                "list team members",
            );
            println!("{:<36}  {:<8}  {}", "USER ID", "STATUS", "NAME");
            for m in out.members() {
                println!(
                    "{:<36}  {:<8}  {} {}",
                    m.user_id(), m.status(), m.first_name(), m.last_name()
                );
            }
        }
        TeamsCommand::Invite {
            team_id,
            invitee_user_id,
        } => {
            let uid = require_user(user_id);
            unwrap_or_exit(
                client
                    .invite_team_member()
                    .user_id(uid)
                    .team_id(&team_id)
                    .invitee_user_id(&invitee_user_id)
                    .send()
                    .await,
                "invite team member",
            );
            println!("invited {invitee_user_id} to team {team_id}");
        }
        TeamsCommand::Accept { team_id } => {
            let uid = require_user(user_id);
            unwrap_or_exit(
                client
                    .accept_team_invite()
                    .user_id(uid)
                    .team_id(&team_id)
                    .send()
                    .await,
                "accept team invite",
            );
            println!("joined team {team_id}");
        }
        TeamsCommand::Leave { team_id } => {
            let uid = require_user(user_id);
            unwrap_or_exit(
                client
                    .leave_team()
                    .user_id(uid)
                    .team_id(&team_id)
                    .send()
                    .await,
                "leave team",
            );
            println!("left team {team_id}");
        }
    }
}
