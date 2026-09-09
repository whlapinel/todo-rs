use chrono::{DateTime, Utc};

use crate::domain::item::ItemKind;

/// The recurrence rule + anchor + static fields for a browsable recurring
/// series — see docs/recurring-events-virtual-occurrences-rough-plan.md's staged
/// breakdown. Originally Event-only (stage 2, as `EventSeries`); generalized to
/// also cover Task series at stage 7a via the `item_type` field, backed by the
/// `item_series` table (renamed from `series` — see that migration for the
/// data carried forward). Distinct from `Item`'s own `recurrence`/`recurrence_basis`
/// auto-advance-on-read mechanism (see CLAUDE.md's Recurrence section): that model
/// conflates "the rule" and "the currently active instance" into one row, whereas a
/// series has no single date of its own — just a rule an occurrence date is computed
/// against via `domain::recurrence::occurrences_between`.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemSeries {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    /// Raw English pattern, parsed via `domain::recurrence::parse` — same
    /// convention as `Item::recurrence`, not a pre-parsed `RecurrenceRule` column.
    pub recurrence: String,
    pub anchor_date: DateTime<Utc>,
    /// Restricted to `Task`/`Event` at the service layer (stage 7b) — `Template`/
    /// `Simple` are not valid series kinds. Every series created before stage 7a
    /// (when this field didn't exist) carries `Event`, via the migration's
    /// `DEFAULT 'EVENT'`/copy-with-'EVENT' — the only kind that existed then.
    pub item_type: ItemKind,
    /// Stage 9: the most recent occurrence date a Task-typed series has settled
    /// (completed or skipped) — not user-settable, only ever advanced via
    /// `ItemSeriesRepo::advance_cursor` (a forward-only max, never regresses). `None`
    /// means nothing has been settled yet, in which case the series' "current"
    /// occurrence is its own `anchor_date`. Meaningless for `Event`-typed series, which
    /// have no completion concept and so never advance it — always `None` for those.
    pub cursor_date: Option<DateTime<Utc>>,
    /// A plain, unvalidated-by-Smithy string, following the precedent CLAUDE.md documents
    /// for `Item::recurrence_basis` (`ItemType` is the deliberate exception, not the norm)
    /// — but unlike most such fields, only two values are actually legal, and
    /// `service::item_series::validate_series_basis` rejects everything else outright
    /// (including the retired `"DUE_DATE"`, see below) rather than silently treating an
    /// unrecognized value as the default. `None` is the default: a Task-typed series
    /// materializes each occurrence onto `due_date` and advances its cursor on the fixed
    /// schedule; an Event-typed series (which can never set this field to anything but
    /// `None`) materializes onto `scheduled_date`. `Some("COMPLETION")` — Task-only, and
    /// only for an "every N days/weeks/months/years" `recurrence` — measures the next
    /// occurrence from *actual settlement time* (`Utc::now()` at completion/skip) instead;
    /// see `service::item_series::is_completion_basis`. It does not change which field a
    /// materialized occurrence lands on, only how the cursor advances.
    ///
    /// A Task series' recurrence rule used to be able to materialize onto `scheduled_date`
    /// too, selected by `basis: None` (the original default) with `Some("DUE_DATE")` as the
    /// opt-in alternative — removed entirely (`docs/issues_and_features.md`, decided
    /// 2026-09-05): a series' recurrence rule defines a due date, and scheduling is a
    /// per-instance decision that has no business being what a recurrence rule produces.
    /// `basis` briefly did double duty as a result (encoding both "due-date vs
    /// scheduled-date" and "fixed-schedule vs completion-time" in one field that can only
    /// hold one string) — collapsing to a single Task-side materialization target restores
    /// it to the one orthogonal flag its doc comment already claimed it was. A migration
    /// normalized every existing Task-typed row's `basis` to `None` unless it was already
    /// `"COMPLETION"` (`AddItemSeriesDueDateOnly`), so no stored row can carry `"DUE_DATE"`
    /// or a scheduled-date-basis `None` after it has run.
    pub basis: Option<String>,
    /// Points/assignment authority for this series' materialized occurrences —
    /// mirrors `TeamAssignment` at the item level (CLAUDE.md's Points section), but
    /// lives on the series rather than each occurrence so every future materialization
    /// inherits the same assignee/point value without re-specifying it per occurrence.
    /// Only ever settable on a `Task`-typed series on a team-backed project
    /// (`service::item_series::resolve_series_assignment`) — `Event` series and
    /// personal-project series never carry a `TeamAssignment` at the item level either,
    /// so this mirrors that restriction rather than introducing a new one.
    /// `assigned_to_user_id` must be a member of the series' project;
    /// `points` is settable only by that project's admin (silently dropped for a
    /// non-admin request, matching `create_team_item`'s existing precedent).
    pub assigned_to_user_id: Option<String>,
    pub points: Option<i32>,
    /// 1 (highest) through 4 (lowest); `None` sorts last — mirrors `Item::priority`
    /// (root CLAUDE.md's Priority section). Unlike `assigned_to_user_id`/`points`
    /// above, this is *not* restricted to a team-backed project and *not*
    /// admin-gated — only the `item_type == Task` restriction applies. Carried onto
    /// every occurrence `get_or_materialize_occurrence` materializes, the same way
    /// `points`/`assigned_to_user_id` already are.
    pub priority: Option<i32>,
}

