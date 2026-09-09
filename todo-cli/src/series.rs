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

/// "schedule"/"completion" — a plain, unvalidated-by-Smithy string on the wire (see
/// `ItemSeries::basis`'s doc comment). Only "completion" is ever sent explicitly;
/// "schedule" maps to `None` since that's the server's own default when the field is
/// omitted. The old "due-date" opt-in (materializing a task series onto its due date
/// instead of its scheduled date) was retired 2026-09-05 — a task series now always
/// materializes onto its due date, so there's nothing left for it to opt into.
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
        /// 'schedule' (default) or 'completion' — 'completion' is task-series-only, and
        /// only valid with an "every N days/weeks/months/years" recurrence. A task
        /// series always materializes onto its due date and an event series always
        /// onto its scheduled date; 'basis' only chooses what the next occurrence is
        /// measured from.
        #[arg(long)]
        basis: Option<String>,
        /// User id to assign every materialized occurrence to — only valid on a task
        /// series on a team-backed project
        #[arg(long)]
        assign: Option<String>,
        /// Points awarded to the assignee on each occurrence's completion — only valid
        /// on a task series on a team-backed project, and only settable by that
        /// project's admin (silently dropped otherwise)
        #[arg(long)]
        points: Option<i32>,
        /// 1 (highest) through 4 (lowest) — only valid on a task series; not restricted
        /// to a team-backed project, unlike --assign/--points
        #[arg(long)]
        priority: Option<i32>,
        /// User id to add to the rotation — repeatable, one occurrence per user in
        /// order of user id. Mutually exclusive with --assign. Only valid on a task
        /// series on a team-backed project
        #[arg(long = "rotate")]
        rotate: Vec<String>,
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
        /// 'schedule' (default) or 'completion' — 'completion' is task-series-only, and
        /// only valid with an "every N days/weeks/months/years" recurrence. A task
        /// series always materializes onto its due date and an event series always
        /// onto its scheduled date; 'basis' only chooses what the next occurrence is
        /// measured from.
        #[arg(long)]
        basis: Option<String>,
        /// User id to assign every materialized occurrence to — only valid on a task
        /// series on a team-backed project; round-trip to keep it, omit to clear it
        #[arg(long)]
        assign: Option<String>,
        /// Points awarded to the assignee on each occurrence's completion — only valid
        /// on a task series on a team-backed project, and only settable by that
        /// project's admin; round-trip to keep it, omit to clear it
        #[arg(long)]
        points: Option<i32>,
        /// 1 (highest) through 4 (lowest) — only valid on a task series; round-trip to
        /// keep it, omit to clear it
        #[arg(long)]
        priority: Option<i32>,
        /// User id to add to the rotation — repeatable, one occurrence per user in
        /// order of user id. Mutually exclusive with --assign. Round-trip to keep the
        /// rotation, omit (along with --assign) to clear it
        #[arg(long = "rotate")]
        rotate: Vec<String>,
    },
    /// Delete an item series. Orphan, not cascade: already-materialized occurrences are
    /// kept as standalone items — only the series itself (and its occurrence records) go away.
    Delete {
        project_id: String,
        series_id: String,
    },
    /// Manage a task series' sub-items — the lead-time preparation work every occurrence
    /// carries ("book venue, 30 days before")
    Children {
        #[command(subcommand)]
        command: SeriesChildCommand,
    },
}

