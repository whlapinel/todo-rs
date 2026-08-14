use super::recurrence;
use chrono::{DateTime, Duration, Utc};
use std::fmt;
use std::str::FromStr;

/// Plain, `Copy`, data-free discriminant for "what kind of thing is this" — used
/// everywhere only the *kind* is needed: DTOs (`CreateItemParams`/`UpdateItemParams`),
/// SDK conversion (`to_domain_item_type`/`to_sdk_item_type`), web_ui type-guards, badge
/// labels, the CLI's `--item-type` flag. See `ItemType` below for the data-carrying
/// counterpart that actually lives on `Item`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ItemKind {
    #[default]
    Task,
    Event,
    Template,
    Simple,
}

impl ItemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemKind::Task => "TASK",
            ItemKind::Event => "EVENT",
            ItemKind::Template => "TEMPLATE",
            ItemKind::Simple => "SIMPLE",
        }
    }

    /// Display label for the "Kind" badge shown in `items`/`team_items` detail views.
    pub fn label(&self) -> &'static str {
        match self {
            ItemKind::Task => "Task",
            ItemKind::Event => "Event",
            ItemKind::Template => "Template",
            ItemKind::Simple => "Simple",
        }
    }

    /// Color passed to `macros::badge` for the "Kind" badge.
    pub fn badge_color(&self) -> &'static str {
        match self {
            ItemKind::Event => "indigo",
            _ => "gray",
        }
    }
}

impl fmt::Display for ItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ItemKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "TASK" => Ok(ItemKind::Task),
            "EVENT" => Ok(ItemKind::Event),
            "TEMPLATE" => Ok(ItemKind::Template),
            "SIMPLE" => Ok(ItemKind::Simple),
            other => Err(format!("unknown item type: {other}")),
        }
    }
}

/// The due/scheduled-window date fields shared by every type except `Simple`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Schedule {
    pub due_date: Option<DateTime<Utc>>,
    pub has_due_time: bool,
    pub scheduled_date: Option<DateTime<Utc>>,
    pub has_scheduled_time: bool,
    pub scheduled_end_date: Option<DateTime<Utc>>,
    pub has_end_time: bool,
}

/// Recurrence pattern plus the child-offset field, shared by every type except `Simple`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recurrence {
    pub pattern: Option<String>,
    pub basis: Option<String>,
    pub due_offset_days: Option<i32>,
}

/// Team-item-only, `Task`-only, top-level-only. Admin-only enforcement of *who* may
/// set `points` lives in the service layer (`create_team_item`/`update_team_item`),
/// not here; the top-level-only restriction is enforced by `Item::validate` below.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamAssignment {
    pub assigned_to_user_id: Option<String>,
    pub points: Option<i32>,
}

/// What kind of thing an `Item` row represents, and the data that only makes sense
/// for that kind. Only `Task` can carry a `TeamAssignment` (points/assignment);
/// only `Event`/`Template` can carry `event_type` (see `create_item`/`create_team_item`
/// for the auto-trigger match this key drives); only `Simple` has none of the
/// scheduling machinery. Putting the data inside the variant instead of alongside a
/// flat `ItemKind` field means these are no longer two things that can drift out of
/// sync (a Task literally has nowhere to put an Event's `event_type`) — see the
/// "keep me honest" discussion in issues.md history for why the flat-Option shape
/// this replaced didn't actually make these invariants type-safe.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemType {
    Task {
        schedule: Schedule,
        recurrence: Recurrence,
        team_assignment: Option<TeamAssignment>,
        /// The Event this task tracks, if it was auto-copied from a matching
        /// template or manually linked from the event's own "Linked tasks"
        /// form — a reference, not structural nesting, so the task stays
        /// top-level (assignable/pointable) while still being offset-driven
        /// off the event's own date. Mutually exclusive with `parent_item_id`
        /// (see `Item::validate`) and only settable on `Task` — only a Task
        /// has anywhere to put it.
        source_event_id: Option<String>,
    },
    Event {
        schedule: Schedule,
        recurrence: Recurrence,
        event_type: Option<String>,
    },
    Template {
        schedule: Schedule,
        recurrence: Recurrence,
        event_type: Option<String>,
    },
    /// A bare checkable name with no scheduling machinery — no due date, scheduled
    /// window, recurrence, due-offset, event_type, or team assignment.
    Simple,
}

