use askama::Template;

#[derive(Template)]
#[template(path = "components/row.html")]
pub struct Row {
    pub expanded_row: bool,
    pub id: String,
    /// Base URL for this item's own detail page and delete action (e.g.
    /// `/web/projects/{project_id}/tasks/{id}`) — set once by the caller rather than
    /// hardcoded here, since `Row` is shared across every project-scoped screen
    /// (Tasks first, Events/Simple Lists to follow in later B5 sub-stages), each with
    /// its own URL family.
    pub item_url: String,
    pub name: String,
    pub complete: bool,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub scheduled_date: Option<String>,
    /// Paired with `scheduled_date` to render a window (`start–end`) rather than a bare
    /// start — added in stage B5b for `ProjectEventRow`, but wired into `ProjectTaskRow` too
    /// since a Task's own scheduled window was previously invisible at the row level (only
    /// `ProjectTaskDetailView` showed it).
    pub scheduled_end_date: Option<String>,
    /// Event-only field (`Item::validate` restricts `event_type` to `Event`/`Template` — see
    /// CLAUDE.md's Events section), always `None` for a Task row.
    pub event_type: Option<String>,
    pub has_children: bool,
    pub offset_label: Option<String>,
    /// Display name of this item's assignee, on a team-backed project — `None` on a
    /// personal project (no assignment concept) or an unassigned team item.
    pub assignee_name: Option<String>,
    pub complete_url: Option<String>,
    pub duplicate_url: Option<String>,
    pub reschedule_url: Option<String>,
    /// `Some(...)` only on a team-backed project (assignment has no meaning on a personal
    /// one) — opens the quick-assign dialog, mirroring `reschedule_url`'s dialog pattern.
    pub assign_url: Option<String>,
    /// `Some(...)` when this item was materialized from an `item_series` occurrence — see
    /// `service::item_series::skip_url_for_item`. Lets a materialized row's own Skip action
    /// reach the same unified Skip route a still-virtual occurrence's row already uses, so the
    /// user never needs to know or care whether a series occurrence is materialized before
    /// skipping it. `None` for any item that never came from a series.
    pub skip_url: Option<String>,
    pub toggle_complete_json: String,
    /// (id, name) of every other item rendered alongside this one in the same list —
    /// i.e. this item's actual siblings, since `render_rows` is only ever called with a
    /// single sibling group (a full top-level list or one parent's children) at a time.
    /// Populates the row's "subordinate under…" picker (see `subordinate_task_form`);
    /// empty for an only child / sole top-level item.
    pub siblings: Vec<(String, String)>,
    /// True if this task references an Event via `sourceEventId` — its row hides the
    /// "subordinate under…" picker even when siblings exist, since giving it a
    /// `parentItemId` too would conflict with the reference (see `Item::validate`).
    pub is_source_event_linked: bool,
    /// Whether the screen this row belongs to is currently showing completed items — baked
    /// into the checkbox's own `hx-vals` (as `showComplete`) so a completion PUT round-trips
    /// the value back to the handler that decides `dismiss_after_ms` below, the same way
    /// `toggle_complete_json` round-trips the next click's target state. Irrelevant (and
    /// harmless as `false`) for a row with no `complete_url`.
    pub show_complete: bool,
    /// Set on the single freshly-rendered row returned by a successful action's response
    /// (e.g. "Completed") to show a brief, non-blocking `.toast-fade` badge — the reusable
    /// counterpart to `ProjectTaskDetailFields`' `just_saved` for row-level actions, not
    /// completion-specific. `None` on every other render (list loads, unrelated updates).
    pub confirmation: Option<String>,
    /// Set (in ms) on that same one-shot response when this row should remove itself from
    /// the list shortly after — e.g. a just-completed task on a list currently hiding
    /// completed items — rather than vanishing the instant the request completes. Consumed
    /// by the generic `data-dismiss-after` handling in `base.html`/`styles/input.css`; not
    /// completion-specific, any action's row response can set it. `None` means "stay put."
    pub dismiss_after_ms: Option<u32>,
    /// True when this item was imported from a Google Calendar subscription
    /// (`Item::google_event_id.is_some()`) — see CLAUDE.md's read-only-enforcement note.
    /// Only ever `true` for a `ProjectEventRow` (imported items are always Events); every
    /// other screen's rows pass `false`. Hides the row's reschedule/duplicate/delete actions
    /// (each would either no-op against a server-side rejection or, for duplicate, hit the
    /// `(calendar_subscription_id, google_event_id)` unique-index guard) and shows a small
    /// "Google Calendar" badge instead.
    pub is_imported: bool,
}
