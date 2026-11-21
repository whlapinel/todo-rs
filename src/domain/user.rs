#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub google_id: Option<String>,
    /// IANA timezone name (e.g. "America/New_York"), set via the web Profile screen
    /// (`service::users::update_profile`) or `UpdateUser`. `None` until a user explicitly
    /// sets one. The stand-in for the browser's live offset wherever there's no request to
    /// read it from: `service::calendar_sync` (all-day event localization) and
    /// `service::push` (reminder times in push text). Never auto-updated, so it goes stale
    /// when a user moves unless they change it.
    pub timezone: Option<String>,
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
