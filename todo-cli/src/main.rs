use clap::Parser;

mod client;
mod commands;
mod config;
mod helpers;
mod items;
mod projects;
mod teams;
mod users;

#[tokio::main]
async fn main() {
    let cli = commands::Cli::parse();
    let file_cfg = config::load_config();

    let url = cli.url.or(file_cfg.url.clone());
    let token = cli.token.or(file_cfg.token.clone());
    let user = cli.user.or(file_cfg.user_id.clone());

    match cli.command {
        commands::Command::Config { command } => {
            config::cmd_config(command);
        }
        commands::Command::Users { command } => {
            let client = build_client(url, token);
            users::cmd_users(&client, command, user).await;
        }
        commands::Command::Items { command } => {
            let client = build_client(url, token);
            items::cmd_items(&client, command, user).await;
        }
        commands::Command::Teams { command } => {
            let client = build_client(url, token);
            teams::cmd_teams(&client, command, user).await;
        }
        commands::Command::Projects { command } => {
            let client = build_client(url, token);
            projects::cmd_projects(&client, command, user).await;
        }
    }
}

fn build_client(url: Option<String>, token: Option<String>) -> todo_client::Client {
    let url = url.unwrap_or_else(|| {
        eprintln!("error: no URL — run `prl config set-url <url>` or pass --url");
        std::process::exit(1);
    });
    let token = token.unwrap_or_else(|| {
        eprintln!("error: no token — run `prl config set-token <token>` or pass --token");
        std::process::exit(1);
    });
    client::build_client(&url, &token)
}