impl Default for ItemType {
    fn default() -> Self {
        ItemType::Task {
            schedule: Schedule::default(),
            recurrence: Recurrence::default(),
            team_assignment: None,
            source_event_id: None,
        }
    }
}

impl ItemType {
    pub fn kind(&self) -> ItemKind {
        match self {
            ItemType::Task { .. } => ItemKind::Task,
            ItemType::Event { .. } => ItemKind::Event,
            ItemType::Template { .. } => ItemKind::Template,
            ItemType::Simple => ItemKind::Simple,
        }
    }

    /// Constructs the default (empty-payload) variant for a given `ItemKind` — used
    /// when a caller (SDK conversion, a fresh `Item::new_*`) only knows the kind and
    /// hasn't supplied schedule/recurrence/assignment data yet.
    pub fn from_kind(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Task => ItemType::Task {
                schedule: Schedule::default(),
                recurrence: Recurrence::default(),
                team_assignment: None,
                source_event_id: None,
            },
            ItemKind::Event => ItemType::Event {
                schedule: Schedule::default(),
                recurrence: Recurrence::default(),
                event_type: None,
            },
            ItemKind::Template => ItemType::Template {
                schedule: Schedule::default(),
                recurrence: Recurrence::default(),
                event_type: None,
            },
            ItemKind::Simple => ItemType::Simple,
        }
    }

    pub fn schedule(&self) -> Option<&Schedule> {
        match self {
            ItemType::Task { schedule, .. }
            | ItemType::Event { schedule, .. }
            | ItemType::Template { schedule, .. } => Some(schedule),
            ItemType::Simple => None,
        }
    }

    pub fn schedule_mut(&mut self) -> Option<&mut Schedule> {
        match self {
            ItemType::Task { schedule, .. }
            | ItemType::Event { schedule, .. }
            | ItemType::Template { schedule, .. } => Some(schedule),
            ItemType::Simple => None,
        }
    }

    pub fn recurrence(&self) -> Option<&Recurrence> {
        match self {
            ItemType::Task { recurrence, .. }
            | ItemType::Event { recurrence, .. }
            | ItemType::Template { recurrence, .. } => Some(recurrence),
            ItemType::Simple => None,
        }
    }

    pub fn recurrence_mut(&mut self) -> Option<&mut Recurrence> {
        match self {
            ItemType::Task { recurrence, .. }
            | ItemType::Event { recurrence, .. }
            | ItemType::Template { recurrence, .. } => Some(recurrence),
            ItemType::Simple => None,
        }
    }

    pub fn event_type(&self) -> Option<&str> {
        match self {
            ItemType::Event { event_type, .. } | ItemType::Template { event_type, .. } => {
                event_type.as_deref()
            }
            ItemType::Task { .. } | ItemType::Simple => None,
        }
    }

    pub fn team_assignment(&self) -> Option<&TeamAssignment> {
        match self {
            ItemType::Task { team_assignment, .. } => team_assignment.as_ref(),
            _ => None,
        }
    }

    pub fn source_event_id(&self) -> Option<&str> {
        match self {
            ItemType::Task { source_event_id, .. } => source_event_id.as_deref(),
            _ => None,
        }
    }
}

/// Max length (in `char`s) allowed for `Item::name`, enforced by `validate()` and
/// mirrored client-side via `maxlength` on every name `<input>`/`<textarea>` in the
/// web UI so the browser blocks the common case before a round-trip is needed.
pub const MAX_NAME_LENGTH: usize = 200;

