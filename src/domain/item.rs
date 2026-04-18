use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub user_id: String,
    pub list_id: String,
    pub name: String,
    pub description: Option<String>,
    pub goal_date: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,
}

impl Item {
    pub fn new(user_id: &str, list_id: &str, name: &str) -> Self {
        Self {
            id: String::new(),
            user_id: user_id.to_string(),
            list_id: list_id.to_string(),
            name: name.to_string(),
            ..Self::default()
        }
    }
}
