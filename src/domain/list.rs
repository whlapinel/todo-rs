#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub has_tasks: bool,
}

impl List {
    pub fn new(user_id: &str, name: &str) -> Self {
        Self {
            id: String::new(),
            user_id: user_id.to_string(),
            name: name.to_string(),
            has_tasks: true,
        }
    }
}

impl Default for List {
    fn default() -> Self {
        Self::new("", "")
    }
}
