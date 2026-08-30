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

/// Payload for `ItemType::Task`. See `ItemType` below for the per-kind split rationale.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskItem {
    /// Moved off `Item` in Stage 4 of `docs/item-kind-split-plan.md` — `Task`,
    /// `Simple` (see `SimpleItem::parent_item_id`), and `Template` (see
    /// `TemplateItem::parent_item_id`) items can all legitimately nest; only
    /// `Event` structurally cannot carry a parent at all now.
    pub parent_item_id: Option<String>,
    pub schedule: Schedule,
    pub recurrence: Recurrence,
    pub team_assignment: Option<TeamAssignment>,
    /// The Event this task tracks, if it was auto-copied from a matching
    /// template or manually linked from the event's own "Linked tasks"
    /// form — a reference, not structural nesting, so the task stays
    /// top-level (assignable/pointable) while still being offset-driven
    /// off the event's own date. Mutually exclusive with `parent_item_id`
    /// (see `Item::validate`) and only settable on `Task` — only a Task
    /// has anywhere to put it.
    pub source_event_id: Option<String>,
    /// 1 (highest) through 4 (lowest); `None` sorts last. Task-only, but
    /// unlike `team_assignment` above it's a plain personal-productivity
    /// field — not team-backed-project-only, not admin-gated, and not
    /// restricted to top-level items. See root CLAUDE.md's Priority section.
    pub priority: Option<i32>,
    /// Moved off `Item` in Stage 3 of `docs/item-kind-split-plan.md` — only a `Task`
    /// has anywhere to put a `complete` flag now; `EventItem`/`TemplateItem`/
    /// `SimpleItem` structurally cannot represent a completed item at all. No
    /// `set_complete()` convenience exists anywhere (on `Item` or `Completable`) —
    /// every write site matches into `ItemType::Task` directly, so there's no code
    /// path that can attempt to complete a non-Task item without first failing to
    /// compile. See the `Completable` trait below.
    pub complete: bool,
}

/// Payload for `ItemType::Event`. See `ItemType` below for the per-kind split rationale.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventItem {
    pub schedule: Schedule,
    pub recurrence: Recurrence,
    pub event_type: Option<String>,
}

/// Payload for `ItemType::Template`. See `ItemType` below for the per-kind split rationale.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateItem {
    /// **Correction to `docs/item-kind-split-plan.md`'s Stage 0 audit**, made during
    /// Stage 4: the audit checked only `service::templates::create_template` (a
    /// *root* template never has a parent) and concluded Template can never be a
    /// child at all — but `service::items::copy_children_as_template` recursively
    /// copies a real item's descendants into *nested* Template-typed rows, each
    /// with `parent_item_id` set to the template row above it, and `create_item`'s
    /// own "child of a Template auto-becomes a Template" branch round-trips a
    /// caller-supplied `parent_item_id` onto a freshly created Template child the
    /// same way. So Template *is* legitimately Task-and-Simple-and-Template for
    /// this field, not Task-and-Simple-only — only a *root* template (no
    /// `source_item_id`/no parent) ever has `None` here.
    pub parent_item_id: Option<String>,
    pub schedule: Schedule,
    pub recurrence: Recurrence,
    pub event_type: Option<String>,
}

/// Payload for `ItemType::Simple` — a bare checkable name with no scheduling
/// machinery — no due date, scheduled window, recurrence, due-offset, or
/// event_type. `parent_item_id` moved onto this struct in Stage 4 of
/// `docs/item-kind-split-plan.md` — `project_simple_lists/` supports nesting
/// a Simple item under another Simple item, so this is legitimately Task-and-Simple,
/// not Task-only (see that plan's Stage 0 audit).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SimpleItem {
    pub parent_item_id: Option<String>,
}

