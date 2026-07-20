use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Set the API URL
    SetUrl { url: String },
    /// Set the Bearer token
    SetToken { token: String },
    /// Set the default user ID
    SetUser { user_id: String },
    /// Show current config
    Show,
}

pub fn cmd_config(cmd: ConfigCommand) {
    let mut cfg = load_config();
    match cmd {
        ConfigCommand::SetUrl { url } => {
            cfg.url = Some(url.clone());
            save_config(&cfg);
            println!("url set to {url}");
        }
        ConfigCommand::SetToken { token } => {
            cfg.token = Some(token);
            save_config(&cfg);
            println!("token saved");
        }
        ConfigCommand::SetUser { user_id } => {
            cfg.user_id = Some(user_id.clone());
            save_config(&cfg);
            println!("default user set to {user_id}");
        }
        ConfigCommand::Show => {
            let path = config_path();
            println!("config file: {}", path.display());
            println!("url:         {}", cfg.url.as_deref().unwrap_or("(not set)"));
            println!(
                "token:       {}",
                cfg.token.as_deref().map(|_| "(set)").unwrap_or("(not set)")
            );
            println!(
                "user_id:     {}",
                cfg.user_id.as_deref().unwrap_or("(not set)")
            );
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Config {
    pub url: Option<String>,
    pub token: Option<String>,
    pub user_id: Option<String>,
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prl")
        .join("config.toml")
}

pub fn load_config() -> Config {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(cfg: &Config) {
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string_pretty(cfg).unwrap()).unwrap();
}
