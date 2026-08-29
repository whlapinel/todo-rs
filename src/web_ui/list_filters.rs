//! Shared list-filter vocabulary — see `docs/list-filtering-plan.md`. Stage 1 of that plan:
//! this module is deliberately screen-agnostic (not nested under `project_tasks/`) so the
//! other list screens that plan defers can reuse `ListFilterQuery`/`ListFilters` as-is later.
//! Nothing in `src/web_ui/project_tasks/` calls into this yet — that's Stage 2.

use crate::domain::item::Item;
use crate::service::item_series::ProjectOccurrence;
use chrono::{DateTime, Utc};

/// Raw query-string shape for a filterable list screen. Every field is single-valued — no
/// repeated-key/multi-select handling anywhere in this module, per the plan's 2026-08-23
/// revision (multi-select and the customizable-select experiment were dropped from scope).
#[derive(serde::Deserialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListFilterQuery {
    /// Rendered as a `<select>` (`"1"` = Complete and Incomplete, `"0"`/absent = Only
    /// Incomplete) — unlike the other single-select fields below, absence must also mean
    /// `false`, so `from_query` checks the value itself (`== "1"`) rather than presence.
    pub show_complete: Option<String>,
    /// `"me"` | `"unassigned"` | `"all"` | a specific user id | absent (defaults to `"me"`).
    pub assigned_to: Option<String>,
    /// `"overdue"` | `"none"` | absent (defaults to showing all).
    pub due_date: Option<String>,
    /// `"past"` | `"none"` | absent (defaults to showing all).
    pub schedule: Option<String>,
    /// `"no"` | absent (defaults to showing recurring items).
    pub recurring: Option<String>,
    /// `"1"`-`"4"` | absent (defaults to showing every priority, including unset). Any other
    /// value falls back to the default, same convention as `due_date`/`schedule`.
    pub priority: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignedToFilter {
    Me,
    Unassigned,
    All,
    User(String),
}

