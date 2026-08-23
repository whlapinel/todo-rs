//! Shared list-filter vocabulary — see `docs/list-filtering-plan.md`. Stage 1 of that plan:
//! this module is deliberately screen-agnostic (not nested under `project_tasks/`) so the
//! other list screens that plan defers can reuse `ListFilterQuery`/`ListFilters` as-is later.
//! Nothing in `src/web_ui/project_tasks/` calls into this yet — that's Stage 2.

use crate::domain::item::Item;
use chrono::{DateTime, Utc};

/// Raw query-string shape for a filterable list screen. Every field is single-valued — no
/// repeated-key/multi-select handling anywhere in this module, per the plan's 2026-08-23
/// revision (multi-select and the customizable-select experiment were dropped from scope).
#[derive(serde::Deserialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListFilterQuery {
    /// Existing convention (carried over from the pre-existing `showComplete` param):
    /// presence of the key at all means `true`, regardless of value.
    pub show_complete: Option<String>,
    /// `"me"` | `"unassigned"` | `"all"` | a specific user id | absent (defaults to `"me"`).
    pub assigned_to: Option<String>,
    /// `"overdue"` | `"none"` | absent (defaults to showing all).
    pub due_date: Option<String>,
    /// `"past"` | `"none"` | absent (defaults to showing all).
    pub schedule: Option<String>,
    /// `"no"` | absent (defaults to showing recurring items).
    pub recurring: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignedToFilter {
    Me,
    Unassigned,
    All,
    User(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueDateFilter {
    All,
    Overdue,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleFilter {
    All,
    Past,
    None,
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
}

impl Default for ListFilters {
    fn default() -> Self {
        Self {
            show_complete: false,
            assigned_to: AssignedToFilter::Me,
            due_date: DueDateFilter::All,
            schedule: ScheduleFilter::All,
            recurring: true,
        }
    }
}

impl ListFilters {
    pub fn from_query(q: ListFilterQuery) -> Self {
        Self {
            show_complete: q.show_complete.is_some(),
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
        parts.join("&")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{ItemType, TeamAssignment};

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
    fn query_string_empty_at_defaults() {
        assert_eq!(ListFilters::default().query_string(), "");
    }

    #[test]
    fn query_string_round_trips_non_default_values() {
        let filters = ListFilters::from_query(ListFilterQuery {
            show_complete: Some("1".to_string()),
            assigned_to: Some("bob".to_string()),
            due_date: Some("overdue".to_string()),
            schedule: Some("past".to_string()),
            recurring: Some("no".to_string()),
        });
        assert_eq!(
            filters.query_string(),
            "showComplete=1&assignedTo=bob&dueDate=overdue&schedule=past&recurring=no"
        );
    }
}
