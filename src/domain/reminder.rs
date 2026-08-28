use chrono::{DateTime, Utc};
use std::fmt;
use std::str::FromStr;

/// What point in an item's life this reminder fires at — see `sync_item_reminders`
/// (`src/service/reminders.rs`) for how each variant maps onto an `Item`'s own date
/// fields. A real closed enum (mirroring `TeamRole`'s precedent), not a raw string like
/// `event_type` — a small fixed set with actual match-arm behavior on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderKind {
    Due,
    ScheduledStart,
    ScheduledEnd,
}

impl ReminderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReminderKind::Due => "DUE",
            ReminderKind::ScheduledStart => "SCHEDULED_START",
            ReminderKind::ScheduledEnd => "SCHEDULED_END",
        }
    }
}

impl fmt::Display for ReminderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReminderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DUE" => Ok(ReminderKind::Due),
            "SCHEDULED_START" => Ok(ReminderKind::ScheduledStart),
            "SCHEDULED_END" => Ok(ReminderKind::ScheduledEnd),
            other => Err(format!("unknown reminder kind: {other}")),
        }
    }
}

/// A single "notify `user_id` at `remind_at`" row, one per (item, kind). `source` is a
/// plain string rather than its own enum (matching `event_type`'s open-string precedent)
/// — this pass only ever writes `"AUTO"`; a future custom-reminder mutation UI would add
/// `"CUSTOM"` rows, distinguished by this column, without needing a schema change.
/// `sent_at` is the in-app dismiss marker (`service::reminders`/`web_ui::notifications`).
/// `push_sent_at` is a second, independent "delivered" marker for the push channel
/// (`service::push::sweep_due_reminders`, `docs/push-notifications-plan.md`) — kept separate
/// from `sent_at` so dismissing in-app doesn't retroactively suppress a push that already
/// fired, and vice versa; the only coupling is that the push sweep also skips anything
/// already dismissed in-app (see `ReminderRepo::list_due_for_push`).
#[derive(Debug, Clone, PartialEq)]
pub struct Reminder {
    pub id: String,
    pub item_id: String,
    pub project_id: String,
    pub user_id: String,
    pub kind: ReminderKind,
    pub source: String,
    pub remind_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub push_sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
