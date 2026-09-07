//! Kind-typed inputs for the item create/update funnel.
//!
//! These are the input counterpart of `domain::item::ItemType` — one variant per
//! `ItemKind`, each carrying only the fields that kind can legitimately have. Until Stage 8
//! of `docs/archived/typed-item-params-plan.md` they sat in front of a flat, kind-agnostic
//! `CreateProjectItemParams`/`UpdateProjectItemParams` pair and converted into it; those
//! structs are gone now, and these are what `service::project_items`, `service::items` and
//! `service::team_items` actually take.
//!
//! The problem they exist to close: only *one* of the funnel's ~21 construction sites
//! (`json_api::project_items`) ever receives untyped input. Every other caller knows its kind
//! statically and was spelling out fields that kind can never carry — where a wrong one was
//! silently dropped rather than rejected. Now there is nowhere to put it, and at the two
//! places where the kind genuinely is data, it is rejected (see "the untyped boundary" below).
//!
//! Deliberately reuses the domain's own `Schedule` and `TeamAssignment` structs rather
//! than defining input twins: the shapes are identical, and mirroring them exactly is the
//! point. `Schedule`'s `has_*_time` flags are plain `bool` here where the flat params used
//! `Option<bool>` — the flat params only ever `unwrap_or(false)` them, so there was never
//! a third state to represent.
//!
//! `google_event_id`/`calendar_subscription_id` deliberately have no input field on
//! `NewEvent`, matching today: `service::calendar_sync` writes them by constructing an
//! `Item` directly rather than going through this funnel, and `build_item_type` below
//! hardcodes both to `None`.

use crate::domain::item::{
    EventItem, Item, ItemKind, ItemType, Recurrence, Schedule, SimpleItem, TaskItem,
    TeamAssignment, TemplateItem,
};
use crate::service::error::ItemError;

/// A Task's anchor for offset-driven scheduling. Exactly one source by construction —
/// this is `Item::validate()`'s "an item cannot both have a parent and reference an event"
/// rule expressed in the type rather than checked at runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TaskAnchor {
    #[default]
    None,
    Parent(String),
    SourceEvent(String),
}

impl TaskAnchor {
    /// Reads an existing item's anchor back off it. `Item::validate()` guarantees at most one
    /// of the two is set, so the `parent_item_id` arm winning here is a tiebreak that cannot
    /// fire — it is the type, not this ordering, that makes "both" unrepresentable.
    pub fn from_item(item: &Item) -> Self {
        match (item.parent_item_id(), item.source_event_id()) {
            (Some(parent), _) => TaskAnchor::Parent(parent),
            (None, Some(event)) => TaskAnchor::SourceEvent(event),
            (None, None) => TaskAnchor::None,
        }
    }

    fn parent_item_id(&self) -> Option<String> {
        match self {
            TaskAnchor::Parent(id) => Some(id.clone()),
            _ => None,
        }
    }

    fn source_event_id(&self) -> Option<String> {
        match self {
            TaskAnchor::SourceEvent(id) => Some(id.clone()),
            _ => None,
        }
    }
}

/// The only kind that can be completed, carry points/assignment, hold a priority, or
/// reference a source Event — mirroring `domain::item::TaskItem`.
#[derive(Debug, Default)]
pub struct NewTask {
    pub anchor: TaskAnchor,
    pub schedule: Schedule,
    pub due_offset_days: Option<i32>,
    pub priority: Option<i32>,
    pub complete: bool,
    /// Team-backed projects only, and admin-gated — `team_items` strips a non-admin's
    /// attempted value, and the personal branch has no slot for it at all. See root
    /// CLAUDE.md's Points section.
    pub assignment: TeamAssignment,
    /// Internal-only — never exposed via Smithy/CLI/MCP. Set exclusively by
    /// `service::item_series::get_or_materialize_occurrence`.
    pub series_id: Option<String>,
}

/// No `parent_item_id`: an Event structurally cannot be a child of anything.
#[derive(Debug, Default)]
pub struct NewEvent {
    pub schedule: Schedule,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    /// Internal-only, as `NewTask::series_id`.
    pub series_id: Option<String>,
}

/// A bare checkable name: no schedule, no recurrence, no `event_type`, no completion.
#[derive(Debug, Default)]
pub struct NewSimple {
    pub parent_item_id: Option<String>,
}

/// A Template *child* — a row in a template's subtree, which is itself Template-typed
/// (root CLAUDE.md's Domain Models section). Deliberately not `Default`, and its parent
/// is a plain `String` rather than an `Option`: a *root* template is a library artifact
/// that only `service::templates` may mint, so an unparented `NewTemplate` is a request
/// `items::create_item` would reject (`require_template_has_template_parent`) and is made
/// unconstructable here instead.
///
/// `event_type` on a template means "fire me when a matching item is created", not "this
/// occurrence's category" — see root CLAUDE.md's Events section.
#[derive(Debug)]
pub struct NewTemplate {
    pub parent_item_id: String,
    pub schedule: Schedule,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
}