/// Max length (in `char`s) allowed for `Item::description`, enforced the same way as
/// `MAX_NAME_LENGTH` above but considerably larger — this is free-form notes text, not
/// a title. Unlike most other fields, `description` is a plain `Item` field rather than
/// something living inside `ItemType`'s variants: it applies uniformly to every kind
/// (Task/Event/Template/Simple), so there's no per-variant payload to attach it to.
pub const MAX_DESCRIPTION_LENGTH: usize = 5000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub user_id: Option<String>,
    /// Dual-written alongside `user_id` (see docs/project-abstraction-plan.md
    /// stage B2) — the project this item belongs to under the new Project abstraction.
    /// Populated on create; on update it's carried forward from the fetched `current`
    /// row rather than re-resolved, since an item's owner (and thus its project) never
    /// changes after creation. (`team_id` was a third dual-write alongside this one
    /// until Stage 6 of docs/team-id-removal-plan.md dropped both the field and its
    /// backing `items.team_id` column.)
    pub project_id: Option<String>,
    pub parent_item_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub complete: bool,
    pub has_children: bool,
    pub item_type: ItemType,
}

impl Item {
    pub fn new_user_item(user_id: &str, name: &str) -> Self {
        Self {
            user_id: Some(user_id.to_string()),
            name: name.to_string(),
            ..Self::default()
        }
    }

    /// `project_id`-primary constructor, added in Stage 4 of
    /// `docs/team-id-removal-plan.md` for `service::team_items.rs`, which treats
    /// `project_id` as its sole scoping key. Leaves `user_id` unset.
    pub fn new_project_item(project_id: &str, name: &str) -> Self {
        Self {
            project_id: Some(project_id.to_string()),
            name: name.to_string(),
            ..Self::default()
        }
    }

    /// Thin `item_type`-setting sugar over `new_user_item` — kept separate from the
    /// owner constructors above since "who owns this" and "what kind is this" are
    /// independent axes. Note these only control an item's *initial* defaults:
    /// `create_item`/`update_item` overlay every field from caller params
    /// unconditionally afterward.
    pub fn new_task(user_id: &str, name: &str) -> Self {
        Self::new_user_item(user_id, name)
    }

    pub fn new_event(user_id: &str, name: &str) -> Self {
        Self {
            item_type: ItemType::from_kind(ItemKind::Event),
            ..Self::new_user_item(user_id, name)
        }
    }

    pub fn new_simple(user_id: &str, name: &str) -> Self {
        Self {
            item_type: ItemType::Simple,
            ..Self::new_user_item(user_id, name)
        }
    }

    pub fn kind(&self) -> ItemKind {
        self.item_type.kind()
    }

    pub fn due_date(&self) -> Option<DateTime<Utc>> {
        self.item_type.schedule().and_then(|s| s.due_date)
    }

    pub fn has_due_time(&self) -> bool {
        self.item_type.schedule().is_some_and(|s| s.has_due_time)
    }

    pub fn scheduled_date(&self) -> Option<DateTime<Utc>> {
        self.item_type.schedule().and_then(|s| s.scheduled_date)
    }

    pub fn has_scheduled_time(&self) -> bool {
        self.item_type
            .schedule()
            .is_some_and(|s| s.has_scheduled_time)
    }

    pub fn scheduled_end_date(&self) -> Option<DateTime<Utc>> {
        self.item_type
            .schedule()
            .and_then(|s| s.scheduled_end_date)
    }

    pub fn has_end_time(&self) -> bool {
        self.item_type.schedule().is_some_and(|s| s.has_end_time)
    }

    pub fn recurrence_pattern(&self) -> Option<String> {
        self.item_type.recurrence().and_then(|r| r.pattern.clone())
    }

    pub fn recurrence_basis(&self) -> Option<String> {
        self.item_type.recurrence().and_then(|r| r.basis.clone())
    }

    pub fn due_offset_days(&self) -> Option<i32> {
        self.item_type.recurrence().and_then(|r| r.due_offset_days)
    }

    pub fn event_type(&self) -> Option<String> {
        self.item_type.event_type().map(|s| s.to_string())
    }

    pub fn assigned_to_user_id(&self) -> Option<String> {
        self.item_type
            .team_assignment()
            .and_then(|a| a.assigned_to_user_id.clone())
    }