/// Implemented by every `ItemType` payload that carries a `Schedule`/`Recurrence`
/// (`TaskItem`, `EventItem`, `TemplateItem` — not `SimpleItem`, which genuinely has
/// neither). For a handful of truly kind-blind helpers (date-based sort/filter
/// predicates, virtual/materialized-occurrence merges) that want to work generically
/// across whichever of these three kinds they're given, without matching `ItemType`
/// themselves. See `docs/item-kind-split-plan.md`'s "Traits for generic code" section
/// for why this exists alongside, not instead of, `Item`'s own delegation methods
/// (`Item::due_date()` etc.) — those remain the right choice at almost every call site;
/// this trait is only for the rare function that's handed a payload struct directly.
///
/// `#[allow(dead_code)]`: as of Stage 2 of `docs/item-kind-split-plan.md`, no call site
/// in this codebase is actually kind-blind in the way this trait exists to serve —
/// every current helper (`list_filters.rs`, `item_series.rs`'s occurrence merge,
/// `main_calendar.rs`'s fan-out) already goes through `Item`'s own delegation methods
/// rather than touching `TaskItem`/`EventItem`/`TemplateItem` directly, so there was
/// nothing to repoint. Remove this attribute once a real consumer exists.
#[allow(dead_code)]
pub trait HasSchedule {
    fn schedule(&self) -> &Schedule;
    fn recurrence(&self) -> &Recurrence;
}

impl HasSchedule for TaskItem {
    fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn recurrence(&self) -> &Recurrence {
        &self.recurrence
    }
}

impl HasSchedule for EventItem {
    fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn recurrence(&self) -> &Recurrence {
        &self.recurrence
    }
}

impl HasSchedule for TemplateItem {
    fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn recurrence(&self) -> &Recurrence {
        &self.recurrence
    }
}

/// Implemented only by payloads whose kind can be marked complete — as of Stage 3 of
/// `docs/item-kind-split-plan.md`, that's `TaskItem` alone. `EventItem`/`TemplateItem`/
/// `SimpleItem` deliberately have no impl: there's no `complete` field on those structs
/// to read, so "can this be completed" is now a compile-time question, not a runtime
/// `validate()` one. See `docs/item-kind-split-plan.md`'s "Traits for generic code"
/// section for why this exists alongside `Item::complete()` rather than instead of it.
///
/// `#[allow(dead_code)]`: no genuinely kind-blind call site consumes this yet (same
/// situation as `HasSchedule` above) — every current caller goes through `Item::complete()`.
/// Remove this attribute once a real consumer exists.
#[allow(dead_code)]
pub trait Completable {
    fn complete(&self) -> bool;
}

impl Completable for TaskItem {
    fn complete(&self) -> bool {
        self.complete
    }
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
    Task(TaskItem),
    Event(EventItem),
    Template(TemplateItem),
    Simple(SimpleItem),
}

impl Default for ItemType {
    fn default() -> Self {
        ItemType::Task(TaskItem::default())
    }
}

impl ItemType {
    pub fn kind(&self) -> ItemKind {
        match self {
            ItemType::Task(_) => ItemKind::Task,
            ItemType::Event(_) => ItemKind::Event,
            ItemType::Template(_) => ItemKind::Template,
            ItemType::Simple(_) => ItemKind::Simple,
        }
    }

