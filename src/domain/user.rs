#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub google_id: Option<String>,
}

impl User {
    pub fn new(first_name: &str, last_name: &str) -> Self {
        Self {
            id: String::new(),
            first_name: first_name.to_string(),
            last_name: last_name.to_string(),
            email: None,
            google_id: None,
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