/// One sub-item *definition* on a Task-typed series — "book the venue, 30 days before
/// every occurrence". Deliberately not an `ItemSeries` of its own (an earlier attempt
/// modeled it that way and was reverted): a sub-item has no recurrence, anchor, cursor,
/// or independent current-occurrence, so modeling it as a series meant nullifying or
/// delegating almost every field on `ItemSeries` and adding a `parent_series_id.is_none()`
/// exemption to every cursor codepath. It also isn't an `ItemType::Template` child, which
/// is a project-library artifact with its own lifecycle that could be edited or deleted
/// out from under the series; these rows are owned by the series and cascade with it.
///
/// The set is flat — a definition has no children of its own. A materialized occurrence
/// can still be given ordinary sub-items by hand afterward.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemSeriesChild {
    pub id: String,
    pub series_id: String,
    pub name: String,
    pub description: Option<String>,
    /// Non-negative, matching the app-wide "days before due" presentation convention
    /// (CLAUDE.md's Recurrence section: callers take a non-negative number and negate it).
    /// Negated into the materialized item's `due_offset_days`, so `Item::validate`'s
    /// "due offset days cannot be positive" rule holds by construction.
    pub days_before: i32,
    /// 1 (highest) through 4 (lowest); carried onto the materialized item. Same range
    /// `Item::validate` enforces.
    pub priority: Option<i32>,
    pub sort_order: i32,
}

/// One cycle's materialization state for a single `ItemSeriesChild`. Mirrors
/// `ItemOccurrence` below, with two deliberate differences: no `is_exdate` (a sub-item has
/// no Skip action, so there is no third state), and therefore a non-optional `item_id` —
/// a row exists only once the sub-item has actually been materialized. A cycle with no row
/// is purely virtual, computed on the fly from the definition and the parent's occurrence
/// date.
///
/// `occurrence_date` is the *parent series'* cycle date, not the sub-item's own due date.
/// That keeps the identity stable when a definition's `days_before` is edited.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesChildOccurrence {
    pub child_id: String,
    pub occurrence_date: DateTime<Utc>,
    pub item_id: String,
}

/// One occurrence date's materialization state within a series. A date with no row
/// here at all is purely virtual (computed on the fly from `occurrences_between`,
/// never persisted); this type only represents dates that have moved past that —
/// materialized (`item_id` points at a real `items` row) or skipped (`is_exdate`,
/// the EXDATE-equivalent — never materializes).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemOccurrence {
    pub series_id: String,
    pub occurrence_date: DateTime<Utc>,
    pub item_id: Option<String>,
    pub is_exdate: bool,
}
