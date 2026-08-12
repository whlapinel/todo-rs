use chrono::{DateTime, Utc};

/// A single completion/points event on a team item — the only kind of event this log
/// records (see CLAUDE.md: general item CRUD history is out of scope). `points_delta`
/// is signed and is the sole "what happened" field (positive = a completion earned
/// points); reversing an entry flips `reversed` rather than writing a second row.
///
/// `item_name`/`team_id` are denormalized deliberately: recurrence deletes and
/// recreates items under fresh ids, and items can be deleted outright, so the log
/// must stay meaningful independent of the `items` table's current state.
#[derive(Debug, Clone)]
pub struct ActivityLogEntry {
    pub id: String,
    pub team_id: String,
    /// Dual-written alongside `team_id` (see docs/project-abstraction-plan.md stage
    /// B2) — `team_id` stays authoritative for points (still `team_members`-keyed,
    /// see CLAUDE.md's Points section), `project_id` is what reads now key off.
    pub project_id: Option<String>,
    pub user_id: String,
    pub item_id: String,
    pub item_name: String,
    pub points_delta: i32,
    pub reversed: bool,
    pub created_at: DateTime<Utc>,
}
