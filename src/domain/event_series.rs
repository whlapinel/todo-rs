use chrono::{DateTime, Utc};

/// The recurrence rule + anchor + static fields for a browsable recurring Event
/// series — see docs/recurring-events-virtual-occurrences-rough-plan.md's staged
/// breakdown, stage 2. Distinct from `Item`'s own `recurrence`/`recurrence_basis`
/// auto-advance-on-read mechanism (see CLAUDE.md's Recurrence section): that model
/// conflates "the rule" and "the currently active instance" into one row, whereas a
/// series has no single date of its own — just a rule an occurrence date is computed
/// against via `domain::recurrence::occurrences_between`.
#[derive(Debug, Clone, PartialEq)]
pub struct EventSeries {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    /// Raw English pattern, parsed via `domain::recurrence::parse` — same
    /// convention as `Item::recurrence`, not a pre-parsed `RecurrenceRule` column.
    pub recurrence: String,
    pub anchor_date: DateTime<Utc>,
}

/// One occurrence date's materialization state within a series. A date with no row
/// here at all is purely virtual (computed on the fly from `occurrences_between`,
/// never persisted); this type only represents dates that have moved past that —
/// materialized (`item_id` points at a real `items` row) or skipped (`is_exdate`,
/// the EXDATE-equivalent — never materializes).
#[derive(Debug, Clone, PartialEq)]
pub struct EventOccurrence {
    pub series_id: String,
    pub occurrence_date: DateTime<Utc>,
    pub item_id: Option<String>,
    pub is_exdate: bool,
}