/// Sub-item definitions live on the series, not on any one occurrence: each one fans out onto
/// every cycle at its own lead-time date, and only becomes a real item when something persists
/// a change to it. Editing or removing a definition therefore never rewrites already-
/// materialized sub-items of past cycles — those are plain items by then, structurally children
/// of the occurrence they were created under.
///
/// Task-series-only, since a sub-item is a structural child and an Event item can never have
/// children. `delete` is the one exception: it works whatever the series' kind, so a legacy
/// Event-typed series that acquired definitions before that guard landed stays cleanable.
#[derive(Subcommand)]
pub enum SeriesChildCommand {
    /// List a series' sub-item definitions, in authored order
    List {
        project_id: String,
        series_id: String,
    },
    /// Create a sub-item definition, appended at the end of the authored order
    Create {
        project_id: String,
        series_id: String,
        name: String,
        /// Lead time in days before each occurrence's own date — non-negative (0 means due
        /// alongside the occurrence itself)
        days_before: i32,
        #[arg(long)]
        description: Option<String>,
        /// 1 (highest) through 4 (lowest)
        #[arg(long)]
        priority: Option<i32>,
    },
    /// Update a sub-item definition (full replace — round-trip description/priority to keep them)
    Update {
        project_id: String,
        series_id: String,
        child_id: String,
        name: String,
        /// Lead time in days before each occurrence's own date — non-negative
        days_before: i32,
        #[arg(long)]
        description: Option<String>,
        /// 1 (highest) through 4 (lowest); round-trip to keep it, omit to clear it
        #[arg(long)]
        priority: Option<i32>,
    },
    /// Delete a sub-item definition. Orphan, not cascade: sub-items already materialized from
    /// it stay as plain children of their occurrence.
    Delete {
        project_id: String,
        series_id: String,
        child_id: String,
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
            println!(
                "{:<36}  {:<24}  {:<6}  {}",
                "ID", "RECURRENCE", "TYPE", "NAME"
            );
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
            assign,
            points,
            priority,
            rotate,
        } => {
            let anchor_date = parse_date(&anchor).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let Some(item_type) = item_type else {
                eprintln!("error: --item-type is required (task or event)");
                std::process::exit(1);
            };
            if !rotate.is_empty() && assign.is_some() {
                eprintln!("error: --rotate and --assign are mutually exclusive");
                std::process::exit(1);
            }
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
            if let Some(assign) = assign {
                req = req.assigned_to_user_id(assign);
            }
            if let Some(points) = points {
                req = req.points(points);
            }
            if let Some(priority) = priority {
                req = req.priority(priority);
            }
            for user_id in rotate {
                req = req.rotation_user_ids(user_id);
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
            println!(
                "anchor:      {}",
                crate::helpers::fmt_date(out.anchor_date())
            );
            println!("item type:   {}", out.item_type());
            println!("basis:       {}", out.basis().unwrap_or("DEFAULT"));
            println!("assigned to: {}", out.assigned_to_user_id().unwrap_or("-"));
            println!(
                "points:      {}",
                out.points()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!(
                "priority:    {}",
                out.priority()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!(
                "rotation:    {}",
                if out.rotation_user_ids().is_empty() {
                    "-".to_string()
                } else {
                    out.rotation_user_ids().join(", ")
                }
            );
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
            assign,
            points,
            priority,
            rotate,
        } => {
            let anchor_date = parse_date(&anchor).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let Some(item_type) = item_type else {
                eprintln!("error: --item-type is required (task or event)");
                std::process::exit(1);
            };
            if !rotate.is_empty() && assign.is_some() {
                eprintln!("error: --rotate and --assign are mutually exclusive");
                std::process::exit(1);
            }
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
            if let Some(assign) = assign {
                req = req.assigned_to_user_id(assign);
            }
            if let Some(points) = points {
                req = req.points(points);
            }
            if let Some(priority) = priority {
                req = req.priority(priority);
            }
            for user_id in rotate {
                req = req.rotation_user_ids(user_id);
            }
            unwrap_or_exit(req.send().await, "update item series");
            println!("updated item series {series_id}");
        }
        SeriesCommand::Delete {
            project_id,
            series_id,
        } => {
            unwrap_or_exit(
                client
                    .delete_item_series()
                    .project_id(&project_id)
                    .series_id(&series_id)
                    .send()
                    .await,
                "delete item series",
            );
            println!("deleted item series {series_id}");
        }
        SeriesCommand::Children { command } => cmd_series_children(client, command).await,
    }
}

async fn cmd_series_children(client: &Client, cmd: SeriesChildCommand) {
    match cmd {
        SeriesChildCommand::List {
            project_id,
            series_id,
        } => {
            let out = unwrap_or_exit(
                client
                    .list_item_series_children()
                    .project_id(&project_id)
                    .series_id(&series_id)
                    .send()
                    .await,
                "list series sub-items",
            );
            if out.children().is_empty() {
                println!("(no sub-items)");
                return;
            }
            println!(
                "{:<36}  {:>12}  {:>8}  {}",
                "ID", "DAYS BEFORE", "PRIORITY", "NAME"
            );
            for c in out.children() {
                println!(
                    "{:<36}  {:>12}  {:>8}  {}",
                    c.child_id(),
                    c.days_before(),
                    c.priority()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    c.name()
                );
            }
        }
        SeriesChildCommand::Create {
            project_id,
            series_id,
            name,
            days_before,
            description,
            priority,
        } => {
            let mut req = client
                .create_item_series_child()
                .project_id(&project_id)
                .series_id(&series_id)
                .name(name)
                .days_before(days_before);
            if let Some(description) = description {
                req = req.description(description);
            }
            if let Some(priority) = priority {
                req = req.priority(priority);
            }
            let out = unwrap_or_exit(req.send().await, "create series sub-item");
            println!("created sub-item {}", out.child_id());
        }
        SeriesChildCommand::Update {
            project_id,
            series_id,
            child_id,
            name,
            days_before,
            description,
            priority,
        } => {
            let mut req = client
                .update_item_series_child()
                .project_id(&project_id)
                .series_id(&series_id)
                .child_id(&child_id)
                .name(name)
                .days_before(days_before);
            if let Some(description) = description {
                req = req.description(description);
            }
            if let Some(priority) = priority {
                req = req.priority(priority);
            }
            unwrap_or_exit(req.send().await, "update series sub-item");
            println!("updated sub-item {child_id}");
        }
        SeriesChildCommand::Delete {
            project_id,
            series_id,
            child_id,
        } => {
            unwrap_or_exit(
                client
                    .delete_item_series_child()
                    .project_id(&project_id)
                    .series_id(&series_id)
                    .child_id(&child_id)
                    .send()
                    .await,
                "delete series sub-item",
            );
            println!("deleted sub-item {child_id}");
        }
    }
}
