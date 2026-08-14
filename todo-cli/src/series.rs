use crate::helpers::{parse_date, unwrap_or_exit};
use clap::Subcommand;
use todo_client::Client;

#[derive(Subcommand)]
pub enum SeriesCommand {
    /// List a project's recurring event series
    List { project_id: String },
    /// Create a new recurring event series
    Create {
        project_id: String,
        name: String,
        /// Recurrence pattern, e.g. "every monday" — same syntax as an item's recurrence
        recurrence: String,
        /// Anchor date: YYYY-MM-DD or a Unix timestamp
        anchor: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        event_type: Option<String>,
    },
    /// Show one event series
    Get {
        project_id: String,
        series_id: String,
    },
    /// Update an event series (full replace — round-trip description/event-type to keep them)
    Update {
        project_id: String,
        series_id: String,
        name: String,
        recurrence: String,
        anchor: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        event_type: Option<String>,
    },
}

pub async fn cmd_series(client: &Client, cmd: SeriesCommand, _user_id: Option<String>) {
    match cmd {
        SeriesCommand::List { project_id } => {
            let out = unwrap_or_exit(
                client
                    .list_event_series_for_project()
                    .project_id(&project_id)
                    .send()
                    .await,
                "list event series",
            );
            if out.series().is_empty() {
                println!("(no event series)");
                return;
            }
            println!("{:<36}  {:<24}  {}", "ID", "RECURRENCE", "NAME");
            for s in out.series() {
                println!("{:<36}  {:<24}  {}", s.series_id(), s.recurrence(), s.name());
            }
        }
        SeriesCommand::Create {
            project_id,
            name,
            recurrence,
            anchor,
            description,
            event_type,
        } => {
            let anchor_date = parse_date(&anchor).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let mut req = client
                .create_event_series()
                .project_id(&project_id)
                .name(name)
                .recurrence(recurrence)
                .anchor_date(anchor_date);
            if let Some(description) = description {
                req = req.description(description);
            }
            if let Some(event_type) = event_type {
                req = req.event_type(event_type);
            }
            let out = unwrap_or_exit(req.send().await, "create event series");
            println!("created event series {}", out.series_id());
        }
        SeriesCommand::Get {
            project_id,
            series_id,
        } => {
            let out = unwrap_or_exit(
                client
                    .get_event_series()
                    .project_id(&project_id)
                    .series_id(&series_id)
                    .send()
                    .await,
                "get event series",
            );
            println!("id:          {}", out.series_id());
            println!("project:     {}", out.project_id());
            println!("name:        {}", out.name());
            println!("description: {}", out.description().unwrap_or("-"));
            println!("event type:  {}", out.event_type().unwrap_or("-"));
            println!("recurrence:  {}", out.recurrence());
            println!("anchor:      {}", crate::helpers::fmt_date(out.anchor_date()));
        }
        SeriesCommand::Update {
            project_id,
            series_id,
            name,
            recurrence,
            anchor,
            description,
            event_type,
        } => {
            let anchor_date = parse_date(&anchor).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let mut req = client
                .update_event_series()
                .project_id(&project_id)
                .series_id(&series_id)
                .name(name)
                .recurrence(recurrence)
                .anchor_date(anchor_date);
            if let Some(description) = description {
                req = req.description(description);
            }
            if let Some(event_type) = event_type {
                req = req.event_type(event_type);
            }
            unwrap_or_exit(req.send().await, "update event series");
            println!("updated event series {series_id}");
        }
    }
}