    pub fn points(&self) -> Option<i32> {
        self.item_type.team_assignment().and_then(|a| a.points)
    }

    pub fn source_event_id(&self) -> Option<String> {
        self.item_type.source_event_id().map(|s| s.to_string())
    }

    /// True if this item's own date is derived from an offset rather than
    /// freely set — either a structural child (`parent_item_id`) or a task
    /// referencing an Event (`source_event_id`). Every "child-like"
    /// restriction (no manual scheduled window, no recurrence, due date
    /// computed rather than accepted) keys off this one predicate.
    pub fn is_offset_driven(&self) -> bool {
        self.parent_item_id.is_some() || self.source_event_id().is_some()
    }

    /// Enforces the cross-field invariants that assigning the payload to the wrong
    /// variant would otherwise still allow at construction time (a caller can still
    /// build `ItemType::Task { team_assignment: Some(..), .. }` on a child by hand) —
    /// called by the service layer once an `Item` is fully assembled, right before
    /// it's persisted.
    pub fn validate(&self) -> Result<(), String> {
        // Points are top-level-only, same shape as the recurrence+parentItemId
        // rejection in create_team_item/update_team_item — a child can't carry
        // its own point value (see CLAUDE.md's Points plan, Stage 2).
        if self.points().is_some() && self.parent_item_id.is_some() {
            return Err("child items cannot have points".to_string());
        }
        if self.name.chars().count() > MAX_NAME_LENGTH {
            return Err(format!("name must be {MAX_NAME_LENGTH} characters or fewer"));
        }
        if self
            .description
            .as_ref()
            .is_some_and(|d| d.chars().count() > MAX_DESCRIPTION_LENGTH)
        {
            return Err(format!(
                "description must be {MAX_DESCRIPTION_LENGTH} characters or fewer"
            ));
        }
        // Simple items are bare checkable-off-nothing names: create or delete, no
        // completion concept at all (unlike Task/Event, which support it).
        if self.complete && self.kind() == ItemKind::Simple {
            return Err("simple items cannot be marked complete".to_string());
        }
        // A task either nests under a parent or references an event, never both —
        // keeps "offset-driven" unambiguous: exactly one anchor source.
        if self.source_event_id().is_some() && self.parent_item_id.is_some() {
            return Err(
                "an item cannot both have a parent and reference an event".to_string(),
            );
        }
        // Offset-driven items (children and event-linked tasks) get their due date
        // computed from the offset (see service::items::resolve_offset_anchor) —
        // there's no equivalent "scheduled offset", so a manual scheduled window
        // isn't allowed on them at all.
        if self.is_offset_driven()
            && (self.scheduled_date().is_some() || self.scheduled_end_date().is_some())
        {
            return Err(
                "scheduled dates are not allowed on child or event-linked items"
                    .to_string(),
            );
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
    /// rides along unchanged.
    pub fn next_recurrence(&self, now: DateTime<Utc>, tz_offset_minutes: i32) -> Option<(Self, DateTime<Utc>)> {
        if !self.complete {
            return None;
        }
        let pattern = self.recurrence_pattern()?;
        let rule = recurrence::parse(&pattern).ok()?;
        let basis = self.recurrence_basis().unwrap_or_else(|| "DUE_DATE".to_string());

        let mut next = self.clone();
        next.id = String::new();
        next.complete = false;
        let schedule = next.item_type.schedule_mut()?;

        let anchor = if basis == "DUE_DATE" {
            let reference = schedule.due_date.unwrap_or(now);
            let next_date = recurrence::next_date(&rule, reference, tz_offset_minutes);
            schedule.has_due_time = rule.time_override.is_some() || schedule.has_due_time;
            schedule.due_date = Some(next_date);
            next_date
        } else {
            let reference = if basis == "SCHEDULED_DATE" {
                schedule.scheduled_date.unwrap_or(now)
            } else {
                now
            };
            let next_date = recurrence::next_date(&rule, reference, tz_offset_minutes);
            let delta = next_date - reference;
            schedule.scheduled_end_date = schedule.scheduled_end_date.map(|e| e + delta);
            schedule.has_scheduled_time =
                rule.time_override.is_some() || schedule.has_scheduled_time;
            schedule.scheduled_date = Some(next_date);
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
        self.due_offset_days().map(|days| {
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
        !self.complete && self.due_date().is_some_and(|d| d < now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_due_date(item: &mut Item, dt: DateTime<Utc>) {
        item.item_type.schedule_mut().expect("has schedule").due_date = Some(dt);
    }

    fn set_scheduled_date(item: &mut Item, dt: DateTime<Utc>) {
        item.item_type
            .schedule_mut()
            .expect("has schedule")
            .scheduled_date = Some(dt);
    }

    fn set_scheduled_end_date(item: &mut Item, dt: DateTime<Utc>) {
        item.item_type
            .schedule_mut()
            .expect("has schedule")
            .scheduled_end_date = Some(dt);
    }

    fn set_recurrence(item: &mut Item, pattern: &str) {
        item.item_type
            .recurrence_mut()
            .expect("has recurrence")
            .pattern = Some(pattern.to_string());
    }

    fn set_recurrence_basis(item: &mut Item, basis: &str) {
        item.item_type
            .recurrence_mut()
            .expect("has recurrence")
            .basis = Some(basis.to_string());
    }

    fn set_due_offset_days(item: &mut Item, days: i32) {
        item.item_type
            .recurrence_mut()
            .expect("has recurrence")
            .due_offset_days = Some(days);
    }

    fn set_source_event_id(item: &mut Item, event_id: &str) {
        match &mut item.item_type {
            ItemType::Task { source_event_id, .. } => *source_event_id = Some(event_id.to_string()),
            _ => panic!("source_event_id only settable on Task"),
        }
    }

    fn set_points(item: &mut Item, points: i32) {
        match &mut item.item_type {
            ItemType::Task { team_assignment, .. } => {
                *team_assignment = Some(TeamAssignment {
                    points: Some(points),
                    ..team_assignment.clone().unwrap_or_default()
                })
            }
            _ => panic!("points only settable on Task"),
        }
    }

    fn set_event_type(item: &mut Item, event_type: &str) {
        match &mut item.item_type {
            ItemType::Event { event_type: e, .. } | ItemType::Template { event_type: e, .. } => {
                *e = Some(event_type.to_string())
            }
            _ => panic!("event_type only settable on Event/Template"),
        }
    }

    #[test]
    fn new_user_item_sets_user_id_and_name() {
        let item = Item::new_user_item("u1", "Buy milk");
        assert_eq!(item.user_id, Some("u1".to_string()));
        assert_eq!(item.name, "Buy milk");
        assert_eq!(item.kind(), ItemKind::Task);
    }

    #[test]
    fn new_simple_sets_simple_item_type() {
        let item = Item::new_simple("u1", "Milk");
        assert_eq!(item.kind(), ItemKind::Simple);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn simple_item_type_has_no_schedule_or_recurrence() {
        let item = Item::new_simple("u1", "Milk");
        assert!(item.item_type.schedule().is_none());
        assert!(item.item_type.recurrence().is_none());
        assert!(item.due_date().is_none());
        assert!(item.recurrence_pattern().is_none());
        assert!(item.due_offset_days().is_none());
    }

    #[test]
    fn validate_rejects_name_over_max_length() {
        let item = Item::new_task("u1", &"a".repeat(MAX_NAME_LENGTH + 1));
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_name_at_max_length() {
        let item = Item::new_task("u1", &"a".repeat(MAX_NAME_LENGTH));
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_description_over_max_length() {
        let mut item = Item::new_task("u1", "Task");
        item.description = Some("a".repeat(MAX_DESCRIPTION_LENGTH + 1));
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_description_at_max_length() {
        let mut item = Item::new_task("u1", "Task");
        item.description = Some("a".repeat(MAX_DESCRIPTION_LENGTH));
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_complete_simple_item() {
        let mut item = Item::new_simple("u1", "Milk");
        item.complete = true;
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_rejects_points_on_child() {
        let mut item = Item::new_user_item("u1", "Subtask");
        item.parent_item_id = Some("parent1".to_string());
        set_points(&mut item, 10);
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_points_on_top_level_item() {
        let mut item = Item::new_user_item("u1", "Task");
        set_points(&mut item, 10);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_item_with_both_parent_and_source_event() {
        let mut item = Item::new_task("u1", "Buy cake");
        item.parent_item_id = Some("parent1".to_string());
        set_source_event_id(&mut item, "event1");
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_source_event_linked_task_with_no_parent() {
        let mut item = Item::new_task("u1", "Buy cake");
        set_source_event_id(&mut item, "event1");
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_scheduled_date_on_child_item() {
        let mut item = Item::new_task("u1", "Sub-task");
        item.parent_item_id = Some("parent1".to_string());
        set_scheduled_date(&mut item, Utc::now());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_rejects_scheduled_date_on_source_event_linked_task() {
        let mut item = Item::new_task("u1", "Buy cake");
        set_source_event_id(&mut item, "event1");
        set_scheduled_date(&mut item, Utc::now());
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_scheduled_date_on_independent_top_level_item() {
        let mut item = Item::new_task("u1", "Plan trip");
        set_scheduled_date(&mut item, Utc::now());
        assert!(item.validate().is_ok());
    }

    #[test]
    fn is_offset_driven_true_for_child() {
        let mut item = Item::new_task("u1", "Sub-task");
        item.parent_item_id = Some("parent1".to_string());
        assert!(item.is_offset_driven());
    }

    #[test]
    fn is_offset_driven_true_for_source_event_linked_task() {
        let mut item = Item::new_task("u1", "Buy cake");
        set_source_event_id(&mut item, "event1");
        assert!(item.is_offset_driven());
    }

    #[test]
    fn is_offset_driven_false_for_independent_top_level_item() {
        let item = Item::new_task("u1", "Plan trip");
        assert!(!item.is_offset_driven());
    }

    #[test]
    fn validate_allows_task_with_all_scheduling_fields() {
        let mut item = Item::new_task("u1", "Water plants");
        let now = Utc::now();
        set_due_date(&mut item, now);
        set_scheduled_date(&mut item, now);
        set_scheduled_end_date(&mut item, now);
        set_recurrence(&mut item, "every day");
        set_due_offset_days(&mut item, 1);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_allows_event_with_all_scheduling_fields() {
        let mut item = Item::new_event("u1", "Team offsite");
        let now = Utc::now();
        set_due_date(&mut item, now);
        set_scheduled_date(&mut item, now);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_allows_event_with_event_type() {
        let mut item = Item::new_event("u1", "Storm watch");
        set_event_type(&mut item, "rain");
        assert!(item.validate().is_ok());
    }

    #[test]
    fn task_item_type_has_no_event_type_slot() {
        let item = Item::new_task("u1", "Water plants");
        assert!(item.event_type().is_none());
        // A Task variant has no field to put event_type in at all — this is the
        // compile-time guarantee the old flat-Option + validate() rejection used to
        // enforce only at runtime.
        assert!(matches!(item.item_type, ItemType::Task { .. }));
    }

    #[test]
    fn simple_item_type_has_no_event_type_slot() {
        let item = Item::new_simple("u1", "Milk");
        assert!(item.event_type().is_none());
    }

    #[test]
    fn validate_allows_template_with_event_type() {
        let mut item = Item::new_user_item("u1", "Rain prep");
        item.item_type = ItemType::from_kind(ItemKind::Template);
        set_event_type(&mut item, "rain");
        assert!(item.validate().is_ok());
    }

    #[test]
    fn new_items_are_not_complete() {
        let user_item = Item::new_user_item("u1", "task");
        let project_item = Item::new_project_item("p1", "task");
        assert!(!user_item.complete);
        assert!(!project_item.complete);
    }

    #[test]
    fn next_recurrence_none_when_not_complete() {
        let mut item = Item::new_user_item("u1", "Water plants");
        set_recurrence(&mut item, "every 3 days");
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
        set_recurrence(&mut item, "every 3 days");
        set_due_date(&mut item, Utc::now());

        let (next, anchor) = item.next_recurrence(Utc::now(), 0).expect("should recur");

        assert!(next.id.is_empty());
        assert!(!next.complete);
        assert_eq!(next.user_id, item.user_id);
        assert_eq!(next.name, item.name);
        assert_eq!(next.recurrence_pattern(), item.recurrence_pattern());
        assert!(next.due_date().unwrap() > item.due_date().unwrap());
        assert_eq!(anchor, next.due_date().unwrap());
    }

    #[test]
    fn next_recurrence_with_scheduled_basis_advances_scheduled_date_not_due_date() {
        let mut item = Item::new_user_item("u1", "Water plants");
        item.complete = true;
        set_recurrence(&mut item, "every 3 days");
        set_recurrence_basis(&mut item, "SCHEDULED_DATE");
        set_scheduled_date(&mut item, Utc::now());
        set_due_date(
            &mut item,
            DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let (next, anchor) = item.next_recurrence(Utc::now(), 0).expect("should recur");

        assert!(next.scheduled_date().unwrap() > item.scheduled_date().unwrap());
        assert_eq!(next.due_date(), item.due_date());
        assert_eq!(anchor, next.scheduled_date().unwrap());
    }

    #[test]
    fn next_recurrence_with_scheduled_basis_preserves_window_length() {
        let mut item = Item::new_user_item("u1", "Work session");
        item.complete = true;
        set_recurrence(&mut item, "every week");
        set_recurrence_basis(&mut item, "SCHEDULED_DATE");
        let start = Utc::now();
        set_scheduled_date(&mut item, start);
        set_scheduled_end_date(&mut item, start + Duration::hours(2));

        let (next, _anchor) = item.next_recurrence(Utc::now(), 0).expect("should recur");

        let original_gap = item.scheduled_end_date().unwrap() - item.scheduled_date().unwrap();
        let new_gap = next.scheduled_end_date().unwrap() - next.scheduled_date().unwrap();
        assert_eq!(original_gap, new_gap);
    }

    #[test]
    fn next_recurrence_with_completion_basis_writes_scheduled_date() {
        let mut item = Item::new_user_item("u1", "Take out trash");
        item.complete = true;
        set_recurrence(&mut item, "every 3 days");
        set_recurrence_basis(&mut item, "COMPLETION_DATE");
        set_due_date(
            &mut item,
            DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let now = Utc::now();
        let (next, anchor) = item.next_recurrence(now, 0).expect("should recur");

        assert!(next.scheduled_date().is_some());
        assert!(next.scheduled_date().unwrap() > now);
        assert_eq!(next.due_date(), item.due_date());
        assert_eq!(anchor, next.scheduled_date().unwrap());
    }

    #[test]
    fn deadline_from_offset_none_when_no_offset() {
        let child = Item::new_user_item("u1", "Check inbox");
        assert!(child.deadline_from_offset(Utc::now(), 0).is_none());
    }

    #[test]
    fn deadline_from_offset_adds_days_to_root_deadline() {
        let mut child = Item::new_user_item("u1", "Prep agenda");
        set_due_offset_days(&mut child, -2);
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
        set_due_offset_days(&mut child, 1);
        set_due_date(
            &mut child,
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
        set_due_date(&mut item, utc("2020-01-01T00:00:00Z"));
        assert!(item.is_overdue(utc("2026-01-01T00:00:00Z")));
    }

    #[test]
    fn is_overdue_false_when_due_date_in_future() {
        let mut item = Item::new_user_item("u1", "Pay rent");
        set_due_date(&mut item, utc("2030-01-01T00:00:00Z"));
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
        set_due_date(&mut item, utc("2020-01-01T00:00:00Z"));
        item.complete = true;
        assert!(!item.is_overdue(utc("2026-01-01T00:00:00Z")));
    }
}