#[derive(Debug)]
pub enum NewItemKind {
    Task(NewTask),
    Event(NewEvent),
    Simple(NewSimple),
    Template(NewTemplate),
}

impl NewItemKind {
    pub fn kind(&self) -> ItemKind {
        match self {
            NewItemKind::Task(_) => ItemKind::Task,
            NewItemKind::Event(_) => ItemKind::Event,
            NewItemKind::Simple(_) => ItemKind::Simple,
            NewItemKind::Template(_) => ItemKind::Template,
        }
    }

    /// The would-be parent, wherever this kind keeps one. An Event has none structurally,
    /// and a Task keeps its parent inside `TaskAnchor` alongside the `sourceEventId` it is
    /// mutually exclusive with — so "read the parent" is a per-variant question rather than
    /// one shared field, and the create/update paths ask it here to resolve the parent's own
    /// kind before building anything.
    pub(crate) fn parent_item_id(&self) -> Option<String> {
        match self {
            NewItemKind::Task(t) => t.anchor.parent_item_id(),
            NewItemKind::Event(_) => None,
            NewItemKind::Simple(s) => s.parent_item_id.clone(),
            NewItemKind::Template(t) => Some(t.parent_item_id.clone()),
        }
    }

    /// Mirrors `Item::complete()`, which reads `false` for every kind but `Task` for the
    /// same structural reason: no other payload has the field.
    pub(crate) fn complete(&self) -> bool {
        match self {
            NewItemKind::Task(t) => t.complete,
            _ => false,
        }
    }

    /// `None` for `Simple`, which has no dates at all — which is also why the
    /// scheduled-window ordering check its callers run can never fire on one.
    pub(crate) fn schedule(&self) -> Option<&Schedule> {
        match self {
            NewItemKind::Task(t) => Some(&t.schedule),
            NewItemKind::Event(e) => Some(&e.schedule),
            NewItemKind::Simple(_) => None,
            NewItemKind::Template(t) => Some(&t.schedule),
        }
    }

    /// A template's child subtree is itself Template-typed (root CLAUDE.md's Domain Models
    /// section), so any child of a Template is rewritten to `Template` regardless of what was
    /// asked for. `parent_item_id` is taken as an argument rather than read back off `self`
    /// because this only ever fires when the caller has already resolved a parent's kind, so
    /// the id is known to exist — which is what lets `NewTemplate` keep a non-optional parent.
    ///
    /// Everything a `TemplateItem` has no slot for is dropped here — a Task's
    /// `priority`/`complete`/`assignment`/`sourceEventId`/`series_id`. That is unchanged from
    /// the flat path, where the coercion produced an `ItemKind::Template` and the old
    /// `build_item_type`'s `Template` arm simply never read those fields.
    pub(crate) fn coerce_to_template(self, parent_item_id: String) -> Self {
        let (schedule, event_type, due_offset_days) = match self {
            NewItemKind::Task(t) => (t.schedule, None, t.due_offset_days),
            NewItemKind::Event(e) => (e.schedule, e.event_type, e.due_offset_days),
            NewItemKind::Simple(_) => (Schedule::default(), None, None),
            NewItemKind::Template(t) => (t.schedule, t.event_type, t.due_offset_days),
        };
        NewItemKind::Template(NewTemplate {
            parent_item_id,
            schedule,
            event_type,
            due_offset_days,
        })
    }
}

/// The envelope every kind carries, mirroring `domain::item::Item`'s own.
#[derive(Debug)]
pub struct NewItem {
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub kind: NewItemKind,
}

/// The one place that decides which of `Schedule`/`Recurrence`/`event_type` a kind actually
/// gets to carry, replacing the near-identical `build_item_type` pair that used to sit in
/// `service::items` and `service::team_items` (Stage 8 of docs/archived/typed-item-params-plan.md).
/// Merging them is the point of the whole plan: there is now exactly one answer to "what does
/// a Task store", and it is reached by matching the input's own variant rather than by
/// re-deriving a kind from a flat bag of `Option`s.
///
/// `team_assignment` is a parameter rather than being read off `NewTask::assignment` because
/// the stored value is not the requested one: `team_items` resolves the assignee and gates
/// points on project-admin authority first, and `items` (the personal branch) never stores an
/// assignment at all, so it passes `None`. See root CLAUDE.md's Points section.
pub(crate) fn build_item_type(
    kind: NewItemKind,
    team_assignment: Option<TeamAssignment>,
) -> ItemType {
    // Item-level recurrence is retired (Stage 10 core) — nothing can ever set
    // `pattern`/`basis` again, only `due_offset_days` survives here.
    fn recurrence(due_offset_days: Option<i32>) -> Recurrence {
        Recurrence {
            pattern: None,
            basis: None,
            due_offset_days,
        }
    }
    match kind {
        NewItemKind::Simple(s) => ItemType::Simple(SimpleItem {
            parent_item_id: s.parent_item_id,
        }),
        NewItemKind::Task(t) => ItemType::Task(TaskItem {
            parent_item_id: t.anchor.parent_item_id(),
            schedule: t.schedule,
            recurrence: recurrence(t.due_offset_days),
            team_assignment,
            source_event_id: t.anchor.source_event_id(),
            priority: t.priority,
            complete: t.complete,
            series_id: t.series_id,
        }),
        NewItemKind::Event(e) => ItemType::Event(EventItem {
            schedule: e.schedule,
            recurrence: recurrence(e.due_offset_days),
            event_type: e.event_type,
            series_id: e.series_id,
            // No input field carries these (see `NewEvent`'s own note and
            // `EventItem::google_event_id`'s doc comment) — only `service::calendar_sync`
            // writes them, directly onto an `Item` it builds itself rather than through this
            // funnel. `update_item`/`update_team_item`'s `current.google_event_id().is_some()`
            // guard means this is never even reached for an already-imported item's update.
            google_event_id: None,
            calendar_subscription_id: None,
        }),
        NewItemKind::Template(t) => ItemType::Template(TemplateItem {
            parent_item_id: Some(t.parent_item_id),
            schedule: t.schedule,
            recurrence: recurrence(t.due_offset_days),
            event_type: t.event_type,
        }),
    }
}

