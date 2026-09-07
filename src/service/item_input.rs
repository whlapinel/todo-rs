//! Kind-typed inputs for the item create/update funnel.
//!
//! These are the input counterpart of `domain::item::ItemType` — one variant per
//! `ItemKind`, each carrying only the fields that kind can legitimately have. They exist
//! because `CreateProjectItemParams`/`UpdateProjectItemParams` are flat, kind-agnostic
//! property bags, and only *one* of their 21/26 construction sites
//! (`json_api::project_items`) actually receives untyped input. Every other caller knows
//! the kind statically and was spelling out fields that its kind can never carry — where
//! a wrong one is silently dropped rather than rejected. See
//! `docs/typed-item-params-plan.md` for the full rationale and staging.
//!
//! Deliberately reuses the domain's own `Schedule` and `TeamAssignment` structs rather
//! than defining input twins: the shapes are identical, and mirroring them exactly is the
//! point. `Schedule`'s `has_*_time` flags are plain `bool` here where the flat params use
//! `Option<bool>` — the flat params only ever `unwrap_or(false)` them, so there was never
//! a third state to represent.
//!
//! `google_event_id`/`calendar_subscription_id` deliberately have no input field on
//! `NewEvent`, matching today: `service::calendar_sync` writes them by constructing an
//! `Item` directly rather than going through this funnel, and `items::build_item_type`
//! already hardcodes both to `None`.

use crate::domain::item::{Item, ItemKind, Schedule, TeamAssignment};
use crate::service::project_items::{CreateProjectItemParams, UpdateProjectItemParams};

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

