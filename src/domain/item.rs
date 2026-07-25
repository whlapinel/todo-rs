use super::recurrence;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub user_id: Option<String>,
    pub team_id: Option<String>,
    pub parent_item_id: Option<String>,
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: bool,
    pub has_tasks: bool,
    pub has_children: bool,
    pub is_template: bool,
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
    /// (fresh id, incomplete, deadline advanced past `now`). Callers are
    /// responsible for persisting the returned item, deleting `self`, and
    /// carrying over any child items onto the new id.
    pub fn next_recurrence(&self, now: DateTime<Utc>, tz_offset_minutes: i32) -> Option<Self> {
        if !self.complete {
            return None;
        }
        let pattern = self.recurrence.as_ref()?;
        let rule = recurrence::parse(pattern).ok()?;
        let reference = if self.recurrence_basis.as_deref() == Some("COMPLETION_DATE") {
            now
        } else {
            self.due_date.unwrap_or(now)
        };
        let mut next = self.clone();
        next.id = String::new();
        next.complete = false;
        next.due_date = Some(recurrence::next_date(&rule, reference, tz_offset_minutes));
        next.has_due_time = rule.time_override.is_some() || self.has_due_time;
        Some(next)
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

        let next = item.next_recurrence(Utc::now(), 0).expect("should recur");

        assert!(next.id.is_empty());
        assert!(!next.complete);
        assert_eq!(next.user_id, item.user_id);
        assert_eq!(next.name, item.name);
        assert_eq!(next.recurrence, item.recurrence);
        assert!(next.due_date.unwrap() > item.due_date.unwrap());
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
