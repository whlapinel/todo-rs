use crate::helpers::{fmt_date, require_user, unwrap_or_exit};
use clap::Subcommand;
use todo_client::Client;

#[derive(Subcommand)]
pub enum TeamsCommand {
    /// List your teams, including pending invites
    List,
    /// Create a new team (you become its first member)
    Create { name: String },
    /// List a team's members, their role, and their points balance
    Members { team_id: String },
    /// Rename a team (you must already be an admin)
    Rename { team_id: String, name: String },
    /// Invite an existing user to a team
    Invite {
        team_id: String,
        invitee_user_id: String,
    },
    /// Accept a pending team invite
    Accept { team_id: String },
    /// Leave a team (or decline a pending invite)
    Leave { team_id: String },
    /// Promote or demote a team member (you must already be an admin)
    SetRole {
        team_id: String,
        target_user_id: String,
        /// "admin" or "member"
        role: String,
    },
    /// List a team's points/completion activity log (most recent first)
    Activity { team_id: String },
    /// Reverse a specific activity log entry's points (only the entry's own user may do this)
    UndoActivity { team_id: String, entry_id: String },
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
            println!(
                "{:<36}  {:<8}  {:<6}  {:>6}  {}",
                "USER ID", "STATUS", "ROLE", "POINTS", "NAME"
            );
            for m in out.members() {
                println!(
                    "{:<36}  {:<8}  {:<6}  {:>6}  {} {}",
                    m.user_id(),
                    m.status(),
                    m.role(),
                    m.points(),
                    m.first_name(),
                    m.last_name()
                );
            }
        }
        TeamsCommand::Rename { team_id, name } => {
            let uid = require_user(user_id);
            unwrap_or_exit(
                client
                    .update_team()
                    .user_id(uid)
                    .team_id(&team_id)
                    .name(&name)
                    .send()
                    .await,
                "rename team",
            );
            println!("renamed team {team_id} to {name}");
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
        TeamsCommand::SetRole {
            team_id,
            target_user_id,
            role,
        } => {
            let uid = require_user(user_id);
            unwrap_or_exit(
                client
                    .set_team_member_role()
                    .user_id(uid)
                    .team_id(&team_id)
                    .target_user_id(&target_user_id)
                    .role(&role)
                    .send()
                    .await,
                "set team member role",
            );
            println!("set {target_user_id}'s role on team {team_id} to {role}");
        }
        TeamsCommand::Activity { team_id } => {
            let out = unwrap_or_exit(
                client
                    .list_team_activity_log()
                    .team_id(&team_id)
                    .send()
                    .await,
                "list team activity log",
            );
            if out.entries().is_empty() {
                println!("(no activity)");
                return;
            }
            println!(
                "{:<36}  {:<10}  {:<36}  {:>6}  {:<9}  {}",
                "ENTRY ID", "WHEN", "USER ID", "POINTS", "REVERSED", "ITEM"
            );
            for e in out.entries() {
                println!(
                    "{:<36}  {:<10}  {:<36}  {:>6}  {:<9}  {}",
                    e.entry_id(),
                    fmt_date(e.created_at()),
                    e.user_id(),
                    e.points_delta(),
                    e.reversed(),
                    e.item_name(),
                );
            }
        }
        TeamsCommand::UndoActivity { team_id, entry_id } => {
            unwrap_or_exit(
                client
                    .undo_activity_log_entry()
                    .team_id(&team_id)
                    .entry_id(&entry_id)
                    .send()
                    .await,
                "undo activity log entry",
            );
            println!("reversed activity log entry {entry_id}");
        }
    }
}