impl From<NewItem> for CreateProjectItemParams {
    fn from(new: NewItem) -> Self {
        let base = CreateProjectItemParams {
            project_id: new.project_id,
            name: new.name,
            description: new.description,
            timezone_offset_minutes: new.timezone_offset_minutes,
            item_type: Some(new.kind.kind()),
            ..Default::default()
        };
        match new.kind {
            NewItemKind::Task(t) => CreateProjectItemParams {
                due_date: t.schedule.due_date,
                has_due_time: Some(t.schedule.has_due_time),
                scheduled_date: t.schedule.scheduled_date,
                has_scheduled_time: Some(t.schedule.has_scheduled_time),
                scheduled_end_date: t.schedule.scheduled_end_date,
                has_end_time: Some(t.schedule.has_end_time),
                parent_item_id: t.anchor.parent_item_id(),
                source_event_id: t.anchor.source_event_id(),
                due_offset_days: t.due_offset_days,
                priority: t.priority,
                complete: Some(t.complete),
                assigned_to_user_id: t.assignment.assigned_to_user_id,
                points: t.assignment.points,
                series_id: t.series_id,
                ..base
            },
            NewItemKind::Event(e) => CreateProjectItemParams {
                due_date: e.schedule.due_date,
                has_due_time: Some(e.schedule.has_due_time),
                scheduled_date: e.schedule.scheduled_date,
                has_scheduled_time: Some(e.schedule.has_scheduled_time),
                scheduled_end_date: e.schedule.scheduled_end_date,
                has_end_time: Some(e.schedule.has_end_time),
                event_type: e.event_type,
                due_offset_days: e.due_offset_days,
                series_id: e.series_id,
                ..base
            },
            NewItemKind::Simple(s) => CreateProjectItemParams {
                parent_item_id: s.parent_item_id,
                ..base
            },
            NewItemKind::Template(t) => CreateProjectItemParams {
                due_date: t.schedule.due_date,
                has_due_time: Some(t.schedule.has_due_time),
                scheduled_date: t.schedule.scheduled_date,
                has_scheduled_time: Some(t.schedule.has_scheduled_time),
                scheduled_end_date: t.schedule.scheduled_end_date,
                has_end_time: Some(t.schedule.has_end_time),
                parent_item_id: Some(t.parent_item_id),
                event_type: t.event_type,
                due_offset_days: t.due_offset_days,
                ..base
            },
        }
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
    pub fn kind(&self) -> ItemKind {
        match self {
            EditItemKind::Task(_) => ItemKind::Task,
            EditItemKind::Event(_) => ItemKind::Event,
            EditItemKind::Simple(_) => ItemKind::Simple,
            EditItemKind::Template(_) => ItemKind::Template,
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

impl From<EditItem> for UpdateProjectItemParams {
    fn from(edit: EditItem) -> Self {
        let base = UpdateProjectItemParams {
            project_id: edit.project_id,
            item_id: edit.item_id,
            name: edit.name,
            description: edit.description,
            timezone_offset_minutes: edit.timezone_offset_minutes,
            depends_on_item_ids: edit.depends_on_item_ids,
            item_type: Some(edit.kind.kind()),
            ..Default::default()
        };
        match edit.kind {
            EditItemKind::Task(t) => UpdateProjectItemParams {
                due_date: t.schedule.due_date,
                has_due_time: Some(t.schedule.has_due_time),
                scheduled_date: t.schedule.scheduled_date,
                has_scheduled_time: Some(t.schedule.has_scheduled_time),
                scheduled_end_date: t.schedule.scheduled_end_date,
                has_end_time: Some(t.schedule.has_end_time),
                parent_item_id: t.anchor.parent_item_id(),
                source_event_id: t.anchor.source_event_id(),
                due_offset_days: t.due_offset_days,
                priority: t.priority,
                complete: t.complete,
                assigned_to_user_id: t.assignment.assigned_to_user_id,
                points: t.assignment.points,
                ..base
            },
            EditItemKind::Event(e) => UpdateProjectItemParams {
                due_date: e.schedule.due_date,
                has_due_time: Some(e.schedule.has_due_time),
                scheduled_date: e.schedule.scheduled_date,
                has_scheduled_time: Some(e.schedule.has_scheduled_time),
                scheduled_end_date: e.schedule.scheduled_end_date,
                has_end_time: Some(e.schedule.has_end_time),
                event_type: e.event_type,
                due_offset_days: e.due_offset_days,
                ..base
            },
            EditItemKind::Simple(s) => UpdateProjectItemParams {
                parent_item_id: s.parent_item_id,
                ..base
            },
            EditItemKind::Template(t) => UpdateProjectItemParams {
                due_date: t.schedule.due_date,
                has_due_time: Some(t.schedule.has_due_time),
                scheduled_date: t.schedule.scheduled_date,
                has_scheduled_time: Some(t.schedule.has_scheduled_time),
                scheduled_end_date: t.schedule.scheduled_end_date,
                has_end_time: Some(t.schedule.has_end_time),
                parent_item_id: Some(t.parent_item_id),
                event_type: t.event_type,
                due_offset_days: t.due_offset_days,
                ..base
            },
        }
    }
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

    #[test]
    fn new_task_carries_every_task_only_field_through() {
        let p: CreateProjectItemParams = new_item(NewItemKind::Task(NewTask {
            anchor: TaskAnchor::Parent("parent".into()),
            schedule: schedule(),
            due_offset_days: Some(-3),
            priority: Some(2),
            complete: true,
            assignment: TeamAssignment {
                assigned_to_user_id: Some("u1".into()),
                points: Some(5),
            },
            series_id: Some("s1".into()),
        }))
        .into();

        assert_eq!(p.item_type, Some(ItemKind::Task));
        assert_eq!(p.parent_item_id.as_deref(), Some("parent"));
        assert_eq!(p.source_event_id, None);
        assert_eq!(p.due_offset_days, Some(-3));
        assert_eq!(p.priority, Some(2));
        assert_eq!(p.complete, Some(true));
        assert_eq!(p.assigned_to_user_id.as_deref(), Some("u1"));
        assert_eq!(p.points, Some(5));
        assert_eq!(p.series_id.as_deref(), Some("s1"));
        assert_eq!(p.due_date, Some(dt(1_000)));
        assert_eq!(p.has_due_time, Some(true));
        assert_eq!(p.has_end_time, Some(false));
        // A Task has no `event_type` slot to fill.
        assert_eq!(p.event_type, None);
        // Envelope.
        assert_eq!(p.project_id, "p1");
        assert_eq!(p.description.as_deref(), Some("d"));
        assert_eq!(p.timezone_offset_minutes, Some(-300));
    }

    /// `TaskAnchor` is what makes `Item::validate()`'s "cannot both have a parent and
    /// reference an event" rule unrepresentable rather than merely rejected.
    #[test]
    fn task_anchor_sets_exactly_one_of_parent_or_source_event() {
        let parent: CreateProjectItemParams = new_item(NewItemKind::Task(NewTask {
            anchor: TaskAnchor::Parent("parent".into()),
            ..Default::default()
        }))
        .into();
        assert_eq!(parent.parent_item_id.as_deref(), Some("parent"));
        assert_eq!(parent.source_event_id, None);

        let event: CreateProjectItemParams = new_item(NewItemKind::Task(NewTask {
            anchor: TaskAnchor::SourceEvent("ev".into()),
            ..Default::default()
        }))
        .into();
        assert_eq!(event.parent_item_id, None);
        assert_eq!(event.source_event_id.as_deref(), Some("ev"));

        let none: CreateProjectItemParams = new_item(NewItemKind::Task(NewTask::default())).into();
        assert_eq!(none.parent_item_id, None);
        assert_eq!(none.source_event_id, None);
    }

    #[test]
    fn new_event_leaves_every_task_only_field_unset() {
        let p: CreateProjectItemParams = new_item(NewItemKind::Event(NewEvent {
            schedule: schedule(),
            event_type: Some("rain".into()),
            due_offset_days: None,
            series_id: Some("s1".into()),
        }))
        .into();

        assert_eq!(p.item_type, Some(ItemKind::Event));
        assert_eq!(p.event_type.as_deref(), Some("rain"));
        assert_eq!(p.scheduled_date, Some(dt(500)));
        assert_eq!(p.series_id.as_deref(), Some("s1"));
        // None of these have a field on `NewEvent` to come from.
        assert_eq!(p.complete, None);
        assert_eq!(p.priority, None);
        assert_eq!(p.points, None);
        assert_eq!(p.assigned_to_user_id, None);
        assert_eq!(p.parent_item_id, None);
        assert_eq!(p.source_event_id, None);
    }

    #[test]
    fn new_simple_carries_nothing_but_its_parent() {
        let p: CreateProjectItemParams = new_item(NewItemKind::Simple(NewSimple {
            parent_item_id: Some("parent".into()),
        }))
        .into();

        assert_eq!(p.item_type, Some(ItemKind::Simple));
        assert_eq!(p.parent_item_id.as_deref(), Some("parent"));
        assert_eq!(p.complete, None);
        assert_eq!(p.due_date, None);
        assert_eq!(p.scheduled_date, None);
        assert_eq!(p.event_type, None);
        assert_eq!(p.due_offset_days, None);
        assert_eq!(p.priority, None);
        assert_eq!(p.series_id, None);
    }

    #[test]
    fn new_template_carries_its_event_type_and_parent() {
        let p: CreateProjectItemParams = new_item(NewItemKind::Template(NewTemplate {
            parent_item_id: "root".into(),
            schedule: schedule(),
            event_type: Some("rain".into()),
            due_offset_days: Some(-7),
        }))
        .into();

        assert_eq!(p.item_type, Some(ItemKind::Template));
        assert_eq!(p.parent_item_id.as_deref(), Some("root"));
        assert_eq!(p.event_type.as_deref(), Some("rain"));
        assert_eq!(p.due_offset_days, Some(-7));
        assert_eq!(p.complete, None);
        assert_eq!(p.priority, None);
        assert_eq!(p.series_id, None);
    }

    #[test]
    fn edit_task_round_trips_completion_and_assignment() {
        let p: UpdateProjectItemParams = edit_item(EditItemKind::Task(EditTask {
            anchor: TaskAnchor::None,
            schedule: schedule(),
            due_offset_days: None,
            priority: Some(1),
            complete: true,
            assignment: TeamAssignment {
                assigned_to_user_id: Some("u1".into()),
                points: Some(3),
            },
        }))
        .into();

        assert_eq!(p.item_id, "i1");
        assert!(p.complete);
        assert_eq!(p.priority, Some(1));
        assert_eq!(p.points, Some(3));
        assert_eq!(p.assigned_to_user_id.as_deref(), Some("u1"));
    }

    /// The update side of "events cannot be marked complete" / "simple items cannot be
    /// marked complete": neither variant has a `complete` field, so the conversion can
    /// only ever produce `false`.
    #[test]
    fn non_task_edits_can_never_be_complete() {
        let event: UpdateProjectItemParams = edit_item(EditItemKind::Event(EditEvent {
            schedule: schedule(),
            event_type: Some("rain".into()),
            due_offset_days: None,
        }))
        .into();
        assert!(!event.complete);

        let simple: UpdateProjectItemParams =
            edit_item(EditItemKind::Simple(EditSimple::default())).into();
        assert!(!simple.complete);

        let template: UpdateProjectItemParams = edit_item(EditItemKind::Template(EditTemplate {
            parent_item_id: "root".into(),
            schedule: Schedule::default(),
            event_type: None,
            due_offset_days: None,
        }))
        .into();
        assert!(!template.complete);
    }

    /// Dependencies live on the envelope, not on `EditTask`, because clearing them must
    /// stay possible for an item whose kind has since changed.
    #[test]
    fn depends_on_rides_the_envelope_for_every_kind() {
        let mut edit = edit_item(EditItemKind::Simple(EditSimple::default()));
        edit.depends_on_item_ids = Some(vec![]);
        let p: UpdateProjectItemParams = edit.into();
        assert_eq!(p.depends_on_item_ids, Some(vec![]));
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

        let p: UpdateProjectItemParams = EditItem {
            project_id: "p1".into(),
            item_id: "i1".into(),
            name: item.name.clone(),
            description: item.description.clone(),
            timezone_offset_minutes: Some(-300),
            depends_on_item_ids: None,
            kind: EditItemKind::Task(EditTask::from_item(&item)),
        }
        .into();

        assert_eq!(p.item_type, Some(ItemKind::Task));
        assert_eq!(p.parent_item_id.as_deref(), Some("p"));
        assert_eq!(p.source_event_id, None);
        assert_eq!(p.priority, Some(2));
        assert!(p.complete);
        assert_eq!(p.due_offset_days, Some(-3));
        assert_eq!(p.assigned_to_user_id.as_deref(), Some("u1"));
        assert_eq!(p.points, Some(5));
        assert_eq!(p.due_date, Some(dt(1_000)));
        assert_eq!(p.has_due_time, Some(true));
        assert_eq!(p.scheduled_date, Some(dt(500)));
        assert_eq!(p.has_scheduled_time, Some(true));
        assert_eq!(p.scheduled_end_date, Some(dt(900)));
        assert_eq!(p.has_end_time, Some(false));
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
