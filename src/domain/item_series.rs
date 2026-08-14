use chrono::{DateTime, Utc};

use crate::domain::item::ItemKind;

/// The recurrence rule + anchor + static fields for a browsable recurring
/// series — see docs/recurring-events-virtual-occurrences-rough-plan.md's staged
/// breakdown. Originally Event-only (stage 2, as `EventSeries`); generalized to
/// also cover Task series at stage 7a via the `item_type` field, backed by the
/// `item_series` table (renamed from `event_series` — see that migration for the
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
