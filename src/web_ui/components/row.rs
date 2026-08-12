use askama::Template;

#[derive(Template)]
#[template(path = "components/row.html")]
pub struct Row {
    pub expanded_row: bool,
    pub id: String,
    pub name: String,
    pub complete: bool,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub scheduled_date: Option<String>,
    pub has_children: bool,
    pub offset_label: Option<String>,
    pub recurrence: Option<String>,
    pub complete_url: Option<String>,
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