/// Update counterpart of `NewTask`. No `series_id`: an item's series membership is set
/// once at creation and carried forward unchanged, so there is nothing to express here
/// (root CLAUDE.md's Item series section).
#[derive(Debug, Default)]
pub struct EditTask {
    pub anchor: TaskAnchor,
    pub schedule: Schedule,
    pub due_offset_days: Option<i32>,
    pub priority: Option<i32>,
    pub complete: bool,
    pub assignment: TeamAssignment,
}

impl EditTask {
    /// Every Task-carried field read straight back off an existing item.
    ///
    /// This is the direct-overwrite convention (root CLAUDE.md's Events section: `priority`,
    /// `event_type` and `due_offset_days` are *cleared* by omission, so a caller must
    /// round-trip whatever it did not mean to touch) written once, for the handlers that only
    /// mean to change one field — a completion toggle, a batch priority set, a reparent. Each
    /// of those used to re-transcribe nineteen fields by hand, where forgetting one is a silent
    /// data loss rather than a compile error.
    ///
    /// Callers must have established the item is a Task first (`require_task`, or an already
    /// checked `complete()`): every field here reads through `Item`'s `Option`-returning
    /// delegation, so a non-Task would come back blank rather than erroring.
    pub fn from_item(item: &Item) -> Self {
        EditTask {
            anchor: TaskAnchor::from_item(item),
            schedule: Schedule {
                due_date: item.due_date(),
                has_due_time: item.has_due_time(),
                scheduled_date: item.scheduled_date(),
                has_scheduled_time: item.has_scheduled_time(),
                scheduled_end_date: item.scheduled_end_date(),
                has_end_time: item.has_end_time(),
            },
            due_offset_days: item.due_offset_days(),
            priority: item.priority(),
            complete: item.complete(),
            assignment: TeamAssignment {
                assigned_to_user_id: item.assigned_to_user_id(),
                points: item.points(),
            },
        }
    }
}

/// No `complete` field — `Item::validate()`'s "events cannot be marked complete" rule,
/// made unrepresentable. Same for `EditSimple`/`EditTemplate` below.
#[derive(Debug, Default)]
pub struct EditEvent {
    pub schedule: Schedule,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
}

#[derive(Debug, Default)]
pub struct EditSimple {
    pub parent_item_id: Option<String>,
}

/// Update counterpart of `NewTemplate`, and non-`Default` for the same reason.
#[derive(Debug)]
pub struct EditTemplate {
    pub parent_item_id: String,
    pub schedule: Schedule,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
}

#[derive(Debug)]
pub enum EditItemKind {
    Task(EditTask),
    Event(EditEvent),
    Simple(EditSimple),
    Template(EditTemplate),
}

impl EditItemKind {
    // No `kind()` counterpart to `NewItemKind`'s: every update path calls `into_new_kind`
    // before it needs a discriminant, and an accessor nothing calls is an accessor that
    // rots. `NewItemKind::kind()` is what the four create/update paths actually ask.

    /// `NewItemKind::complete`'s counterpart, and false for the same structural reason.
    pub(crate) fn complete(&self) -> bool {
        match self {
            EditItemKind::Task(t) => t.complete,
            _ => false,
        }
    }

    /// `NewItemKind::schedule`'s counterpart, needed before the stored item has been read —
    /// which is what `into_new_kind` waits on.
    pub(crate) fn schedule(&self) -> Option<&Schedule> {
        match self {
            EditItemKind::Task(t) => Some(&t.schedule),
            EditItemKind::Event(e) => Some(&e.schedule),
            EditItemKind::Simple(_) => None,
            EditItemKind::Template(t) => Some(&t.schedule),
        }
    }
}

