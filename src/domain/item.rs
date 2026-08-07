use super::recurrence;
use chrono::{DateTime, Duration, Utc};
use std::fmt;
use std::str::FromStr;

/// What kind of thing an `Item` row represents. Replaces what used to be an
/// `is_template: bool` alongside a task/event distinction as two independent
/// flags — those were both answering "what kind of row is this," so they're
/// one field now instead of two that can drift out of sync.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ItemType {
    #[default]
    Task,
    Event,
    Template,
}

impl ItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemType::Task => "TASK",
            ItemType::Event => "EVENT",
            ItemType::Template => "TEMPLATE",
        }
    }
}

impl fmt::Display for ItemType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ItemType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "TASK" => Ok(ItemType::Task),
            "EVENT" => Ok(ItemType::Event),
            "TEMPLATE" => Ok(ItemType::Template),
            other => Err(format!("unknown item type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub user_id: Option<String>,
    pub team_id: Option<String>,
    pub parent_item_id: Option<String>,
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: bool,
    pub has_scheduled_time: bool,
    pub has_end_time: bool,
    pub has_tasks: bool,
    pub has_children: bool,
    pub item_type: ItemType,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
}

impl Item {
    pub fn new_user_item(user_id: &str, name: &str) -> Self {
        Self {
            user_id: Some(user_id.to_string()),
            name: name.to_string(),
            has_tasks: true,
            ..Self::default()
        }
    }

    pub fn new_team_item(team_id: &str, name: &str) -> Self {
        Self {
            team_id: Some(team_id.to_string()),
            name: name.to_string(),
            has_tasks: true,
            ..Self::default()
        }
    }

    /// If this item is complete and recurring, returns the next occurrence
    /// (fresh id, incomplete, primary date advanced past `now`) along with the
    /// freshly-advanced date itself (the "anchor" child items should measure
    /// their own `due_offset_days` from). Callers are responsible for
    /// persisting the returned item, deleting `self`, and carrying over any
    /// child items onto the new id using the returned anchor.
    ///
    /// `recurrence_basis` picks both the *reference* used to compute the next
    /// date and the *field* that date gets written into: legacy `"DUE_DATE"`
    /// (the default when unset) reads and writes `due_date`, unchanged from
    /// this method's original behavior. `"COMPLETION_DATE"` and
    /// `"SCHEDULED_DATE"` both write the advanced date into `scheduled_date`
    /// instead — `scheduled_date` is the primary field for anything not
    /// explicitly pinned to the legacy due-date basis. `scheduled_end_date`,
    /// if set, shifts by the same delta so the window's length survives the
    /// recurrence. Whichever field isn't the active basis's output simply
    /// rides along unchanged via `self.clone()`.
    pub fn next_recurrence(&self, now: DateTime<Utc>, tz_offset_minutes: i32) -> Option<(Self, DateTime<Utc>)> {
        if !self.complete {
            return None;
        }
        let pattern = self.recurrence.as_ref()?;
        let rule = recurrence::parse(pattern).ok()?;
        let basis = self.recurrence_basis.as_deref().unwrap_or("DUE_DATE");

        let mut next = self.clone();
        next.id = String::new();
        next.complete = false;

        let anchor = if basis == "DUE_DATE" {
            let reference = self.due_date.unwrap_or(now);
            let next_date = recurrence::next_date(&rule, reference, tz_offset_minutes);
            next.due_date = Some(next_date);
            next.has_due_time = rule.time_override.is_some() || self.has_due_time;
            next_date
        } else {
            let reference = if basis == "SCHEDULED_DATE" {
                self.scheduled_date.unwrap_or(now)
            } else {
                now
            };
            let next_date = recurrence::next_date(&rule, reference, tz_offset_minutes);
            let delta = next_date - reference;
            next.scheduled_end_date = self.scheduled_end_date.map(|e| e + delta);
            next.scheduled_date = Some(next_date);
            next.has_scheduled_time = rule.time_override.is_some() || self.has_scheduled_time;
            next_date
        };
        Some((next, anchor))
    }

    /// Deadline for this item as a child, measured from the top-level ancestor's
    /// `root_deadline` plus this item's own `due_offset_days`. `None` if no offset
    /// is set — the item's own (pre-existing) deadline is never consulted here,
    /// since children can't carry independent recurrence and their prior deadline
    /// has no bearing on the next one.
    pub fn deadline_from_offset(
        &self,
        root_deadline: DateTime<Utc>,
        tz_offset_minutes: i32,
    ) -> Option<DateTime<Utc>> {
        self.due_offset_days.map(|days| {
            recurrence::apply_end_of_day(
                root_deadline + Duration::days(days as i64),
                tz_offset_minutes,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_user_item_sets_user_id_and_name() {
        let item = Item::new_user_item("u1", "Buy milk");
        assert_eq!(item.user_id, Some("u1".to_string()));
        assert_eq!(item.name, "Buy milk");
        assert!(item.team_id.is_none());
        assert!(item.has_tasks);
    }

    #[test]
    fn new_team_item_sets_team_id_and_name() {
        let item = Item::new_team_item("t1", "Deploy server");
        assert_eq!(item.team_id, Some("t1".to_string()));
        assert_eq!(item.name, "Deploy server");
        assert!(item.user_id.is_none());
        assert!(item.has_tasks);
    }

    #[test]
    fn new_items_are_not_complete() {
        let user_item = Item::new_user_item("u1", "task");
        let team_item = Item::new_team_item("t1", "task");
        assert!(!user_item.complete);
        assert!(!team_item.complete);
    }

    #[test]
    fn next_recurrence_none_when_not_complete() {
        let mut item = Item::new_user_item("u1", "Water plants");
        item.recurrence = Some("every 3 days".to_string());
        assert!(item.next_recurrence(Utc::now(), 0).is_none());
    }

    #[test]
    fn next_recurrence_none_when_no_recurrence() {
        let mut item = Item::new_user_item("u1", "One-off task");
        item.complete = true;
        assert!(item.next_recurrence(Utc::now(), 0).is_none());
    }

    #[test]
    fn next_recurrence_produces_fresh_incomplete_item() {
        let mut item = Item::new_user_item("u1", "Water plants");
        item.id = "old-id".to_string();
        item.complete = true;
        item.recurrence = Some("every 3 days".to_string());
        item.due_date = Some(Utc::now());

        let (next, anchor) = item.next_recurrence(Utc::now(), 0).expect("should recur");

        assert!(next.id.is_empty());
        assert!(!next.complete);
        assert_eq!(next.user_id, item.user_id);
        assert_eq!(next.name, item.name);
        assert_eq!(next.recurrence, item.recurrence);
        assert!(next.due_date.unwrap() > item.due_date.unwrap());
        assert_eq!(anchor, next.due_date.unwrap());
    }

    #[test]
    fn next_recurrence_with_scheduled_basis_advances_scheduled_date_not_due_date() {
        let mut item = Item::new_user_item("u1", "Water plants");
        item.complete = true;
        item.recurrence = Some("every 3 days".to_string());
        item.recurrence_basis = Some("SCHEDULED_DATE".to_string());
        item.scheduled_date = Some(Utc::now());
        item.due_date = Some(
            DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let (next, anchor) = item.next_recurrence(Utc::now(), 0).expect("should recur");

        assert!(next.scheduled_date.unwrap() > item.scheduled_date.unwrap());
        assert_eq!(next.due_date, item.due_date);
        assert_eq!(anchor, next.scheduled_date.unwrap());
    }

    #[test]
    fn next_recurrence_with_scheduled_basis_preserves_window_length() {
        let mut item = Item::new_user_item("u1", "Work session");
        item.complete = true;
        item.recurrence = Some("every week".to_string());
        item.recurrence_basis = Some("SCHEDULED_DATE".to_string());
        let start = Utc::now();
        item.scheduled_date = Some(start);
        item.scheduled_end_date = Some(start + Duration::hours(2));

        let (next, _anchor) = item.next_recurrence(Utc::now(), 0).expect("should recur");

        let original_gap = item.scheduled_end_date.unwrap() - item.scheduled_date.unwrap();
        let new_gap = next.scheduled_end_date.unwrap() - next.scheduled_date.unwrap();
        assert_eq!(original_gap, new_gap);
    }

    #[test]
    fn next_recurrence_with_completion_basis_writes_scheduled_date() {
        let mut item = Item::new_user_item("u1", "Take out trash");
        item.complete = true;
        item.recurrence = Some("every 3 days".to_string());
        item.recurrence_basis = Some("COMPLETION_DATE".to_string());
        item.due_date = Some(
            DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let now = Utc::now();
        let (next, anchor) = item.next_recurrence(now, 0).expect("should recur");

        assert!(next.scheduled_date.is_some());
        assert!(next.scheduled_date.unwrap() > now);
        assert_eq!(next.due_date, item.due_date);
        assert_eq!(anchor, next.scheduled_date.unwrap());
    }

    #[test]
    fn deadline_from_offset_none_when_no_offset() {
        let child = Item::new_user_item("u1", "Check inbox");
        assert!(child.deadline_from_offset(Utc::now(), 0).is_none());
    }

    #[test]
    fn deadline_from_offset_adds_days_to_root_deadline() {
        let mut child = Item::new_user_item("u1", "Prep agenda");
        child.due_offset_days = Some(-2);
        let root_deadline = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let deadline = child.deadline_from_offset(root_deadline, 0).unwrap();

        assert_eq!(
            deadline.date_naive(),
            (root_deadline - Duration::days(2)).date_naive()
        );
    }

    #[test]
    fn deadline_from_offset_ignores_prior_deadline() {
        let mut child = Item::new_user_item("u1", "Check inbox");
        child.due_offset_days = Some(1);
        child.due_date = Some(
            DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let root_deadline = DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let deadline = child.deadline_from_offset(root_deadline, 0).unwrap();

        assert_eq!(
            deadline.date_naive(),
            (root_deadline + Duration::days(1)).date_naive()
        );
    }
}
