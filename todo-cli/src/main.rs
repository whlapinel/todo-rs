use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// --- CLI definition ---

#[derive(Parser)]
#[command(name = "prl", about = "PeoplesRepublicOfLists — official task management CLI")]
struct Cli {
    #[arg(long, env = "TODO_URL", global = true, help = "API base URL")]
    url: Option<String>,

    #[arg(long, env = "TODO_TOKEN", global = true, help = "Bearer auth token")]
    token: Option<String>,

    #[arg(long, env = "TODO_USER", global = true, help = "Default user ID")]
    user: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    /// Configure prl
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
enum ItemsCommand {
    /// List items (optionally under a parent)
    List {
        #[arg(long, help = "Filter by parent item ID")]
        parent: Option<String>,
    },
    /// Add a new item
    Add {
        name: String,
        #[arg(long, help = "Due date: YYYY-MM-DD or Unix timestamp")]
        due: Option<String>,
        #[arg(long, help = "Parent item ID")]
        parent: Option<String>,
        #[arg(long, help = "Recurrence pattern, e.g. 'every week'")]
        recurrence: Option<String>,
        #[arg(long, help = "Assign to a fellow active team member's user ID")]
        assign: Option<String>,
    },
    /// Mark an item complete
    Done { item_id: String },
    /// Delete an item
    Delete { item_id: String },
    /// Show item details
    Get { item_id: String },
    /// List items due in a time window
    Due {
        #[arg(long, help = "Only items due after this date (YYYY-MM-DD)")]
        after: Option<String>,
        #[arg(long, help = "Only items due before this date (YYYY-MM-DD)")]
        before: Option<String>,
    },
    /// Assign an item to a fellow active team member
    Assign { item_id: String, user_id: String },
    /// Remove an item's assignee
    Unassign { item_id: String },
    /// List items assigned to you by other team members
    Assigned,
}

#[derive(Subcommand)]
enum UsersCommand {
    /// List all users
    List,
    /// Show a user (defaults to configured user)
    Get { user_id: Option<String> },
}

#[derive(Subcommand)]
enum TeamsCommand {
    /// List your teams, including pending invites
    List,
    /// Create a new team (you become its first member)
    Create { name: String },
    /// List a team's members
    Members { team_id: String },
    /// Invite an existing user to a team
    Invite { team_id: String, invitee_user_id: String },
    /// Accept a pending team invite
    Accept { team_id: String },
    /// Leave a team (or decline a pending invite)
    Leave { team_id: String },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Set the API URL
    SetUrl { url: String },
    /// Set the Bearer token
    SetToken { token: String },
    /// Set the default user ID
    SetUser { user_id: String },
    /// Show current config
    Show,
}

// --- Config file ---

#[derive(Serialize, Deserialize, Default, Clone)]
struct Config {
    url: Option<String>,
    token: Option<String>,
    user_id: Option<String>,
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prl")
        .join("config.toml")
}

fn load_config() -> Config {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &Config) {
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string_pretty(cfg).unwrap()).unwrap();
}

// --- HTTP client wrapper ---

struct Api {
    base: String,
    client: reqwest::blocking::Client,
}

impl Api {
    fn new(url: &str, token: &str) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let client = reqwest::blocking::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();
        Self { base: format!("{}/api", url.trim_end_matches('/')), client }
    }

    fn get(&self, path: &str) -> reqwest::Result<reqwest::blocking::Response> {
        self.client.get(format!("{}{}", self.base, path)).send()
    }

    fn post(&self, path: &str, body: &impl Serialize) -> reqwest::Result<reqwest::blocking::Response> {
        self.client.post(format!("{}{}", self.base, path)).json(body).send()
    }

    fn put(&self, path: &str, body: &impl Serialize) -> reqwest::Result<reqwest::blocking::Response> {
        self.client.put(format!("{}{}", self.base, path)).json(body).send()
    }

    fn delete(&self, path: &str) -> reqwest::Result<reqwest::blocking::Response> {
        self.client.delete(format!("{}{}", self.base, path)).send()
    }
}