#[derive(Debug)]
pub struct EditItem {
    pub project_id: String,
    pub item_id: String,
    pub name: String,
    pub description: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    /// Deliberately on the envelope rather than on `EditTask`, even though "depends on"
    /// is Task-only: clearing is allowed regardless of kind, since that's the only way
    /// rows on an item whose kind has since changed could ever be removed (see root
    /// CLAUDE.md's Item dependencies section). `None` means "leave dependencies alone";
    /// `Some(vec![])` clears them.
    pub depends_on_item_ids: Option<Vec<String>>,
    pub kind: EditItemKind,
}

impl EditItemKind {
    /// An edit is a create plus the one field a create's caller supplies and an edit's
    /// caller cannot: `series_id`. An item's series membership is set once at
    /// materialization and carried forward from the stored item (root CLAUDE.md's Item
    /// series section), which is exactly what `series_id` is here — so rather than a second
    /// near-identical `build_item_type`, the update paths fold that carried-forward value
    /// back in and reuse the create one. The `Simple` and `Template` arms drop it, as their
    /// payloads have no such field.
    pub(crate) fn into_new_kind(self, series_id: Option<String>) -> NewItemKind {
        match self {
            EditItemKind::Task(t) => NewItemKind::Task(NewTask {
                anchor: t.anchor,
                schedule: t.schedule,
                due_offset_days: t.due_offset_days,
                priority: t.priority,
                complete: t.complete,
                assignment: t.assignment,
                series_id,
            }),
            EditItemKind::Event(e) => NewItemKind::Event(NewEvent {
                schedule: e.schedule,
                event_type: e.event_type,
                due_offset_days: e.due_offset_days,
                series_id,
            }),
            EditItemKind::Simple(s) => NewItemKind::Simple(NewSimple {
                parent_item_id: s.parent_item_id,
            }),
            EditItemKind::Template(t) => NewItemKind::Template(NewTemplate {
                parent_item_id: t.parent_item_id,
                schedule: t.schedule,
                event_type: t.event_type,
                due_offset_days: t.due_offset_days,
            }),
        }
    }
}

// ---- the untyped boundary --------------------------------------------------------------
//
// Stage 7 of docs/archived/typed-item-params-plan.md. Two callers receive a request whose kind is
// *data* rather than a fact the code knows: `json_api::project_items` (an `itemType` field on
// the wire) and `service::import` (an `itemType` CSV column). Everything else in the codebase
// reaches the types above by construction. These helpers are what those two share, so the one
// place a caller can ask for an Event and attach `points` to it has a single answer.
//
// **A field the requested kind cannot hold is rejected, not dropped.** Before Stage 7 such a
// value was silently discarded and the request returned success, so a caller had no way to
// learn that part of what it sent went nowhere. It now fails with the API's ordinary client
// error naming the field and the kind — the same treatment `itemType` itself already gets,
// where smithy-rs rejects an unrecognized enum value at the deserialization boundary before
// any of this code runs (root CLAUDE.md's Events section).
//
// This is about *kind*, not *authority*. `points` set by a non-admin on a perfectly valid
// Task is still silently preserved-not-applied by `team_items` — a different rule with its
// own rationale (root CLAUDE.md's Points section), untouched here.
//
// Field names in these messages are the wire's own camelCase, which the PRL CSV format
// deliberately matches column-for-column (root CLAUDE.md's CSV import section), so one set of
// strings serves both callers.

/// Rejects a field the requested kind has no place to put.
pub(crate) fn reject_field<T>(
    kind: ItemKind,
    field: &str,
    value: &Option<T>,
) -> Result<(), ItemError> {
    if value.is_some() {
        return Err(ItemError::Invalid(format!(
            "{field} is not valid on {kind} items"
        )));
    }
    Ok(())
}

/// The boolean counterpart, which rejects only `true`.
///
/// A cross-kind boolean carrying `false` is accepted and ignored, and that is not a
/// convenience carve-out. `complete` is `@required` on `UpdateProjectItem`, and on the MCP
/// server's own `update_item` tool, so a caller renaming an Event has no way *not* to send it
/// — rejecting `false` would make editing a non-Task impossible. And `false` discards
/// nothing: `Some(false)` and `None` were already indistinguishable to every consumer of
/// these fields (see this module's header on `has_*_time`). `complete: true` on a non-Task is
/// a real request to do something the kind cannot do, and is rejected.
pub(crate) fn reject_flag(
    kind: ItemKind,
    field: &str,
    value: Option<bool>,
) -> Result<(), ItemError> {
    if value == Some(true) {
        return Err(ItemError::Invalid(format!(
            "{field} is not valid on {kind} items"
        )));
    }
    Ok(())
}

/// Fields no kind but `Task` can hold, checked in one place so each caller's Event/Simple/
/// Template arms can't drift apart on which of them they remember.
pub(crate) fn reject_task_only_fields(
    kind: ItemKind,
    complete: Option<bool>,
    priority: &Option<i32>,
    points: &Option<i32>,
    assigned_to_user_id: &Option<String>,
    source_event_id: &Option<String>,
) -> Result<(), ItemError> {
    reject_flag(kind, "complete", complete)?;
    reject_field(kind, "priority", priority)?;
    reject_field(kind, "points", points)?;
    reject_field(kind, "assignedToUserId", assigned_to_user_id)?;
    reject_field(kind, "sourceEventId", source_event_id)
}

