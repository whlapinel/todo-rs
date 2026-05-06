use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub user_id: String,
    pub parent_item_id: Option<String>,
    pub name: String,
    pub deadline: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: bool,
    pub has_tasks: bool,
    pub has_children: bool,
}

impl Item {
    pub fn new(user_id: &str, name: &str) -> Self {
        Self {
            user_id: user_id.to_string(),
            name: name.to_string(),
            has_tasks: true,
            ..Self::default()
        }
    }
}
