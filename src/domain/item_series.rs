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
    /// Stage 10 gap 1: a plain, unvalidated-by-Smithy string, following the precedent
    /// CLAUDE.md documents for `Item::recurrence_basis` (`ItemType` is the deliberate
    /// exception, not the norm). `Some("COMPLETION")` measures the next occurrence from
    /// *actual settlement time* (`Utc::now()` at completion/skip) rather than the fixed
    /// schedule — see `service::item_series::is_completion_basis`. `None` (or any other
    /// value) is today's only behavior, schedule-basis. Only meaningful for `Task`-typed
    /// series with an "every N days/weeks/months/years" `recurrence` — validated at the
    /// service layer (`validate_series_basis`), not structurally enforced here.
    pub basis: Option<String>,
    /// docs/series-sub-items-plan.md: `Some` only for a `Task`-typed series that is itself
    /// a child of another `Task`-typed series (validated at the service layer, not
    /// structurally — see `service::item_series::validate_series_parent`). One level of
    /// nesting only — a child series can never itself have `parent_series_id` set on one
    /// of its own children.
    pub parent_series_id: Option<String>,
    /// docs/series-sub-items-plan.md: `Some` only when `parent_series_id.is_some()` —
    /// mirrors `Item.due_offset_days`'s "only meaningful on a child" convention exactly. A
    /// child series has no cadence of its own (see `recurrence`/`anchor_date` above); its
    /// materialized occurrence's `due_date` is computed via `deadline_from_offset` from its
    /// parent's current occurrence date plus this offset.
    pub due_offset_days: Option<i32>,
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
