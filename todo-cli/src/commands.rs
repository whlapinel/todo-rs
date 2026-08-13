use crate::items::ItemsCommand;
use crate::projects::ProjectsCommand;
use crate::teams::TeamsCommand;
use crate::users::UsersCommand;
use crate::config::ConfigCommand;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "prl",
    about = "PeoplesRepublicOfLists — official task management CLI"
)]
pub struct Cli {
    #[arg(long, env = "TODO_URL", global = true, help = "API base URL")]
    pub url: Option<String>,

    #[arg(long, env = "TODO_TOKEN", global = true, help = "Bearer auth token")]
    pub token: Option<String>,

    #[arg(long, env = "TODO_USER", global = true, help = "Default user ID")]
    pub user: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Manage items
    Items {
        #[command(subcommand)]
        command: ItemsCommand,
    },
    /// Manage users
    Users {
        #[command(subcommand)]
        command: UsersCommand,
    },
    /// Manage teams
    Teams {
        #[command(subcommand)]
        command: TeamsCommand,
    },
    /// Manage projects
    Projects {
        #[command(subcommand)]
        command: ProjectsCommand,
    },
    /// Configure prl
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}