// --- API response types ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserSummary {
    user_id: String,
    first_name: String,
    last_name: String,
}

#[derive(Deserialize)]
struct ListUsersOutput {
    users: Vec<UserSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemSummary {
    item_id: Option<String>,
    name: Option<String>,
    due_date: Option<f64>,
    complete: Option<bool>,
    recurrence: Option<String>,
    has_due_time: Option<bool>,
    parent_item_id: Option<String>,
    has_children: Option<bool>,
}

#[derive(Deserialize)]
struct ListItemsOutput {
    items: Vec<ItemSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetItemOutput {
    name: String,
    due_date: Option<f64>,
    complete: bool,
    recurrence: Option<String>,
    has_due_time: Option<bool>,
    parent_item_id: Option<String>,
    has_children: Option<bool>,
    assigned_to_user_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateItemOutput {
    item_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DueItemSummary {
    item_id: String,
    name: String,
    parent_name: Option<String>,
    due_date: Option<f64>,
    complete: Option<bool>,
    recurrence: Option<String>,
}

#[derive(Deserialize)]
struct ListItemsDueOutput {
    items: Vec<DueItemSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamSummary {
    team_id: String,
    name: String,
    status: String,
    invited_by_name: Option<String>,
}

#[derive(Deserialize)]
struct ListTeamsOutput {
    teams: Vec<TeamSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssignedItemSummary {
    item_id: String,
    name: String,
    owner_user_id: String,
    due_date: Option<f64>,
    complete: Option<bool>,
    recurrence: Option<String>,
}

#[derive(Deserialize)]
struct ListAssignedItemsOutput {
    items: Vec<AssignedItemSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamMemberSummary {
    user_id: String,
    first_name: String,
    last_name: String,
    status: String,
}

#[derive(Deserialize)]
struct ListTeamMembersOutput {
    members: Vec<TeamMemberSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTeamOutput {
    team_id: String,
}

// --- API request types ---

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateItemBody {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recurrence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assigned_to_user_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateItemBody {
    name: String,
    complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recurrence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recurrence_basis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    has_due_time: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assigned_to_user_id: Option<String>,
}

#[derive(Serialize)]
struct CreateTeamBody {
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteTeamMemberBody {
    invitee_user_id: String,
}

// --- Helpers ---

fn parse_date(s: &str) -> Result<f64, String> {
    if let Ok(ts) = s.parse::<f64>() {
        return Ok(ts);
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|d| {
            Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .timestamp() as f64
        })
        .map_err(|_| format!("invalid date '{s}' — use YYYY-MM-DD or a Unix timestamp"))
}

fn fmt_date(secs: f64) -> String {
    if secs == 0.0 {
        return "-".to_string();
    }
    DateTime::from_timestamp(secs as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn check(resp: reqwest::blocking::Response, action: &str) -> String {
    if resp.status().is_success() {
        resp.text().unwrap_or_default()
    } else {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        eprintln!("error: {action} failed ({status}): {body}");
        std::process::exit(1);
    }
}

fn require_user(user: Option<String>) -> String {
    user.unwrap_or_else(|| {
        eprintln!("error: no user ID — set one with `prl config set-user <id>` or pass --user");
        std::process::exit(1);
    })
}

// --- Command handlers ---

fn cmd_users(api: &Api, cmd: UsersCommand, default_user: Option<String>) {
    match cmd {
        UsersCommand::List => {
            let body = check(api.get("/users").unwrap(), "list users");
            let out: ListUsersOutput = serde_json::from_str(&body).unwrap();
            println!("{:<36}  {}", "ID", "NAME");
            for u in out.users {
                println!("{:<36}  {} {}", u.user_id, u.first_name, u.last_name);
            }
        }
        UsersCommand::Get { user_id } => {
            let id = user_id.or(default_user);
            let id = require_user(id);
            let body = check(api.get(&format!("/users/{id}")).unwrap(), "get user");
            let out: UserSummary = serde_json::from_str(&body).unwrap();
            println!("id:   {}", out.user_id);
            println!("name: {} {}", out.first_name, out.last_name);
        }
    }
}

fn cmd_items(api: &Api, cmd: ItemsCommand, user_id: Option<String>) {
    match cmd {
        ItemsCommand::List { parent } => {
            let uid = require_user(user_id);
            let url = match &parent {
                Some(p) => format!("/users/{uid}/items?parentItemId={p}"),
                None => format!("/users/{uid}/items"),
            };
            let body = check(api.get(&url).unwrap(), "list items");
            let out: ListItemsOutput = serde_json::from_str(&body).unwrap();
            if out.items.is_empty() {
                println!("(no items)");
                return;
            }
            println!("{:<36}  {:<4}  {:<12}  {}", "ID", "DONE", "DUE", "NAME");
            for i in out.items {
                let id = i.item_id.as_deref().unwrap_or("-");
                let name = i.name.as_deref().unwrap_or("-");
                let done = if i.complete.unwrap_or(false) { "✓" } else { " " };
                let due = i.due_date.map(fmt_date).unwrap_or_else(|| "-".into());
                let suffix = if i.has_children.unwrap_or(false) { " ▸" } else { "" };
                println!("{:<36}  {:<4}  {:<12}  {}{}", id, done, due, name, suffix);
            }
        }
        ItemsCommand::Add { name, due, parent, recurrence, assign } => {
            let uid = require_user(user_id);
            let due_date = match due {
                Some(ref s) => Some(parse_date(s).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1); })),
                None => None,
            };
            let body = check(
                api.post(
                    &format!("/users/{uid}/items"),
                    &CreateItemBody { name, due_date, parent_item_id: parent, recurrence, assigned_to_user_id: assign },
                )
                .unwrap(),
                "add item",
            );
            let out: CreateItemOutput = serde_json::from_str(&body).unwrap();
            println!("created item {}", out.item_id);
        }
        ItemsCommand::Done { item_id } => {
            let uid = require_user(user_id);
            let body = check(api.get(&format!("/users/{uid}/items/{item_id}")).unwrap(), "get item");
            let item: GetItemOutput = serde_json::from_str(&body).unwrap();
            let update = UpdateItemBody {
                name: item.name,
                complete: true,
                due_date: item.due_date,
                recurrence: item.recurrence,
                recurrence_basis: None,
                has_due_time: item.has_due_time,
                parent_item_id: item.parent_item_id,
                assigned_to_user_id: item.assigned_to_user_id,
            };
            check(api.put(&format!("/users/{uid}/items/{item_id}"), &update).unwrap(), "mark done");
            println!("marked {item_id} complete");
        }
        ItemsCommand::Assign { item_id, user_id: assignee } => {
            let uid = require_user(user_id);
            let body = check(api.get(&format!("/users/{uid}/items/{item_id}")).unwrap(), "get item");
            let item: GetItemOutput = serde_json::from_str(&body).unwrap();
            let update = UpdateItemBody {
                name: item.name,
                complete: item.complete,
                due_date: item.due_date,
                recurrence: item.recurrence,
                recurrence_basis: None,
                has_due_time: item.has_due_time,
                parent_item_id: item.parent_item_id,
                assigned_to_user_id: Some(assignee),
            };
            check(api.put(&format!("/users/{uid}/items/{item_id}"), &update).unwrap(), "assign item");
            println!("assigned {item_id}");
        }
        ItemsCommand::Unassign { item_id } => {
            let uid = require_user(user_id);
            let body = check(api.get(&format!("/users/{uid}/items/{item_id}")).unwrap(), "get item");
            let item: GetItemOutput = serde_json::from_str(&body).unwrap();
            let update = UpdateItemBody {
                name: item.name,
                complete: item.complete,
                due_date: item.due_date,
                recurrence: item.recurrence,
                recurrence_basis: None,
                has_due_time: item.has_due_time,
                parent_item_id: item.parent_item_id,
                assigned_to_user_id: None,
            };
            check(api.put(&format!("/users/{uid}/items/{item_id}"), &update).unwrap(), "unassign item");
            println!("unassigned {item_id}");
        }
        ItemsCommand::Assigned => {
            let uid = require_user(user_id);
            let body = check(api.get(&format!("/users/{uid}/assigned-items")).unwrap(), "list assigned items");
            let out: ListAssignedItemsOutput = serde_json::from_str(&body).unwrap();
            if out.items.is_empty() {
                println!("(nothing assigned to you)");
                return;
            }
            println!("{:<36}  {:<4}  {:<12}  {:<36}  {}", "ID", "DONE", "DUE", "OWNER", "NAME");
            for i in out.items {
                let done = if i.complete.unwrap_or(false) { "✓" } else { " " };
                let due = i.due_date.map(fmt_date).unwrap_or_else(|| "-".into());
                println!("{:<36}  {:<4}  {:<12}  {:<36}  {}", i.item_id, done, due, i.owner_user_id, i.name);
            }
        }
        ItemsCommand::Delete { item_id } => {
            let uid = require_user(user_id);
            check(api.delete(&format!("/users/{uid}/items/{item_id}")).unwrap(), "delete item");
            println!("deleted {item_id}");
        }
        ItemsCommand::Get { item_id } => {
            let uid = require_user(user_id);
            let body = check(api.get(&format!("/users/{uid}/items/{item_id}")).unwrap(), "get item");
            let item: GetItemOutput = serde_json::from_str(&body).unwrap();
            println!("id:         {item_id}");
            println!("name:       {}", item.name);
            println!("complete:   {}", item.complete);
            println!("due:        {}", item.due_date.map(fmt_date).unwrap_or_else(|| "-".into()));
            println!("recurrence: {}", item.recurrence.as_deref().unwrap_or("-"));
            println!("children:   {}", item.has_children.unwrap_or(false));
            println!("assigned:   {}", item.assigned_to_user_id.as_deref().unwrap_or("-"));
        }
        ItemsCommand::Due { after, before } => {
            let uid = require_user(user_id);
            let mut params = vec![];
            if let Some(a) = after {
                let ts = parse_date(&a).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1); });
                params.push(format!("deadlineAfter={}", ts as i64));
            }
            if let Some(b) = before {
                let ts = parse_date(&b).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1); });
                params.push(format!("deadlineBefore={}", ts as i64));
            }
            let qs = if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
            let body = check(api.get(&format!("/users/{uid}/due-items{qs}")).unwrap(), "list due items");
            let out: ListItemsDueOutput = serde_json::from_str(&body).unwrap();
            if out.items.is_empty() {
                println!("(nothing due)");
                return;
            }
            println!("{:<36}  {:<4}  {:<12}  {:<24}  {}", "ID", "DONE", "DUE", "PARENT", "NAME");
            for i in out.items {
                let done = if i.complete.unwrap_or(false) { "✓" } else { " " };
                let due = i.due_date.map(fmt_date).unwrap_or_else(|| "-".into());
                let parent = i.parent_name.as_deref().unwrap_or("-");
                println!("{:<36}  {:<4}  {:<12}  {:<24}  {}", i.item_id, done, due, parent, i.name);
            }
        }
    }
}

fn cmd_teams(api: &Api, cmd: TeamsCommand, user_id: Option<String>) {
    match cmd {
        TeamsCommand::List => {
            let uid = require_user(user_id);
            let body = check(api.get(&format!("/users/{uid}/teams")).unwrap(), "list teams");
            let out: ListTeamsOutput = serde_json::from_str(&body).unwrap();
            if out.teams.is_empty() {
                println!("(no teams)");
                return;
            }
            println!("{:<36}  {:<8}  {}", "ID", "STATUS", "NAME");
            for t in out.teams {
                let suffix = t
                    .invited_by_name
                    .map(|n| format!("  (invited by {n})"))
                    .unwrap_or_default();
                println!("{:<36}  {:<8}  {}{}", t.team_id, t.status, t.name, suffix);
            }
        }
        TeamsCommand::Create { name } => {
            let uid = require_user(user_id);
            let body = check(
                api.post(&format!("/users/{uid}/teams"), &CreateTeamBody { name }).unwrap(),
                "create team",
            );
            let out: CreateTeamOutput = serde_json::from_str(&body).unwrap();
            println!("created team {}", out.team_id);
        }
        TeamsCommand::Members { team_id } => {
            let uid = require_user(user_id);
            let body = check(
                api.get(&format!("/users/{uid}/teams/{team_id}/members")).unwrap(),
                "list team members",
            );
            let out: ListTeamMembersOutput = serde_json::from_str(&body).unwrap();
            println!("{:<36}  {:<8}  {}", "USER ID", "STATUS", "NAME");
            for m in out.members {
                println!(
                    "{:<36}  {:<8}  {} {}",
                    m.user_id, m.status, m.first_name, m.last_name
                );
            }
        }
        TeamsCommand::Invite { team_id, invitee_user_id } => {
            let uid = require_user(user_id);
            check(
                api.post(
                    &format!("/users/{uid}/teams/{team_id}/invites"),
                    &InviteTeamMemberBody { invitee_user_id: invitee_user_id.clone() },
                )
                .unwrap(),
                "invite team member",
            );
            println!("invited {invitee_user_id} to team {team_id}");
        }
        TeamsCommand::Accept { team_id } => {
            let uid = require_user(user_id);
            check(
                api.put(&format!("/users/{uid}/teams/{team_id}/accept"), &serde_json::json!({}))
                    .unwrap(),
                "accept team invite",
            );
            println!("joined team {team_id}");
        }
        TeamsCommand::Leave { team_id } => {
            let uid = require_user(user_id);
            check(
                api.delete(&format!("/users/{uid}/teams/{team_id}/membership")).unwrap(),
                "leave team",
            );
            println!("left team {team_id}");
        }
    }
}

fn cmd_config(cmd: ConfigCommand) {
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
            println!("token:       {}", cfg.token.as_deref().map(|_| "(set)").unwrap_or("(not set)"));
            println!("user_id:     {}", cfg.user_id.as_deref().unwrap_or("(not set)"));
        }
    }
}

// --- Main ---

fn main() {
    let cli = Cli::parse();
    let file_cfg = load_config();

    let url = cli.url.or(file_cfg.url.clone());
    let token = cli.token.or(file_cfg.token.clone());
    let user = cli.user.or(file_cfg.user_id.clone());

    match cli.command {
        Command::Config { command } => {
            cmd_config(command);
        }
        Command::Users { command } => {
            let url = url.unwrap_or_else(|| {
                eprintln!("error: no URL — run `prl config set-url <url>` or pass --url");
                std::process::exit(1);
            });
            let token = token.unwrap_or_else(|| {
                eprintln!("error: no token — run `prl config set-token <token>` or pass --token");
                std::process::exit(1);
            });
            let api = Api::new(&url, &token);
            cmd_users(&api, command, user);
        }
        Command::Items { command } => {
            let url = url.unwrap_or_else(|| {
                eprintln!("error: no URL — run `prl config set-url <url>` or pass --url");
                std::process::exit(1);
            });
            let token = token.unwrap_or_else(|| {
                eprintln!("error: no token — run `prl config set-token <token>` or pass --token");
                std::process::exit(1);
            });
            let api = Api::new(&url, &token);
            cmd_items(&api, command, user);
        }
        Command::Teams { command } => {
            let url = url.unwrap_or_else(|| {
                eprintln!("error: no URL — run `prl config set-url <url>` or pass --url");
                std::process::exit(1);
            });
            let token = token.unwrap_or_else(|| {
                eprintln!("error: no token — run `prl config set-token <token>` or pass --token");
                std::process::exit(1);
            });
            let api = Api::new(&url, &token);
            cmd_teams(&api, command, user);
        }
    }
}