    /// Constructs the default (empty-payload) variant for a given `ItemKind` — used
    /// when a caller (SDK conversion, a fresh `Item::new_*`) only knows the kind and
    /// hasn't supplied schedule/recurrence/assignment data yet.
    pub fn from_kind(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Task => ItemType::Task(TaskItem::default()),
            ItemKind::Event => ItemType::Event(EventItem::default()),
            ItemKind::Template => ItemType::Template(TemplateItem::default()),
            ItemKind::Simple => ItemType::Simple(SimpleItem::default()),
        }
    }

    pub fn schedule(&self) -> Option<&Schedule> {
        match self {
            ItemType::Task(t) => Some(&t.schedule),
            ItemType::Event(e) => Some(&e.schedule),
            ItemType::Template(t) => Some(&t.schedule),
            ItemType::Simple(_) => None,
        }
    }

    pub fn schedule_mut(&mut self) -> Option<&mut Schedule> {
        match self {
            ItemType::Task(t) => Some(&mut t.schedule),
            ItemType::Event(e) => Some(&mut e.schedule),
            ItemType::Template(t) => Some(&mut t.schedule),
            ItemType::Simple(_) => None,
        }
    }

    pub fn recurrence(&self) -> Option<&Recurrence> {
        match self {
            ItemType::Task(t) => Some(&t.recurrence),
            ItemType::Event(e) => Some(&e.recurrence),
            ItemType::Template(t) => Some(&t.recurrence),
            ItemType::Simple(_) => None,
        }
    }

    pub fn recurrence_mut(&mut self) -> Option<&mut Recurrence> {
        match self {
            ItemType::Task(t) => Some(&mut t.recurrence),
            ItemType::Event(e) => Some(&mut e.recurrence),
            ItemType::Template(t) => Some(&mut t.recurrence),
            ItemType::Simple(_) => None,
        }
    }

    pub fn event_type(&self) -> Option<&str> {
        match self {
            ItemType::Event(e) => e.event_type.as_deref(),
            ItemType::Template(t) => t.event_type.as_deref(),
            ItemType::Task(_) | ItemType::Simple(_) => None,
        }
    }

    pub fn team_assignment(&self) -> Option<&TeamAssignment> {
        match self {
            ItemType::Task(t) => t.team_assignment.as_ref(),
            _ => None,
        }
    }

    pub fn source_event_id(&self) -> Option<&str> {
        match self {
            ItemType::Task(t) => t.source_event_id.as_deref(),
            _ => None,
        }
    }

    pub fn priority(&self) -> Option<i32> {
        match self {
            ItemType::Task(t) => t.priority,
            _ => None,
        }
    }

    pub fn parent_item_id(&self) -> Option<&str> {
        match self {
            ItemType::Task(t) => t.parent_item_id.as_deref(),
            ItemType::Simple(s) => s.parent_item_id.as_deref(),
            ItemType::Template(t) => t.parent_item_id.as_deref(),
            ItemType::Event(_) => None,
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
    pub name: String,
    pub description: Option<String>,
    pub has_children: bool,
    pub item_type: ItemType,
    /// Set once, at creation, only by `service::item_series::get_or_materialize_occurrence` —
    /// no Smithy field, CLI flag, or MCP parameter can ever set or clear it. `None` for every
    /// item not materialized from a series. Carried forward on every update (like `project_id`)
    /// since an item's series membership never changes after creation.
    pub series_id: Option<String>,
    /// The upstream iCal `UID` (or, once Stage 7 lands, a synthetic per-occurrence id
    /// derived from it) this item was imported from — see
    /// `docs/google-calendar-import-plan.md`. Set once, at creation, only by
    /// `service::calendar_sync::sync_subscription`; `None` for every non-imported item.
    /// Follows `series_id`'s exact precedent: carried forward unchanged on every update,
    /// never settable via Smithy/CLI/MCP.
    pub google_event_id: Option<String>,
    /// The `calendar_subscriptions` row this item was imported from, alongside
    /// `google_event_id` above. `None` for every non-imported item.
    pub calendar_subscription_id: Option<String>,
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
            item_type: ItemType::Simple(SimpleItem::default()),
            ..Self::new_user_item(user_id, name)
        }
    }

    pub fn kind(&self) -> ItemKind {
        self.item_type.kind()
    }

    /// `false` for every non-`Task` kind — matches today's implicit behavior, since
    /// `validate()` already forbade `true` on those kinds before this field moved onto
    /// `TaskItem` in Stage 3 of `docs/item-kind-split-plan.md`. No `Item::set_complete()`
    /// exists — see `TaskItem::complete`'s doc comment for why every write site must
    /// match into `ItemType::Task` directly instead.
    pub fn complete(&self) -> bool {
        match &self.item_type {
            ItemType::Task(t) => t.complete,
            _ => false,
        }
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
        self.item_type.schedule().and_then(|s| s.scheduled_end_date)
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

    pub fn priority(&self) -> Option<i32> {
        self.item_type.priority()
    }

    /// `None` for `Event` — moved off `Item` in Stage 4 of
    /// `docs/item-kind-split-plan.md`; `TaskItem`/`SimpleItem`/`TemplateItem` are
    /// the only kinds with anywhere to put a parent now. No
    /// `Item::set_parent_item_id()` exists — every write site matches into
    /// `ItemType::Task`/`ItemType::Simple`/`ItemType::Template` directly,
    /// following `TaskItem::complete`'s precedent from Stage 3. Owned `String`
    /// (not `&str`), matching `event_type()`/`source_event_id()`'s convention.
    pub fn parent_item_id(&self) -> Option<String> {
        self.item_type.parent_item_id().map(|s| s.to_string())
    }

    /// True if this item's own date is derived from an offset rather than
    /// freely set — either a structural child (`parent_item_id`) or a task
    /// referencing an Event (`source_event_id`). Every "child-like"
    /// restriction (no manual scheduled window, no recurrence, due date
    /// computed rather than accepted) keys off this one predicate.
    pub fn is_offset_driven(&self) -> bool {
        self.parent_item_id().is_some() || self.source_event_id().is_some()
    }

    /// Enforces the cross-field invariants that assigning the payload to the wrong
    /// variant would otherwise still allow at construction time (a caller can still
    /// build `ItemType::Task(TaskItem { team_assignment: Some(..), .. })` on a child by hand) —
    /// called by the service layer once an `Item` is fully assembled, right before
    /// it's persisted.
    pub fn validate(&self) -> Result<(), String> {
        // Points are top-level-only, same shape as the recurrence+parentItemId
        // rejection in create_team_item/update_team_item — a child can't carry
        // its own point value (see CLAUDE.md's Points plan, Stage 2).
        if self.points().is_some() && self.parent_item_id().is_some() {
            return Err("child items cannot have points".to_string());
        }
        if self.name.chars().count() > MAX_NAME_LENGTH {
            return Err(format!(
                "name must be {MAX_NAME_LENGTH} characters or fewer"
            ));
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
        // completion concept at all (unlike Task, which supports it).
        //
        // As of Stage 3 of `docs/item-kind-split-plan.md`, `complete` lives only on
        // `TaskItem` — `self.complete()` structurally cannot return `true` for a
        // `Simple`/`Event` item (there's no field to read), so these two checks are
        // now unreachable for any internally-constructed `Item`. Left in place rather
        // than deleted: they still document the rule, and they're the actual
        // enforcement point if a future wire-input boundary ever regains a way to
        // request completion alongside a non-Task kind (see Non-goals #1 in that plan
        // — the Smithy wire format itself is untouched and stays a flat, unvalidated
        // property bag).
        if self.complete() && self.kind() == ItemKind::Simple {
            return Err("simple items cannot be marked complete".to_string());
        }
        // Events are scheduled entries, not actionable to-dos — no checkbox, no
        // Done/Not done, no "Show completed" filter (same treatment Simple items got
        // above). Item-level recurrence (the mechanism that used to require completion
        // to advance a recurring Event's date) was retired in Stage 10; recurring
        // Events now live on item_series, advanced by materialization, independent of
        // this field. See docs/archived/archived_issues_and_features.md for the removal.
        if self.complete() && self.kind() == ItemKind::Event {
            return Err("events cannot be marked complete".to_string());
        }
        // A task either nests under a parent or references an event, never both —
        // keeps "offset-driven" unambiguous: exactly one anchor source.
        if self.source_event_id().is_some() && self.parent_item_id().is_some() {
            return Err("an item cannot both have a parent and reference an event".to_string());
        }
        // It never makes sense for a child/event-linked item's own due date to fall after its
        // anchor's — a sub-item existing to track work that supports the parent shouldn't be
        // "due" once the parent already is. `due_offset_days` keeps its historical signed
        // representation (negative = before, positive = after, see CLAUDE.md's Recurrence
        // section) for every existing positive row written before this check landed, but no
        // new write may set one: the web UI's own offset field enforces "days before" (positive
        // input, negated before it reaches here) at the presentation layer, and this is the
        // matching enforcement point for every other writer (CLI, MCP, direct API calls).
        if self.due_offset_days().is_some_and(|d| d > 0) {
            return Err(
                "due offset days cannot be positive (days after the due date are not allowed)"
                    .to_string(),
            );
        }
        if self.priority().is_some_and(|p| !(1..=4).contains(&p)) {
            return Err("priority must be between 1 and 4".to_string());
        }
        Ok(())
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
        !self.complete() && self.due_date().is_some_and(|d| d < now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn set_scheduled_end_date(item: &mut Item, dt: DateTime<Utc>) {
        item.item_type
            .schedule_mut()
            .expect("has schedule")
            .scheduled_end_date = Some(dt);
    }

    fn set_due_offset_days(item: &mut Item, days: i32) {
        item.item_type
            .recurrence_mut()
            .expect("has recurrence")
            .due_offset_days = Some(days);
    }

    fn set_parent_item_id(item: &mut Item, parent_id: &str) {
        match &mut item.item_type {
            ItemType::Task(task) => task.parent_item_id = Some(parent_id.to_string()),
            ItemType::Simple(simple) => simple.parent_item_id = Some(parent_id.to_string()),
            _ => panic!("parent_item_id only settable on Task/Simple"),
        }
    }

    fn set_source_event_id(item: &mut Item, event_id: &str) {
        match &mut item.item_type {
            ItemType::Task(task) => task.source_event_id = Some(event_id.to_string()),
            _ => panic!("source_event_id only settable on Task"),
        }
    }

    fn set_points(item: &mut Item, points: i32) {
        match &mut item.item_type {
            ItemType::Task(task) => {
                task.team_assignment = Some(TeamAssignment {
                    points: Some(points),
                    ..task.team_assignment.clone().unwrap_or_default()
                })
            }
            _ => panic!("points only settable on Task"),
        }
    }

    fn set_priority(item: &mut Item, priority: i32) {
        match &mut item.item_type {
            ItemType::Task(task) => task.priority = Some(priority),
            _ => panic!("priority only settable on Task"),
        }
    }

    fn set_complete(item: &mut Item, complete: bool) {
        match &mut item.item_type {
            ItemType::Task(task) => task.complete = complete,
            _ => panic!("complete only settable on Task"),
        }
    }

    fn set_event_type(item: &mut Item, event_type: &str) {
        match &mut item.item_type {
            ItemType::Event(e) => e.event_type = Some(event_type.to_string()),
            ItemType::Template(t) => t.event_type = Some(event_type.to_string()),
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

    // `validate_rejects_complete_simple_item`/`validate_rejects_complete_event` (the
    // pre-Stage-3 runtime tests that did `item.complete = true` on a Simple/Event `Item`)
    // no longer compile as of Stage 3 of `docs/item-kind-split-plan.md`: `complete` now
    // lives only on `TaskItem`, so `ItemType::Simple(SimpleItem { complete: true, .. })`
    // and `ItemType::Event(EventItem { complete: true, .. })` are not expressible at all
    // — neither struct has such a field. That's the actual point of the migration: what
    // used to be a `validate()` rejection is now a compile error. The tests below assert
    // the type-level guarantee's runtime-observable consequence instead (`complete()`
    // always reads `false` for these kinds), since "this doesn't compile" isn't itself a
    // `#[test]`-able fact.

    #[test]
    fn simple_item_complete_is_always_false() {
        let item = Item::new_simple("u1", "Milk");
        assert!(!item.complete());
    }

    #[test]
    fn event_item_complete_is_always_false() {
        let item = Item::new_event("u1", "Standup");
        assert!(!item.complete());
    }

    #[test]
    fn validate_rejects_points_on_child() {
        let mut item = Item::new_user_item("u1", "Subtask");
        set_parent_item_id(&mut item, "parent1");
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
    fn validate_allows_priority_in_range() {
        let mut item = Item::new_task("u1", "Task");
        set_priority(&mut item, 1);
        assert!(item.validate().is_ok());
        set_priority(&mut item, 4);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_priority_out_of_range() {
        let mut item = Item::new_task("u1", "Task");
        set_priority(&mut item, 0);
        assert!(item.validate().is_err());
        set_priority(&mut item, 5);
        assert!(item.validate().is_err());
    }

    #[test]
    fn validate_allows_priority_on_child_task() {
        let mut item = Item::new_task("u1", "Subtask");
        set_parent_item_id(&mut item, "parent1");
        set_priority(&mut item, 2);
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_rejects_item_with_both_parent_and_source_event() {
        let mut item = Item::new_task("u1", "Buy cake");
        set_parent_item_id(&mut item, "parent1");
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
    fn validate_allows_scheduled_date_on_child_item() {
        let mut item = Item::new_task("u1", "Sub-task");
        set_parent_item_id(&mut item, "parent1");
        set_scheduled_date(&mut item, Utc::now());
        assert!(item.validate().is_ok());
    }

    #[test]
    fn validate_allows_scheduled_date_on_source_event_linked_task() {
        let mut item = Item::new_task("u1", "Buy cake");
        set_source_event_id(&mut item, "event1");
        set_scheduled_date(&mut item, Utc::now());
        assert!(item.validate().is_ok());
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
        set_parent_item_id(&mut item, "parent1");
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
        set_due_offset_days(&mut item, -1);
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
        assert!(matches!(item.item_type, ItemType::Task(_)));
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
        assert!(!user_item.complete());
        assert!(!project_item.complete());
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
        set_complete(&mut item, true);
        assert!(!item.is_overdue(utc("2026-01-01T00:00:00Z")));
    }
}
