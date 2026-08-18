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
}