impl AssignedToFilter {
    /// Canonical string form — the same values `ListFilterQuery::assigned_to` accepts, and
    /// what a `<select>` control's `value`/`selected` comparison round-trips against. Stage 2
    /// of `docs/list-filtering-plan.md`.
    pub fn as_value(&self) -> String {
        match self {
            AssignedToFilter::Me => "me".to_string(),
            AssignedToFilter::Unassigned => "unassigned".to_string(),
            AssignedToFilter::All => "all".to_string(),
            AssignedToFilter::User(id) => id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueDateFilter {
    All,
    Overdue,
    None,
}

impl DueDateFilter {
    /// See `AssignedToFilter::as_value`'s identical rationale.
    pub fn as_value(&self) -> &'static str {
        match self {
            DueDateFilter::All => "all",
            DueDateFilter::Overdue => "overdue",
            DueDateFilter::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleFilter {
    All,
    Past,
    None,
}

impl ScheduleFilter {
    /// See `AssignedToFilter::as_value`'s identical rationale.
    pub fn as_value(&self) -> &'static str {
        match self {
            ScheduleFilter::All => "all",
            ScheduleFilter::Past => "past",
            ScheduleFilter::None => "none",
        }
    }
}

/// `priority` (root CLAUDE.md's Priority section, 1 = highest .. 4 = lowest) is filterable but
/// deliberately not a sort key — see `project_tasks::sort_key`'s doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityFilter {
    All,
    Exact(i32),
}

impl PriorityFilter {
    /// See `AssignedToFilter::as_value`'s identical rationale.
    pub fn as_value(&self) -> String {
        match self {
            PriorityFilter::All => String::new(),
            PriorityFilter::Exact(p) => p.to_string(),
        }
    }
}

/// Parsed/normalized form of `ListFilterQuery` — what every predicate and URL builder
/// actually works against.
#[derive(Debug, Clone, PartialEq)]
pub struct ListFilters {
    pub show_complete: bool,
    pub assigned_to: AssignedToFilter,
    pub due_date: DueDateFilter,
    pub schedule: ScheduleFilter,
    /// `true` = show recurring items (the default); `false` = hide any item whose
    /// `series_id` is set.
    pub recurring: bool,
    pub priority: PriorityFilter,
}

impl Default for ListFilters {
    fn default() -> Self {
        Self {
            show_complete: false,
            assigned_to: AssignedToFilter::Me,
            due_date: DueDateFilter::All,
            schedule: ScheduleFilter::All,
            recurring: true,
            priority: PriorityFilter::All,
        }
    }
}

impl ListFilters {
    pub fn from_query(q: ListFilterQuery) -> Self {
        Self {
            show_complete: q.show_complete.as_deref() == Some("1"),
            assigned_to: match q.assigned_to.as_deref() {
                None | Some("me") => AssignedToFilter::Me,
                Some("unassigned") => AssignedToFilter::Unassigned,
                Some("all") => AssignedToFilter::All,
                Some(id) => AssignedToFilter::User(id.to_string()),
            },
            due_date: match q.due_date.as_deref() {
                Some("overdue") => DueDateFilter::Overdue,
                Some("none") => DueDateFilter::None,
                _ => DueDateFilter::All,
            },
            schedule: match q.schedule.as_deref() {
                Some("past") => ScheduleFilter::Past,
                Some("none") => ScheduleFilter::None,
                _ => ScheduleFilter::All,
            },
            recurring: q.recurring.as_deref() != Some("no"),
            priority: match q.priority.as_deref().map(str::parse::<i32>) {
                Some(Ok(p)) if (1..=4).contains(&p) => PriorityFilter::Exact(p),
                _ => PriorityFilter::All,
            },
        }
    }

    /// Whether `item` should be visible under these filters. `is_team_project` gates
    /// `assigned_to` only — assignment is a team-project-only concept everywhere else in this
    /// codebase (`is_project_admin`, `active_member_options`, `TeamAssignment` itself), so on a
    /// personal project this filter never excludes anything, matching that same precedent
    /// rather than inventing a new one. `now` is a parameter (not read internally) so this
    /// stays unit-testable without a clock dependency — same convention as `Item::is_overdue`.
    pub fn matches(
        &self,
        item: &Item,
        requester_user_id: &str,
        is_team_project: bool,
        now: DateTime<Utc>,
    ) -> bool {
        if !self.show_complete && item.complete {
            return false;
        }
        if is_team_project {
            let matches_assignment = match &self.assigned_to {
                AssignedToFilter::All => true,
                AssignedToFilter::Me => {
                    item.assigned_to_user_id().as_deref() == Some(requester_user_id)
                }
                AssignedToFilter::Unassigned => item.assigned_to_user_id().is_none(),
                AssignedToFilter::User(id) => {
                    item.assigned_to_user_id().as_deref() == Some(id.as_str())
                }
            };
            if !matches_assignment {
                return false;
            }
        }
        let matches_due = match self.due_date {
            DueDateFilter::All => true,
            DueDateFilter::Overdue => item.is_overdue(now),
            DueDateFilter::None => item.due_date().is_none(),
        };
        if !matches_due {
            return false;
        }
        let matches_schedule = match self.schedule {
            ScheduleFilter::All => true,
            ScheduleFilter::Past => item.scheduled_date().is_some_and(|d| d < now),
            ScheduleFilter::None => item.scheduled_date().is_none(),
        };
        if !matches_schedule {
            return false;
        }
        if !self.recurring && item.series_id.is_some() {
            return false;
        }
        if let PriorityFilter::Exact(p) = self.priority {
            if item.priority() != Some(p) {
                return false;
            }
        }
        true
    }

    /// `matches`' counterpart for a still-virtual/skipped series occurrence — every caller
    /// that merges `ProjectOccurrence`s alongside real items into one `ListFilters`-filtered
    /// screen (currently just `project_tasks::list_task_rows_for_project`) must run occurrences
    /// through this too, or a filter that should exclude an item silently leaves its series'
    /// current occurrence showing anyway. `show_complete` has no occurrence counterpart — a
    /// virtual/skipped occurrence is never complete (there's no `complete` flag on
    /// `ProjectOccurrence` at all), and the caller already excludes `Materialized` occurrences
    /// before this runs (those are real items by then, filtered via `matches` instead) — so
    /// this only checks `assigned_to`/`due_date`/`schedule`/`priority`. `recurring` has no counterpart
    /// either: `filters.recurring == false` means the caller skips querying occurrences
    /// entirely (a virtual row is always a series occurrence), so nothing here would ever see
    /// one to reject. An occurrence carries exactly one date (`occurrence_date`), tagged by
    /// `is_due_date_basis` as either that series' due-date-equivalent or its
    /// scheduled-date-equivalent (mirrors how `ProjectTaskVirtualRow` already picks which
    /// icon/label to render) — so `due_date`/`schedule` are mutually exclusive on an
    /// occurrence, unlike on a real item which can carry both independently.
    pub fn matches_occurrence(
        &self,
        occ: &ProjectOccurrence,
        requester_user_id: &str,
        is_team_project: bool,
        now: DateTime<Utc>,
    ) -> bool {
        if is_team_project {
            let matches_assignment = match &self.assigned_to {
                AssignedToFilter::All => true,
                AssignedToFilter::Me => {
                    occ.assigned_to_user_id.as_deref() == Some(requester_user_id)
                }
                AssignedToFilter::Unassigned => occ.assigned_to_user_id.is_none(),
                AssignedToFilter::User(id) => {
                    occ.assigned_to_user_id.as_deref() == Some(id.as_str())
                }
            };
            if !matches_assignment {
                return false;
            }
        }
        let matches_due = match self.due_date {
            DueDateFilter::All => true,
            DueDateFilter::Overdue => occ.is_due_date_basis && occ.occurrence_date < now,
            DueDateFilter::None => !occ.is_due_date_basis,
        };
        if !matches_due {
            return false;
        }
        let matches_schedule = match self.schedule {
            ScheduleFilter::All => true,
            ScheduleFilter::Past => !occ.is_due_date_basis && occ.occurrence_date < now,
            ScheduleFilter::None => occ.is_due_date_basis,
        };
        if !matches_schedule {
            return false;
        }
        if let PriorityFilter::Exact(p) = self.priority {
            if occ.priority != Some(p) {
                return false;
            }
        }
        true
    }

    /// Non-default filter params as a `key=value&key2=value2` fragment — no leading `?`/`&`,
    /// empty when every filter is at its default. A caller combining this with an existing
    /// prefix (e.g. `view=tasks-list`) joins with `&` when non-empty; a caller with nothing
    /// else prepends `?` (or emits nothing if this is itself empty). Centralizes what today's
    /// `showComplete`-only threading hand-builds ad hoc per call site (see
    /// `ProjectTaskVirtualRow::from_occurrence`'s `list_query` in
    /// `src/web_ui/project_tasks/templates.rs`) — Stage 2 of `docs/list-filtering-plan.md`
    /// replaces that inline literal with this.
    pub fn query_string(&self) -> String {
        let mut parts = Vec::new();
        if self.show_complete {
            parts.push("showComplete=1".to_string());
        }
        match &self.assigned_to {
            AssignedToFilter::Me => {}
            AssignedToFilter::Unassigned => parts.push("assignedTo=unassigned".to_string()),
            AssignedToFilter::All => parts.push("assignedTo=all".to_string()),
            AssignedToFilter::User(id) => parts.push(format!("assignedTo={id}")),
        }
        match self.due_date {
            DueDateFilter::All => {}
            DueDateFilter::Overdue => parts.push("dueDate=overdue".to_string()),
            DueDateFilter::None => parts.push("dueDate=none".to_string()),
        }
        match self.schedule {
            ScheduleFilter::All => {}
            ScheduleFilter::Past => parts.push("schedule=past".to_string()),
            ScheduleFilter::None => parts.push("schedule=none".to_string()),
        }
        if !self.recurring {
            parts.push("recurring=no".to_string());
        }
        if let PriorityFilter::Exact(p) = self.priority {
            parts.push(format!("priority={p}"));
        }
        parts.join("&")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{ItemKind, ItemType, TeamAssignment};
    use crate::service::item_series::OccurrenceState;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn set_due_date(item: &mut Item, dt: DateTime<Utc>) {
        item.item_type
            .schedule_mut()
            .expect("has schedule")
            .due_date = Some(dt);
    }

    fn set_scheduled_date(item: &mut Item, dt: DateTime<Utc>) {
        item.item_type
            .schedule_mut()
            .expect("has schedule")
            .scheduled_date = Some(dt);
    }

    fn set_priority(item: &mut Item, priority: i32) {
        if let ItemType::Task { priority: p, .. } = &mut item.item_type {
            *p = Some(priority);
        } else {
            panic!("test item must be a Task");
        }
    }

    fn set_assigned_to(item: &mut Item, user_id: &str) {
        if let ItemType::Task {
            team_assignment, ..
        } = &mut item.item_type
        {
            *team_assignment = Some(TeamAssignment {
                assigned_to_user_id: Some(user_id.to_string()),
                points: None,
            });
        } else {
            panic!("test item must be a Task");
        }
    }

    fn task() -> Item {
        Item::new_user_item("u1", "Test task")
    }

    fn occurrence(occurrence_date: DateTime<Utc>, is_due_date_basis: bool) -> ProjectOccurrence {
        ProjectOccurrence {
            series_id: "s1".to_string(),
            series_name: "Series".to_string(),
            item_type: ItemKind::Task,
            event_type: None,
            occurrence_date,
            is_current: true,
            assigned_to_user_id: None,
            assigned_to_user_name: None,
            state: OccurrenceState::Virtual,
            is_due_date_basis,
            priority: None,
        }
    }

    const NOW: &str = "2026-08-23T12:00:00Z";

    #[test]
    fn default_filters_hide_complete_but_show_everything_else() {
        let filters = ListFilters::default();
        let mut item = task();
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
        item.complete = true;
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn show_complete_includes_completed_items() {
        let filters = ListFilters::from_query(ListFilterQuery {
            show_complete: Some("1".to_string()),
            ..Default::default()
        });
        let mut item = task();
        item.complete = true;
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn assigned_to_me_is_a_noop_on_a_personal_project() {
        let filters = ListFilters::default(); // AssignedToFilter::Me
        let item = task(); // unassigned
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn assigned_to_me_excludes_other_members_items_on_a_team_project() {
        let filters = ListFilters::default();
        let mut item = task();
        set_assigned_to(&mut item, "someone-else");
        assert!(!filters.matches(&item, "u1", true, utc(NOW)));
        set_assigned_to(&mut item, "u1");
        assert!(filters.matches(&item, "u1", true, utc(NOW)));
    }

    #[test]
    fn assigned_to_unassigned_matches_only_unassigned_items() {
        let filters = ListFilters::from_query(ListFilterQuery {
            assigned_to: Some("unassigned".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(filters.matches(&item, "u1", true, utc(NOW)));
        set_assigned_to(&mut item, "u1");
        assert!(!filters.matches(&item, "u1", true, utc(NOW)));
    }

    #[test]
    fn assigned_to_all_matches_regardless_of_assignment() {
        let filters = ListFilters::from_query(ListFilterQuery {
            assigned_to: Some("all".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(filters.matches(&item, "u1", true, utc(NOW)));
        set_assigned_to(&mut item, "someone-else");
        assert!(filters.matches(&item, "u1", true, utc(NOW)));
    }

    #[test]
    fn assigned_to_specific_user_matches_only_that_user() {
        let filters = ListFilters::from_query(ListFilterQuery {
            assigned_to: Some("bob".to_string()),
            ..Default::default()
        });
        let mut item = task();
        set_assigned_to(&mut item, "alice");
        assert!(!filters.matches(&item, "u1", true, utc(NOW)));
        set_assigned_to(&mut item, "bob");
        assert!(filters.matches(&item, "u1", true, utc(NOW)));
    }

    #[test]
    fn due_date_overdue_matches_only_overdue_items() {
        let filters = ListFilters::from_query(ListFilterQuery {
            due_date: Some("overdue".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
        set_due_date(&mut item, utc("2020-01-01T00:00:00Z"));
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn due_date_none_matches_only_undated_items() {
        let filters = ListFilters::from_query(ListFilterQuery {
            due_date: Some("none".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
        set_due_date(&mut item, utc("2030-01-01T00:00:00Z"));
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn schedule_past_matches_only_items_scheduled_before_now() {
        let filters = ListFilters::from_query(ListFilterQuery {
            schedule: Some("past".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
        set_scheduled_date(&mut item, utc("2030-01-01T00:00:00Z"));
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
        set_scheduled_date(&mut item, utc("2020-01-01T00:00:00Z"));
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn schedule_none_matches_only_unscheduled_items() {
        let filters = ListFilters::from_query(ListFilterQuery {
            schedule: Some("none".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
        set_scheduled_date(&mut item, utc("2020-01-01T00:00:00Z"));
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn recurring_no_excludes_series_linked_items() {
        let filters = ListFilters::from_query(ListFilterQuery {
            recurring: Some("no".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
        item.series_id = Some("series-1".to_string());
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn matches_occurrence_assigned_to_me_excludes_other_members_occurrences() {
        let filters = ListFilters::default(); // AssignedToFilter::Me
        let mut occ = occurrence(utc(NOW), true);
        occ.assigned_to_user_id = Some("someone-else".to_string());
        assert!(!filters.matches_occurrence(&occ, "u1", true, utc(NOW)));
        occ.assigned_to_user_id = Some("u1".to_string());
        assert!(filters.matches_occurrence(&occ, "u1", true, utc(NOW)));
    }

    #[test]
    fn matches_occurrence_assigned_to_is_a_noop_on_a_personal_project() {
        let filters = ListFilters::default(); // AssignedToFilter::Me
        let mut occ = occurrence(utc(NOW), true);
        occ.assigned_to_user_id = Some("someone-else".to_string());
        assert!(filters.matches_occurrence(&occ, "u1", false, utc(NOW)));
    }

    #[test]
    fn matches_occurrence_due_date_overdue_only_matches_due_basis_in_the_past() {
        let filters = ListFilters::from_query(ListFilterQuery {
            due_date: Some("overdue".to_string()),
            ..Default::default()
        });
        assert!(!filters.matches_occurrence(&occurrence(utc(NOW), true), "u1", false, utc(NOW)));
        assert!(filters.matches_occurrence(
            &occurrence(utc("2020-01-01T00:00:00Z"), true),
            "u1",
            false,
            utc(NOW)
        ));
        // A scheduled-basis occurrence has no due date at all, so it can never be "overdue".
        assert!(!filters.matches_occurrence(
            &occurrence(utc("2020-01-01T00:00:00Z"), false),
            "u1",
            false,
            utc(NOW)
        ));
    }

    #[test]
    fn matches_occurrence_due_date_none_only_matches_scheduled_basis() {
        let filters = ListFilters::from_query(ListFilterQuery {
            due_date: Some("none".to_string()),
            ..Default::default()
        });
        assert!(!filters.matches_occurrence(&occurrence(utc(NOW), true), "u1", false, utc(NOW)));
        assert!(filters.matches_occurrence(&occurrence(utc(NOW), false), "u1", false, utc(NOW)));
    }

    #[test]
    fn matches_occurrence_schedule_past_only_matches_scheduled_basis_in_the_past() {
        let filters = ListFilters::from_query(ListFilterQuery {
            schedule: Some("past".to_string()),
            ..Default::default()
        });
        assert!(!filters.matches_occurrence(&occurrence(utc(NOW), false), "u1", false, utc(NOW)));
        assert!(filters.matches_occurrence(
            &occurrence(utc("2020-01-01T00:00:00Z"), false),
            "u1",
            false,
            utc(NOW)
        ));
        // A due-basis occurrence has no scheduled date at all.
        assert!(!filters.matches_occurrence(
            &occurrence(utc("2020-01-01T00:00:00Z"), true),
            "u1",
            false,
            utc(NOW)
        ));
    }

    #[test]
    fn matches_occurrence_schedule_none_only_matches_due_basis() {
        let filters = ListFilters::from_query(ListFilterQuery {
            schedule: Some("none".to_string()),
            ..Default::default()
        });
        assert!(filters.matches_occurrence(&occurrence(utc(NOW), true), "u1", false, utc(NOW)));
        assert!(!filters.matches_occurrence(&occurrence(utc(NOW), false), "u1", false, utc(NOW)));
    }

    #[test]
    fn query_string_empty_at_defaults() {
        assert_eq!(ListFilters::default().query_string(), "");
    }

    #[test]
    fn as_value_round_trips_through_from_query() {
        let filters = ListFilters::from_query(ListFilterQuery {
            assigned_to: Some("bob".to_string()),
            due_date: Some("overdue".to_string()),
            schedule: Some("past".to_string()),
            ..Default::default()
        });
        assert_eq!(filters.assigned_to.as_value(), "bob");
        assert_eq!(filters.due_date.as_value(), "overdue");
        assert_eq!(filters.schedule.as_value(), "past");
        assert_eq!(ListFilters::default().assigned_to.as_value(), "me");
    }

    #[test]
    fn query_string_round_trips_non_default_values() {
        let filters = ListFilters::from_query(ListFilterQuery {
            show_complete: Some("1".to_string()),
            assigned_to: Some("bob".to_string()),
            due_date: Some("overdue".to_string()),
            schedule: Some("past".to_string()),
            recurring: Some("no".to_string()),
            priority: Some("2".to_string()),
        });
        assert_eq!(
            filters.query_string(),
            "showComplete=1&assignedTo=bob&dueDate=overdue&schedule=past&recurring=no&priority=2"
        );
    }

    #[test]
    fn priority_exact_matches_only_that_priority() {
        let filters = ListFilters::from_query(ListFilterQuery {
            priority: Some("2".to_string()),
            ..Default::default()
        });
        let mut item = task();
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
        set_priority(&mut item, 2);
        assert!(filters.matches(&item, "u1", false, utc(NOW)));
        set_priority(&mut item, 1);
        assert!(!filters.matches(&item, "u1", false, utc(NOW)));
    }

    #[test]
    fn priority_out_of_range_or_non_numeric_falls_back_to_all() {
        let filters = ListFilters::from_query(ListFilterQuery {
            priority: Some("5".to_string()),
            ..Default::default()
        });
        assert_eq!(filters.priority, PriorityFilter::All);
        let filters = ListFilters::from_query(ListFilterQuery {
            priority: Some("nope".to_string()),
            ..Default::default()
        });
        assert_eq!(filters.priority, PriorityFilter::All);
    }

    #[test]
    fn matches_occurrence_priority_exact_matches_only_that_priority() {
        let filters = ListFilters::from_query(ListFilterQuery {
            priority: Some("3".to_string()),
            ..Default::default()
        });
        let mut occ = occurrence(utc(NOW), true);
        occ.priority = Some(1);
        assert!(!filters.matches_occurrence(&occ, "u1", false, utc(NOW)));
        occ.priority = Some(3);
        assert!(filters.matches_occurrence(&occ, "u1", false, utc(NOW)));
    }
}
