#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub google_id: Option<String>,
    /// IANA timezone name (e.g. "America/New_York"), set via `UpdateUser`. `None` until
    /// a user explicitly sets one. Currently consumed only by
    /// `service::calendar_sync` to resolve an imported all-day event's date into the
    /// correct UTC instant — see root CLAUDE.md's Google Calendar import notes.
    pub timezone: Option<String>,
    /// The id of this user's canonical default "Personal" project — set once, when that
    /// project is first created/discovered by `service::projects::ensure_default_project`
    /// (`docs/dialog-item-forms-plan.md`'s Stage 0). Unlike `ProjectRepo::find_personal_project`
    /// (any team-less project the user owns, ambiguous once a user has more than one), this
    /// is the single unambiguous answer to "which project is *the* Personal one" — currently
    /// consumed only as the all-projects new-item dialog's default project selection. `None`
    /// only for a user who predates this column and hasn't logged in since (self-heals on
    /// next login).
    pub personal_project_id: Option<String>,
}

impl User {
    pub fn new(first_name: &str, last_name: &str) -> Self {
        Self {
            id: String::new(),
            first_name: first_name.to_string(),
            last_name: last_name.to_string(),
            email: None,
            google_id: None,
            timezone: None,
            personal_project_id: None,
        }
    }
}

/// Splits a display name like "Will Lapinel" into ("Will", "Lapinel").
/// A name with no space becomes (name, ""); an empty/whitespace-only name is None.
pub fn split_display_name(name: &str) -> Option<(String, String)> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    match name.split_once(' ') {
        Some((first, rest)) => Some((first.to_string(), rest.trim().to_string())),
        None => Some((name.to_string(), String::new())),
    }
}
