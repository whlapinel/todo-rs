use crate::helpers::{fmt_date, fmt_date_opt, parse_date, require_user, unwrap_or_exit};
use clap::Subcommand;
use todo_client::Client;

#[derive(Subcommand)]
pub enum ItemsCommand {
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
        #[arg(
            long,
            help = "Recurrence pattern, e.g. 'every week' (top-level items only)"
        )]
        recurrence: Option<String>,
        #[arg(
            long,
            help = "Days from the top-level item's due date (child items only; negative = before, positive = after)"
        )]
        due_offset_days: Option<i32>,
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
    /// List items assigned to you by other team members
    Assigned,
}

pub async fn cmd_items(client: &Client, cmd: ItemsCommand, user_id: Option<String>) {
    match cmd {
        ItemsCommand::List { parent } => {
            let uid = require_user(user_id);
            let mut req = client.list_items().user_id(uid);
            if let Some(p) = parent {
                req = req.parent_item_id(p);
            }
            let out = unwrap_or_exit(req.send().await, "list items");
            if out.items().is_empty() {
                println!("(no items)");
                return;
            }
            println!("{:<36}  {:<4}  {:<12}  {}", "ID", "DONE", "DUE", "NAME");
            for i in out.items() {
                let id = i.item_id().unwrap_or("-");
                let name = i.name().unwrap_or("-");
                let done = if i.complete().unwrap_or(false) {
                    "✓"
                } else {
                    " "
                };
                let due = fmt_date_opt(i.due_date());
                let suffix = if i.has_children().unwrap_or(false) {
                    " ▸"
                } else {
                    ""
                };
                println!("{:<36}  {:<4}  {:<12}  {}{}", id, done, due, name, suffix);
            }
        }
        ItemsCommand::Add {
            name,
            due,
            parent,
            recurrence,
            due_offset_days,
        } => {
            if parent.is_some() && recurrence.is_some() {
                eprintln!(
                    "error: --recurrence can't be combined with --parent — child items can't have their own recurrence; use --due-offset-days instead"
                );
                std::process::exit(1);
            }
            let uid = require_user(user_id);
            let mut req = client.create_item().user_id(uid).name(name);
            if let Some(ref s) = due {
                let due_date = parse_date(s).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                req = req.due_date(due_date);
            }
            if let Some(p) = parent {
                req = req.parent_item_id(p);
            }
            if let Some(r) = recurrence {
                req = req.recurrence(r);
            }
            if let Some(o) = due_offset_days {
                req = req.due_offset_days(o);
            }
            let out = unwrap_or_exit(req.send().await, "add item");
            println!("created item {}", out.item_id());
        }
        ItemsCommand::Done { item_id } => {
            let uid = require_user(user_id);
            let item = unwrap_or_exit(
                client
                    .get_item()
                    .user_id(&uid)
                    .item_id(&item_id)
                    .send()
                    .await,
                "get item",
            );
            let mut req = client
                .update_item()
                .user_id(uid)
                .item_id(&item_id)
                .name(item.name())
                .complete(true)
                .due_date(item.due_date().cloned().unwrap());
            if let Some(r) = item.recurrence() {
                req = req.recurrence(r);
            }
            if let Some(b) = item.recurrence_basis() {
                req = req.recurrence_basis(b);
            }
            if let Some(t) = item.has_due_time() {
                req = req.has_due_time(t);
            }
            if let Some(p) = item.parent_item_id() {
                req = req.parent_item_id(p);
            }
            if let Some(o) = item.due_offset_days() {
                req = req.due_offset_days(o);
            }
            unwrap_or_exit(req.send().await, "mark done");
            println!("marked {item_id} complete");
        }
        ItemsCommand::Assigned => {
            let uid = require_user(user_id);
            let out = unwrap_or_exit(
                client.list_assigned_items().user_id(uid).send().await,
                "list assigned items",
            );
            if out.items().is_empty() {
                println!("(nothing assigned to you)");
                return;
            }
            println!(
                "{:<36}  {:<4}  {:<12}  {:<36}  {}",
                "ID", "DONE", "DUE", "OWNER", "NAME"
            );
            for i in out.items() {
                let done = if i.complete().unwrap_or(false) {
                    "✓"
                } else {
                    " "
                };
                let due = fmt_date_opt(i.due_date());
                println!(
                    "{:<36}  {:<4}  {:<12}  {:<36}  {}",
                    i.item_id(),
                    done,
                    due,
                    i.owner_user_id(),
                    i.name()
                );
            }
        }
        ItemsCommand::Delete { item_id } => {
            let uid = require_user(user_id);
            unwrap_or_exit(
                client
                    .delete_item()
                    .user_id(uid)
                    .item_id(&item_id)
                    .send()
                    .await,
                "delete item",
            );
            println!("deleted {item_id}");
        }
        ItemsCommand::Get { item_id } => {
            let uid = require_user(user_id);
            let item = unwrap_or_exit(
                client
                    .get_item()
                    .user_id(uid)
                    .item_id(&item_id)
                    .send()
                    .await,
                "get item",
            );
            println!("id:         {item_id}");
            println!("name:       {}", item.name());
            println!("complete:   {}", item.complete());
            println!("due:        {}", fmt_date_opt(item.due_date()));
            println!("recurrence: {}", item.recurrence().unwrap_or("-"));
            println!(
                "offset:     {}",
                item.due_offset_days()
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!("children:   {}", item.has_children().unwrap_or(false));
            println!("assigned:   {}", item.assigned_to_user_id().unwrap_or("-"));
        }
        ItemsCommand::Due { after, before } => {
            let uid = require_user(user_id);
            let mut req = client.list_items_due().user_id(uid);
            if let Some(a) = after {
                let ts = parse_date(&a).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                req = req.deadline_after(ts);
            }
            if let Some(b) = before {
                let ts = parse_date(&b).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                req = req.deadline_before(ts);
            }
            let out = unwrap_or_exit(req.send().await, "list due items");
            if out.items().is_empty() {
                println!("(nothing due)");
                return;
            }
            println!(
                "{:<36}  {:<4}  {:<12}  {:<24}  {}",
                "ID", "DONE", "DUE", "PARENT", "NAME"
            );
            for i in out.items() {
                let done = if i.complete().unwrap_or(false) {
                    "✓"
                } else {
                    " "
                };
                let due = fmt_date_opt(i.due_date());
                let parent = i.parent_name().unwrap_or("-");
                println!(
                    "{:<36}  {:<4}  {:<12}  {:<24}  {}",
                    i.item_id(),
                    done,
                    due,
                    parent,
                    i.name()
                );
            }
        }
    }
}
