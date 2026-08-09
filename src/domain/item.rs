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
    /// A bare checkable name with no scheduling machinery — no due date, scheduled
    /// window, recurrence, or due-offset. Enforced by `Item::validate`.
    Simple,
}

impl ItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemType::Task => "TASK",
            ItemType::Event => "EVENT",
            ItemType::Template => "TEMPLATE",
            ItemType::Simple => "SIMPLE",
        }
    }

    /// Display label for the "Kind" badge shown in `items`/`team_items` detail views.
    pub fn label(&self) -> &'static str {
        match self {
            ItemType::Task => "Task",
            ItemType::Event => "Event",
            ItemType::Template => "Template",
            ItemType::Simple => "Simple",
        }
    }

    /// Color passed to `macros::badge` for the "Kind" badge.
    pub fn badge_color(&self) -> &'static str {
        match self {
            ItemType::Event => "indigo",
            _ => "gray",
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
            "SIMPLE" => Ok(ItemType::Simple),
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
            ..Self::default()
        }
    }

    pub fn new_team_item(team_id: &str, name: &str) -> Self {
        Self {
            team_id: Some(team_id.to_string()),
            name: name.to_string(),
            ..Self::default()
        }
    }

    /// Thin `item_type`-setting sugar over `new_user_item`/`new_team_item` — kept
    /// separate from the owner constructors above since "who owns this" and "what kind
    /// is this" are independent axes. Note these only control an item's *initial*
    /// defaults: `create_item`/`update_item` overlay every field from caller params
    /// unconditionally afterward, so a caller can still hand a `Simple` item a due date
    /// this way — `validate()` below is the actual enforcement point, called once the
    /// full item is assembled.
    pub fn new_task(user_id: &str, name: &str) -> Self {
        Self::new_user_item(user_id, name)
    }

    pub fn new_event(user_id: &str, name: &str) -> Self {
        Self {
            item_type: ItemType::Event,
            ..Self::new_user_item(user_id, name)
        }
    }

    pub fn new_simple(user_id: &str, name: &str) -> Self {
        Self {
            item_type: ItemType::Simple,
            ..Self::new_user_item(user_id, name)
        }
    }

    /// Enforces the one cross-field invariant `ItemType` currently implies: a `Simple`
    /// item is defined by *not* having scheduling machinery, so it can't carry a due
    /// date, scheduled window, recurrence, or due-offset. Called by the service layer
    /// once an `Item` is fully assembled, right before it's persisted — a constructor
    /// alone can't guarantee this, since callers overlay fields onto a freshly
    /// constructed item afterward (see the constructors above).
    pub fn validate(&self) -> Result<(), String> {
        // event_type is the auto-trigger match key (see create_item/create_team_item):
        // only an Event actually "occurs" in a way that can fire a matching template, so
        // it's the one type allowed to carry it. Template is also allowed here (it's the
        // trigger's match *target*, set today via create_template — a path that never
        // calls validate() — but permitting it too means this check stays correct even if
        // that changes later) rather than carving out an exemption tied to today's call
        // graph.
        if self.event_type.is_some()
            && self.item_type != ItemType::Event
            && self.item_type != ItemType::Template
        {
            return Err("event_type can only be set on event or template items".to_string());
        }
        if self.item_type != ItemType::Simple {
            return Ok(());
        }
        if self.due_date.is_some()
            || self.scheduled_date.is_some()
            || self.scheduled_end_date.is_some()
        {
            return Err("simple items can't have a due date or scheduled window".to_string());
        }
        if self.recurrence.is_some() {
            return Err("simple items can't recur".to_string());
        }
        if self.due_offset_days.is_some() {
            return Err("simple items can't have a due offset".to_string());
        }
        Ok(())
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

    /// An incomplete item whose `due_date` has passed. `scheduled_date` never factors in here
    /// — "overdue" is a deadline concept, not a planning-window one (see the Scheduled
    /// start/end section of CLAUDE.md for that distinction).
    pub fn is_overdue(&self, now: DateTime<Utc>) -> bool {
        !self.complete && self.due_date.is_some_and(|d| d < now)
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
        assert_eq!(item.item_type, ItemType::Task);
    }

    #[test]
    fn new_team_item_sets_team_id_and_name() {
        let item = Item::new_team_item("t1", "Deploy server");
        assert_eq!(item.team_id, Some("t1".to_string()));
        assert_eq!(item.name, "Deploy server");
        assert!(item.user_id.is_none());
        assert_eq!(item.item_type, ItemType::Task);
    }

    #[test]
    fn new_simple_sets_simple_item_type() {
        let item = Item::new_simple("u1", "Milk");
        assert_eq!(item.item_type, ItemType::Simple);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_simple_item_with_due_date() {
        let mut item = Item::new_simple("u1", "Milk");
        item.due_date = Some(Utc::now());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_rejects_simple_item_with_scheduled_date() {
        let mut item = Item::new_simple("u1", "Milk");
        item.scheduled_date = Some(Utc::now());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_rejects_simple_item_with_scheduled_end_date() {
        let mut item = Item::new_simple("u1", "Milk");
        item.scheduled_end_date = Some(Utc::now());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_rejects_simple_item_with_recurrence() {
        let mut item = Item::new_simple("u1", "Milk");
        item.recurrence = Some("every day".to_string());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_rejects_simple_item_with_due_offset_days() {
        let mut item = Item::new_simple("u1", "Milk");
        item.due_offset_days = Some(1);
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_task_with_all_scheduling_fields() {
        let mut item = Item::new_task("u1", "Water plants");
        item.due_date = Some(Utc::now());
        item.scheduled_date = Some(Utc::now());
        item.scheduled_end_date = Some(Utc::now());
        item.recurrence = Some("every day".to_string());
        item.due_offset_days = Some(1);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_allows_event_with_all_scheduling_fields() {
        let mut item = Item::new_event("u1", "Team offsite");
        item.due_date = Some(Utc::now());
        item.scheduled_date = Some(Utc::now());
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_allows_event_with_event_type() {
        let mut item = Item::new_event("u1", "Storm watch");
        item.event_type = Some("rain".to_string());
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_task_with_event_type() {
        let mut item = Item::new_task("u1", "Water plants");
        item.event_type = Some("rain".to_string());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_rejects_simple_item_with_event_type() {
        let mut item = Item::new_simple("u1", "Milk");
        item.event_type = Some("rain".to_string());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_template_with_event_type() {
        let mut item = Item::new_user_item("u1", "Rain prep");
        item.item_type = ItemType::Template;
        item.event_type = Some("rain".to_string());
        assert!(item.validate().is_ok());
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

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn is_overdue_true_when_incomplete_and_due_date_past() {
        let mut item = Item::new_user_item("u1", "Pay rent");
        item.due_date = Some(utc("2020-01-01T00:00:00Z"));
        assert!(item.is_overdue(utc("2026-01-01T00:00:00Z")));
    }

    #[test]
    fn is_overdue_false_when_due_date_in_future() {
        let mut item = Item::new_user_item("u1", "Pay rent");
        item.due_date = Some(utc("2030-01-01T00:00:00Z"));
        assert!(!item.is_overdue(utc("2026-01-01T00:00:00Z")));
    }

    #[test]
    fn is_overdue_false_when_no_due_date() {
        let item = Item::new_user_item("u1", "Pay rent");
        assert!(!item.is_overdue(utc("2026-01-01T00:00:00Z")));
    }

    #[test]
    fn is_overdue_false_when_complete_even_if_due_date_past() {
        let mut item = Item::new_user_item("u1", "Pay rent");
        item.due_date = Some(utc("2020-01-01T00:00:00Z"));
        item.complete = true;
        assert!(!item.is_overdue(utc("2026-01-01T00:00:00Z")));
    }
}
