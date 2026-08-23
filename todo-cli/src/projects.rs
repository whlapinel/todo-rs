use crate::helpers::{fmt_date_opt, require_user, unwrap_or_exit};
use clap::Subcommand;
use todo_client::Client;

#[derive(Subcommand)]
pub enum ProjectsCommand {
    /// List projects you're a member of
    List,
    /// Create a new personal project (no attached team)
    Create { name: String },
    /// Delete a project and everything in it — every task, event, series, and its
    /// activity history (you must be a project admin; your own personal project can't
    /// be deleted)
    Delete { project_id: String },
    /// List a project's members, their role, and their points balance
    Members { project_id: String },
    /// Attach a team to a project, granting its active members access (you must be a project admin)
    AttachTeam { project_id: String, team_id: String },
    /// Detach a project's team, revoking its members' access (you must be a project admin)
    DetachTeam { project_id: String },
    /// Promote or demote a project member (you must already be a project admin)
    SetRole {
        project_id: String,
        target_user_id: String,
        /// "admin" or "member"
        role: String,
    },
    /// Subscribe a project to a Google Calendar (private iCal URL) — imports its events read-only (you must be a project admin)
    CalendarAdd { project_id: String, url: String },
    /// List a project's Google Calendar subscriptions
    CalendarList { project_id: String },
    /// Unsubscribe a Google Calendar — also deletes every event it imported (you must be a project admin)
    CalendarRemove {
        project_id: String,
        subscription_id: String,
    },
}

pub async fn cmd_projects(client: &Client, cmd: ProjectsCommand, user_id: Option<String>) {
    match cmd {
        ProjectsCommand::List => {
            let uid = require_user(user_id);
            let out = unwrap_or_exit(
                client.list_projects().user_id(uid).send().await,
                "list projects",
            );
            if out.projects().is_empty() {
                println!("(no projects)");
                return;
            }
            println!("{:<36}  {:<36}  {}", "ID", "TEAM ID", "NAME");
            for p in out.projects() {
                println!(
                    "{:<36}  {:<36}  {}",
                    p.project_id(),
                    p.team_id().unwrap_or("-"),
                    p.name()
                );
            }
        }
        ProjectsCommand::Create { name } => {
            let uid = require_user(user_id);
            let out = unwrap_or_exit(
                client.create_project().user_id(uid).name(name).send().await,
                "create project",
            );
            println!("created project {}", out.project_id());
        }
        ProjectsCommand::Delete { project_id } => {
            let uid = require_user(user_id);
            unwrap_or_exit(
                client
                    .delete_project()
                    .user_id(uid)
                    .project_id(&project_id)
                    .send()
                    .await,
                "delete project",
            );
            println!("deleted project {project_id}");
        }
        ProjectsCommand::Members { project_id } => {
            let out = unwrap_or_exit(
                client
                    .list_project_members()
                    .project_id(project_id)
                    .send()
                    .await,
                "list project members",
            );
            if out.members().is_empty() {
                println!("(no members)");
                return;
            }
            println!(
                "{:<36}  {:<6}  {:>6}  {}",
                "USER ID", "ROLE", "POINTS", "NAME"
            );
            for m in out.members() {
                println!(
                    "{:<36}  {:<6}  {:>6}  {} {}",
                    m.user_id(),
                    m.role(),
                    m.points(),
                    m.first_name(),
                    m.last_name()
                );
            }
        }
        ProjectsCommand::AttachTeam {
            project_id,
            team_id,
        } => {
            unwrap_or_exit(
                client
                    .attach_team_to_project()
                    .project_id(&project_id)
                    .team_id(&team_id)
                    .send()
                    .await,
                "attach team to project",
            );
            println!("attached team {team_id} to project {project_id}");
        }
        ProjectsCommand::DetachTeam { project_id } => {
            unwrap_or_exit(
                client
                    .detach_team_from_project()
                    .project_id(&project_id)
                    .send()
                    .await,
                "detach team from project",
            );
            println!("detached team from project {project_id}");
        }
        ProjectsCommand::SetRole {
            project_id,
            target_user_id,
            role,
        } => {
            unwrap_or_exit(
                client
                    .set_project_member_role()
                    .project_id(&project_id)
                    .target_user_id(&target_user_id)
                    .role(&role)
                    .send()
                    .await,
                "set project member role",
            );
            println!("set {target_user_id}'s role on project {project_id} to {role}");
        }
        ProjectsCommand::CalendarAdd { project_id, url } => {
            let out = unwrap_or_exit(
                client
                    .create_calendar_subscription()
                    .project_id(&project_id)
                    .ical_url(url)
                    .send()
                    .await,
                "create calendar subscription",
            );
            println!("subscribed calendar {} to project {project_id}", out.id());
        }
        ProjectsCommand::CalendarList { project_id } => {
            let out = unwrap_or_exit(
                client
                    .list_calendar_subscriptions()
                    .project_id(&project_id)
                    .send()
                    .await,
                "list calendar subscriptions",
            );
            if out.subscriptions().is_empty() {
                println!("(no calendar subscriptions)");
                return;
            }
            println!(
                "{:<36}  {:<10}  {:<10}  {}",
                "ID", "LAST SYNC", "ERROR", "URL"
            );
            for s in out.subscriptions() {
                println!(
                    "{:<36}  {:<10}  {:<10}  {}",
                    s.id(),
                    fmt_date_opt(s.last_synced_at()),
                    s.last_sync_error().unwrap_or("-"),
                    s.ical_url()
                );
            }
        }
        ProjectsCommand::CalendarRemove {
            project_id,
            subscription_id,
        } => {
            unwrap_or_exit(
                client
                    .delete_calendar_subscription()
                    .project_id(&project_id)
                    .id(&subscription_id)
                    .send()
                    .await,
                "delete calendar subscription",
            );
            println!("unsubscribed calendar {subscription_id} from project {project_id}");
        }
    }
}
