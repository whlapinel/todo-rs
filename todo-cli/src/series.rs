use crate::helpers::{parse_date, unwrap_or_exit};
use clap::Subcommand;
use todo_client::types::ItemType;
use todo_client::Client;

fn parse_series_item_type_flag(s: &str) -> ItemType {
    match s.to_lowercase().as_str() {
        "task" => ItemType::Task,
        "event" => ItemType::Event,
        _ => {
            eprintln!("error: --item-type must be 'task' or 'event'");
            std::process::exit(1);
        }
    }
}

/// Stage 10 gap 1: "schedule"/"completion" — a plain, unvalidated-by-Smithy string on
/// the wire (see `ItemSeries::basis`'s doc comment), same as `--recurrence-basis` on
/// `prl items add`. Only "completion" is ever sent explicitly; "schedule" maps to
/// `None` since that's the server's own default when the field is omitted.
fn parse_series_basis_flag(s: &str) -> Option<String> {
    match s.to_lowercase().as_str() {
        "schedule" => None,
        "completion" => Some("COMPLETION".to_string()),
        _ => {
            eprintln!("error: --basis must be 'schedule' or 'completion'");
            std::process::exit(1);
        }
    }
}

#[derive(Subcommand)]
pub enum SeriesCommand {
    /// List a project's recurring item series
    List { project_id: String },
    /// Create a new recurring item series
    Create {
        project_id: String,
        name: String,
        /// Recurrence pattern, e.g. "every monday" — same syntax as an item's recurrence
        recurrence: String,
        /// Anchor date: YYYY-MM-DD or a Unix timestamp
        anchor: String,
        #[arg(long)]
        description: Option<String>,
        /// Required: 'task' or 'event' — the kind of item this series materializes
        #[arg(long)]
        item_type: Option<String>,
        /// 'schedule' (default) or 'completion' — only valid on a task series with an
        /// "every N days/weeks/months/years" recurrence
        #[arg(long)]
        basis: Option<String>,
    },
    /// Show one item series
    Get {
        project_id: String,
        series_id: String,
    },
    /// Update an item series (full replace — round-trip description/basis to keep them)
    Update {
        project_id: String,
        series_id: String,
        name: String,
        recurrence: String,
        anchor: String,
        #[arg(long)]
        description: Option<String>,
        /// Required: 'task' or 'event' — the kind of item this series materializes
        #[arg(long)]
        item_type: Option<String>,
        /// 'schedule' (default) or 'completion' — only valid on a task series with an
        /// "every N days/weeks/months/years" recurrence
        #[arg(long)]
        basis: Option<String>,
    },
}

pub async fn cmd_series(client: &Client, cmd: SeriesCommand, _user_id: Option<String>) {
    match cmd {
        SeriesCommand::List { project_id } => {
            let out = unwrap_or_exit(
                client
                    .list_item_series_for_project()
                    .project_id(&project_id)
                    .send()
                    .await,
                "list item series",
            );
            if out.series().is_empty() {
                println!("(no item series)");
                return;
            }
            println!("{:<36}  {:<24}  {:<6}  {}", "ID", "RECURRENCE", "TYPE", "NAME");
            for s in out.series() {
                println!(
                    "{:<36}  {:<24}  {:<6}  {}",
                    s.series_id(),
                    s.recurrence(),
                    s.item_type(),
                    s.name()
                );
            }
        }
        SeriesCommand::Create {
            project_id,
            name,
            recurrence,
            anchor,
            description,
            item_type,
            basis,
        } => {
            let anchor_date = parse_date(&anchor).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let Some(item_type) = item_type else {
                eprintln!("error: --item-type is required (task or event)");
                std::process::exit(1);
            };
            let mut req = client
                .create_item_series()
                .project_id(&project_id)
                .name(name)
                .recurrence(recurrence)
                .anchor_date(anchor_date)
                .item_type(parse_series_item_type_flag(&item_type));
            if let Some(description) = description {
                req = req.description(description);
            }
            if let Some(basis) = basis.and_then(|b| parse_series_basis_flag(&b)) {
                req = req.basis(basis);
            }
            let out = unwrap_or_exit(req.send().await, "create item series");
            println!("created item series {}", out.series_id());
        }
        SeriesCommand::Get {
            project_id,
            series_id,
        } => {
            let out = unwrap_or_exit(
                client
                    .get_item_series()
                    .project_id(&project_id)
                    .series_id(&series_id)
                    .send()
                    .await,
                "get item series",
            );
            println!("id:          {}", out.series_id());
            println!("project:     {}", out.project_id());
            println!("name:        {}", out.name());
            println!("description: {}", out.description().unwrap_or("-"));
            println!("event type:  {}", out.event_type().unwrap_or("-"));
            println!("recurrence:  {}", out.recurrence());
            println!("anchor:      {}", crate::helpers::fmt_date(out.anchor_date()));
            println!("item type:   {}", out.item_type());
            println!("basis:       {}", out.basis().unwrap_or("SCHEDULE"));
        }
        SeriesCommand::Update {
            project_id,
            series_id,
            name,
            recurrence,
            anchor,
            description,
            item_type,
            basis,
        } => {
            let anchor_date = parse_date(&anchor).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let Some(item_type) = item_type else {
                eprintln!("error: --item-type is required (task or event)");
                std::process::exit(1);
            };
            let mut req = client
                .update_item_series()
                .project_id(&project_id)
                .series_id(&series_id)
                .name(name)
                .recurrence(recurrence)
                .anchor_date(anchor_date)
                .item_type(parse_series_item_type_flag(&item_type));
            if let Some(description) = description {
                req = req.description(description);
            }
            if let Some(basis) = basis.and_then(|b| parse_series_basis_flag(&b)) {
                req = req.basis(basis);
            }
            unwrap_or_exit(req.send().await, "update item series");
            println!("updated item series {series_id}");
        }
    }
}
