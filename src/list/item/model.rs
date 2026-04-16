use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListItem {
    pub name: String,
    pub id: Option<u64>,
    pub description: Option<String>,
    pub goal_date: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,
    pub recur_reference: Option<RecurringReference>,
    /// duration that will be added to last due date or last completion date
    pub frequency: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecurringReference {
    Deadline,
    Completion,
}

impl ListItem {
    pub fn new(name: &str) -> Self {
        let mut task = Self::default();
        task.name = name.to_string();
        task
    }
}