/// A Simple item is a bare checkable name — no schedule, no `event_type`, no offset (root
/// CLAUDE.md's Domain Models section). Its rejections have no counterpart on any other kind.
pub(crate) fn reject_simple_only_fields(
    event_type: &Option<String>,
    due_offset_days: &Option<i32>,
    schedule: &Schedule,
) -> Result<(), ItemError> {
    let kind = ItemKind::Simple;
    reject_field(kind, "eventType", event_type)?;
    reject_field(kind, "dueOffsetDays", due_offset_days)?;
    reject_field(kind, "dueDate", &schedule.due_date)?;
    reject_field(kind, "scheduledDate", &schedule.scheduled_date)?;
    reject_field(kind, "scheduledEndDate", &schedule.scheduled_end_date)?;
    reject_flag(kind, "hasDueTime", Some(schedule.has_due_time))?;
    reject_flag(kind, "hasScheduledTime", Some(schedule.has_scheduled_time))?;
    reject_flag(kind, "hasEndTime", Some(schedule.has_end_time))
}

/// A Task's single anchor, resolved from the two wire fields. The "both" case used to reach
/// `Item::validate()` and be rejected there; `TaskAnchor` cannot express it, so the same
/// rejection is raised here with the same wording — as it already is in
/// `web_ui::project_tasks::reparent_edit` (Stage 5).
pub(crate) fn task_anchor_from_fields(
    parent_item_id: Option<String>,
    source_event_id: Option<String>,
) -> Result<TaskAnchor, ItemError> {
    match (parent_item_id, source_event_id) {
        (Some(_), Some(_)) => Err(ItemError::Invalid(
            "an item cannot both have a parent and reference an event".to_string(),
        )),
        (Some(parent), None) => Ok(TaskAnchor::Parent(parent)),
        (None, Some(event)) => Ok(TaskAnchor::SourceEvent(event)),
        (None, None) => Ok(TaskAnchor::None),
    }
}

