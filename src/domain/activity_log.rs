use chrono::{DateTime, Utc};

/// A single completion event on an item — team-assigned points-bearing completions and
/// (see docs/issues.md's "unify completion-undo" note) any other top-level completion,
/// personal or team, points or not (see CLAUDE.md: general item CRUD history is still
/// out of scope — only completions get a row). `points_delta` is signed and the sole
/// "what happened to the points balance" field (positive = a completion earned points,
/// zero = a completion with nothing to award); reversing an entry flips `reversed`
/// rather than writing a second row.
///
/// `item_name`/`team_id` are denormalized deliberately: recurrence deletes and
/// recreates items under fresh ids, and items can be deleted outright, so the log
/// must stay meaningful independent of the `items` table's current state.
#[derive(Debug, Clone)]
pub struct ActivityLogEntry {
    pub id: String,
    /// `None` for a personal (non-team) project's item completion — every row before
    /// this was always team-scoped, so `team_id` was `NOT NULL` until then (see
    /// `docs/issues.md`'s "unify completion-undo" note and the
    /// `ActivityLogTeamIdNullable` migration). Still dual-written alongside
    /// `project_id` for a team completion (see docs/project-abstraction-plan.md stage
    /// B2) — `team_id` stays authoritative for the legacy team-scoped
    /// `ListTeamActivityLog`/`UndoActivityLogEntry` ops (still `team_members`-keyed,
    /// see CLAUDE.md's Points section), `project_id` is what every other read keys
    /// off, and the only thing ever set for a personal completion.
    pub team_id: Option<String>,
    pub project_id: Option<String>,
    pub user_id: String,
    pub item_id: String,
    pub item_name: String,
    pub points_delta: i32,
    pub reversed: bool,
    pub created_at: DateTime<Utc>,
}