/// A Template child's parent. `NewTemplate`/`EditTemplate` take a non-optional `String`
/// because a *root* template is `service::templates`' business alone — the unparented case is
/// unconstructable, so `items::require_template_has_template_parent`'s wording is raised here
/// instead.
pub(crate) fn template_parent(parent_item_id: Option<String>) -> Result<String, ItemError> {
    parent_item_id.ok_or_else(|| {
        ItemError::Invalid(
            "item_type Template can only be set via the template creation flow".to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{ItemType, Recurrence, TaskItem};
    use chrono::TimeZone;

    fn dt(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn schedule() -> Schedule {
        Schedule {
            due_date: Some(dt(1_000)),
            has_due_time: true,
            scheduled_date: Some(dt(500)),
            has_scheduled_time: true,
            scheduled_end_date: Some(dt(900)),
            has_end_time: false,
        }
    }

    fn new_item(kind: NewItemKind) -> NewItem {
        NewItem {
            project_id: "p1".into(),
            name: "n".into(),
            description: Some("d".into()),
            timezone_offset_minutes: Some(-300),
            kind,
        }
    }

    fn edit_item(kind: EditItemKind) -> EditItem {
        EditItem {
            project_id: "p1".into(),
            item_id: "i1".into(),
            name: "n".into(),
            description: None,
            timezone_offset_minutes: None,
            depends_on_item_ids: None,
            kind,
        }
    }

    /// Builds the payload the personal branch would store — `items::create_item` passes
    /// `None` for the assignment, so this is that call with the mock repos left out.
    fn built(kind: NewItemKind) -> ItemType {
        build_item_type(kind, None)
    }

    #[test]
    fn new_task_carries_every_task_only_field_through() {
        let built = build_item_type(
            NewItemKind::Task(NewTask {
                anchor: TaskAnchor::Parent("parent".into()),
                schedule: schedule(),
                due_offset_days: Some(-3),
                priority: Some(2),
                complete: true,
                // Ignored by `build_item_type` — `team_items` resolves and gates the stored
                // assignment and passes it separately, which is what the argument below is.
                assignment: TeamAssignment::default(),
                series_id: Some("s1".into()),
            }),
            Some(TeamAssignment {
                assigned_to_user_id: Some("u1".into()),
                points: Some(5),
            }),
        );

        let ItemType::Task(task) = built else {
            panic!("expected a Task payload");
        };
        assert_eq!(task.parent_item_id.as_deref(), Some("parent"));
        assert_eq!(task.source_event_id, None);
        assert_eq!(task.recurrence.due_offset_days, Some(-3));
        assert_eq!(task.priority, Some(2));
        assert!(task.complete);
        assert_eq!(
            task.team_assignment
                .as_ref()
                .unwrap()
                .assigned_to_user_id
                .as_deref(),
            Some("u1")
        );
        assert_eq!(task.team_assignment.as_ref().unwrap().points, Some(5));
        assert_eq!(task.series_id.as_deref(), Some("s1"));
        assert_eq!(task.schedule.due_date, Some(dt(1_000)));
        assert!(task.schedule.has_due_time);
        assert!(!task.schedule.has_end_time);
        // Item-level recurrence is retired — nothing can set these again.
        assert_eq!(task.recurrence.pattern, None);
        assert_eq!(task.recurrence.basis, None);
    }

    /// The envelope is the caller's business, not `build_item_type`'s — these are the
    /// fields every kind shares, which is why they sit on `NewItem` rather than in any
    /// variant.
    #[test]
    fn the_envelope_carries_the_kind_agnostic_fields() {
        let new = new_item(NewItemKind::Task(NewTask::default()));
        assert_eq!(new.project_id, "p1");
        assert_eq!(new.name, "n");
        assert_eq!(new.description.as_deref(), Some("d"));
        assert_eq!(new.timezone_offset_minutes, Some(-300));
        assert_eq!(new.kind.kind(), ItemKind::Task);
    }

    /// `TaskAnchor` is what makes `Item::validate()`'s "cannot both have a parent and
    /// reference an event" rule unrepresentable rather than merely rejected.
    #[test]
    fn task_anchor_sets_exactly_one_of_parent_or_source_event() {
        fn anchor_of(anchor: TaskAnchor) -> (Option<String>, Option<String>) {
            let ItemType::Task(task) = built(NewItemKind::Task(NewTask {
                anchor,
                ..Default::default()
            })) else {
                panic!("expected a Task payload");
            };
            (task.parent_item_id, task.source_event_id)
        }

        assert_eq!(
            anchor_of(TaskAnchor::Parent("parent".into())),
            (Some("parent".to_string()), None)
        );
        assert_eq!(
            anchor_of(TaskAnchor::SourceEvent("ev".into())),
            (None, Some("ev".to_string()))
        );
        assert_eq!(anchor_of(TaskAnchor::None), (None, None));
    }

    #[test]
    fn new_event_leaves_every_task_only_field_unset() {
        let built = built(NewItemKind::Event(NewEvent {
            schedule: schedule(),
            event_type: Some("rain".into()),
            due_offset_days: None,
            series_id: Some("s1".into()),
        }));

        let ItemType::Event(event) = built else {
            panic!("expected an Event payload");
        };
        assert_eq!(event.event_type.as_deref(), Some("rain"));
        assert_eq!(event.schedule.scheduled_date, Some(dt(500)));
        assert_eq!(event.series_id.as_deref(), Some("s1"));
        // Only `service::calendar_sync` writes these, and it builds its `Item` directly.
        assert_eq!(event.google_event_id, None);
        assert_eq!(event.calendar_subscription_id, None);
        // `EventItem` has no field for completion, priority, points, an assignee, a parent
        // or a source event — which is the whole point, so there is nothing to assert
        // `None` on. `Item`'s own delegation is what reports them absent.
    }

    #[test]
    fn new_simple_carries_nothing_but_its_parent() {
        let built = built(NewItemKind::Simple(NewSimple {
            parent_item_id: Some("parent".into()),
        }));

        let ItemType::Simple(simple) = built else {
            panic!("expected a Simple payload");
        };
        assert_eq!(simple.parent_item_id.as_deref(), Some("parent"));
    }

    #[test]
    fn new_template_carries_its_event_type_and_parent() {
        let built = built(NewItemKind::Template(NewTemplate {
            parent_item_id: "root".into(),
            schedule: schedule(),
            event_type: Some("rain".into()),
            due_offset_days: Some(-7),
        }));

        let ItemType::Template(template) = built else {
            panic!("expected a Template payload");
        };
        assert_eq!(template.parent_item_id.as_deref(), Some("root"));
        assert_eq!(template.event_type.as_deref(), Some("rain"));
        assert_eq!(template.recurrence.due_offset_days, Some(-7));
        assert_eq!(template.schedule.due_date, Some(dt(1_000)));
    }

    /// An edit is a create plus the carried-forward `series_id`, which is what lets both
    /// sides share one `build_item_type`.
    #[test]
    fn edit_task_round_trips_completion_and_assignment() {
        let edit = edit_item(EditItemKind::Task(EditTask {
            anchor: TaskAnchor::None,
            schedule: schedule(),
            due_offset_days: None,
            priority: Some(1),
            complete: true,
            assignment: TeamAssignment {
                assigned_to_user_id: Some("u1".into()),
                points: Some(3),
            },
        }));
        assert_eq!(edit.item_id, "i1");

        let kind = edit.kind.into_new_kind(Some("s1".into()));
        assert!(kind.complete());
        let NewItemKind::Task(ref task) = kind else {
            panic!("expected a Task input");
        };
        // The requested assignment survives the conversion; `team_items` is what decides
        // whether it survives *authority* (root CLAUDE.md's Points section).
        assert_eq!(task.assignment.points, Some(3));
        assert_eq!(task.assignment.assigned_to_user_id.as_deref(), Some("u1"));

        let ItemType::Task(task) = built(kind) else {
            panic!("expected a Task payload");
        };
        assert!(task.complete);
        assert_eq!(task.priority, Some(1));
        assert_eq!(task.series_id.as_deref(), Some("s1"));
    }

    /// The update side of "events cannot be marked complete" / "simple items cannot be
    /// marked complete": no variant but `Task` has a `complete` field, so an edit of any
    /// other kind can only ever report `false`.
    #[test]
    fn non_task_edits_can_never_be_complete() {
        assert!(
            !EditItemKind::Event(EditEvent {
                schedule: schedule(),
                event_type: Some("rain".into()),
                due_offset_days: None,
            })
            .complete()
        );
        assert!(!EditItemKind::Simple(EditSimple::default()).complete());
        assert!(
            !EditItemKind::Template(EditTemplate {
                parent_item_id: "root".into(),
                schedule: Schedule::default(),
                event_type: None,
                due_offset_days: None,
            })
            .complete()
        );
    }

    /// A `Simple` edit drops the carried-forward `series_id` rather than smuggling it —
    /// `SimpleItem` has no such field, and neither does `TemplateItem`.
    #[test]
    fn kinds_without_a_series_field_drop_the_carried_forward_id() {
        let kind = EditItemKind::Simple(EditSimple::default()).into_new_kind(Some("s1".into()));
        assert!(matches!(built(kind), ItemType::Simple(_)));

        let kind = EditItemKind::Template(EditTemplate {
            parent_item_id: "root".into(),
            schedule: Schedule::default(),
            event_type: None,
            due_offset_days: None,
        })
        .into_new_kind(Some("s1".into()));
        assert!(matches!(built(kind), ItemType::Template(_)));
    }

    /// Dependencies live on the envelope, not on `EditTask`, because clearing them must
    /// stay possible for an item whose kind has since changed.
    #[test]
    fn depends_on_rides_the_envelope_for_every_kind() {
        let mut edit = edit_item(EditItemKind::Simple(EditSimple::default()));
        edit.depends_on_item_ids = Some(vec![]);
        assert_eq!(edit.depends_on_item_ids, Some(vec![]));
    }

    /// A child of a Template is Template-typed whatever it asked to be, and everything a
    /// `TemplateItem` has no slot for is dropped in the rewrite.
    #[test]
    fn coercing_a_task_to_a_template_keeps_only_what_a_template_can_hold() {
        let coerced = NewItemKind::Task(NewTask {
            anchor: TaskAnchor::Parent("root".into()),
            schedule: schedule(),
            due_offset_days: Some(-7),
            priority: Some(2),
            complete: true,
            assignment: TeamAssignment {
                assigned_to_user_id: Some("u1".into()),
                points: Some(5),
            },
            series_id: Some("s1".into()),
        })
        .coerce_to_template("root".into());

        let ItemType::Template(template) = built(coerced) else {
            panic!("expected a Template payload");
        };
        assert_eq!(template.parent_item_id.as_deref(), Some("root"));
        assert_eq!(template.schedule.due_date, Some(dt(1_000)));
        assert_eq!(template.recurrence.due_offset_days, Some(-7));
        // A Task carries no `event_type`, so there is nothing to carry over.
        assert_eq!(template.event_type, None);
    }

    /// An Event's `event_type` does survive the coercion — both kinds have the field, and
    /// on a Template it means "fire me when a matching item is created" (root CLAUDE.md's
    /// Events section).
    #[test]
    fn coercing_an_event_to_a_template_keeps_its_event_type() {
        let coerced = NewItemKind::Event(NewEvent {
            schedule: schedule(),
            event_type: Some("rain".into()),
            due_offset_days: None,
            series_id: Some("s1".into()),
        })
        .coerce_to_template("root".into());

        let ItemType::Template(template) = built(coerced) else {
            panic!("expected a Template payload");
        };
        assert_eq!(template.event_type.as_deref(), Some("rain"));
    }

    /// A `Simple` has no dates at all, so the coercion has nothing to carry and must not
    /// invent any.
    #[test]
    fn coercing_a_simple_to_a_template_leaves_it_dateless() {
        let coerced = NewItemKind::Simple(NewSimple {
            parent_item_id: Some("root".into()),
        })
        .coerce_to_template("root".into());

        let ItemType::Template(template) = built(coerced) else {
            panic!("expected a Template payload");
        };
        assert_eq!(template.schedule, Schedule::default());
        assert_eq!(template.recurrence.due_offset_days, None);
        assert_eq!(template.event_type, None);
    }

    /// `parent_item_id` is a per-variant question, not one shared field — an Event has
    /// none structurally, and a Task keeps its parent inside `TaskAnchor`.
    #[test]
    fn parent_item_id_reads_through_whichever_variant_holds_one() {
        assert_eq!(
            NewItemKind::Task(NewTask {
                anchor: TaskAnchor::Parent("p".into()),
                ..Default::default()
            })
            .parent_item_id()
            .as_deref(),
            Some("p")
        );
        assert_eq!(
            NewItemKind::Task(NewTask {
                anchor: TaskAnchor::SourceEvent("ev".into()),
                ..Default::default()
            })
            .parent_item_id(),
            None
        );
        assert_eq!(
            NewItemKind::Event(NewEvent::default()).parent_item_id(),
            None
        );
        assert_eq!(
            NewItemKind::Simple(NewSimple {
                parent_item_id: Some("p".into()),
            })
            .parent_item_id()
            .as_deref(),
            Some("p")
        );
        assert_eq!(
            NewItemKind::Template(NewTemplate {
                parent_item_id: "p".into(),
                schedule: Schedule::default(),
                event_type: None,
                due_offset_days: None,
            })
            .parent_item_id()
            .as_deref(),
            Some("p")
        );
    }

    /// A `Simple` has no `Schedule`, which is why its callers' scheduled-window ordering
    /// check can never fire on one.
    #[test]
    fn only_simple_has_no_schedule() {
        assert!(NewItemKind::Task(NewTask::default()).schedule().is_some());
        assert!(NewItemKind::Event(NewEvent::default()).schedule().is_some());
        assert!(
            NewItemKind::Simple(NewSimple::default())
                .schedule()
                .is_none()
        );
        assert!(
            NewItemKind::Template(NewTemplate {
                parent_item_id: "p".into(),
                schedule: Schedule::default(),
                event_type: None,
                due_offset_days: None,
            })
            .schedule()
            .is_some()
        );
    }

    fn task_item(f: impl FnOnce(&mut TaskItem)) -> Item {
        let mut task = TaskItem {
            schedule: schedule(),
            recurrence: Recurrence::default(),
            ..Default::default()
        };
        f(&mut task);
        Item {
            id: "i1".into(),
            name: "n".into(),
            description: Some("d".into()),
            item_type: ItemType::Task(task),
            ..Item::default()
        }
    }

    #[test]
    fn task_anchor_from_item_reads_back_whichever_anchor_is_set() {
        assert_eq!(
            TaskAnchor::from_item(&task_item(|t| t.parent_item_id = Some("p".into()))),
            TaskAnchor::Parent("p".into())
        );
        assert_eq!(
            TaskAnchor::from_item(&task_item(|t| t.source_event_id = Some("ev".into()))),
            TaskAnchor::SourceEvent("ev".into())
        );
        assert_eq!(TaskAnchor::from_item(&task_item(|_| {})), TaskAnchor::None);
    }

    /// The round-trip the completion toggles, the batch actions and the reparent all depend on:
    /// every field a plain edit did not mean to touch must survive, since omission *clears*
    /// (root CLAUDE.md's Events section).
    #[test]
    fn edit_task_from_item_round_trips_every_task_field() {
        let item = task_item(|t| {
            t.parent_item_id = Some("p".into());
            t.priority = Some(2);
            t.complete = true;
            t.recurrence.due_offset_days = Some(-3);
            t.team_assignment = Some(TeamAssignment {
                assigned_to_user_id: Some("u1".into()),
                points: Some(5),
            });
        });

        let edit = EditItem {
            project_id: "p1".into(),
            item_id: "i1".into(),
            name: item.name.clone(),
            description: item.description.clone(),
            timezone_offset_minutes: Some(-300),
            depends_on_item_ids: None,
            kind: EditItemKind::Task(EditTask::from_item(&item)),
        };

        let ItemType::Task(task) = build_item_type(
            edit.kind.into_new_kind(None),
            Some(TeamAssignment {
                assigned_to_user_id: item.assigned_to_user_id(),
                points: item.points(),
            }),
        ) else {
            panic!("expected a Task payload");
        };
        assert_eq!(task.parent_item_id.as_deref(), Some("p"));
        assert_eq!(task.source_event_id, None);
        assert_eq!(task.priority, Some(2));
        assert!(task.complete);
        assert_eq!(task.recurrence.due_offset_days, Some(-3));
        let assignment = task.team_assignment.as_ref().unwrap();
        assert_eq!(assignment.assigned_to_user_id.as_deref(), Some("u1"));
        assert_eq!(assignment.points, Some(5));
        assert_eq!(task.schedule.due_date, Some(dt(1_000)));
        assert!(task.schedule.has_due_time);
        assert_eq!(task.schedule.scheduled_date, Some(dt(500)));
        assert!(task.schedule.has_scheduled_time);
        assert_eq!(task.schedule.scheduled_end_date, Some(dt(900)));
        assert!(!task.schedule.has_end_time);
    }

    /// A completion toggle is `from_item` plus one overlaid field — nothing else may move.
    #[test]
    fn overlaying_one_field_leaves_the_rest_of_the_round_trip_intact() {
        let item = task_item(|t| {
            t.complete = false;
            t.priority = Some(4);
        });
        let mut task = EditTask::from_item(&item);
        task.complete = true;
        assert!(task.complete);
        assert_eq!(task.priority, Some(4));
        assert_eq!(task.schedule.due_date, Some(dt(1_000)));
    }
}
