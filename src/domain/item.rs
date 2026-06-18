use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub user_id: Option<String>,
    pub team_id: Option<String>,
    pub parent_item_id: Option<String>,
    pub name: String,
    pub deadline: Option<DateTime<Utc>>,
    pub complete: bool,
    pub recurrence: Option<String>,
    pub recurrence_basis: Option<String>,
    pub has_due_time: bool,
    pub has_tasks: bool,
    pub has_children: bool,
    pub is_template: bool,
    pub due_offset_days: Option<i32>,
    pub assigned_to_user_id: Option<String>,
}

impl Item {
    pub fn new_user_item(user_id: &str, name: &str) -> Self {
        Self {
            user_id: Some(user_id.to_string()),
            name: name.to_string(),
            has_tasks: true,
            ..Self::default()
        }
    }

    pub fn new_team_item(team_id: &str, name: &str) -> Self {
        Self {
            team_id: Some(team_id.to_string()),
            name: name.to_string(),
            has_tasks: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_user_item_sets_user_id_and_name() {
        let item = Item::new_user_item("u1", "Buy milk");
        assert_eq!(item.user_id, Some("u1".to_string()));
        assert_eq!(item.name, "Buy milk");
        assert!(item.team_id.is_none());
        assert!(item.has_tasks);
    }

    #[test]
    fn new_team_item_sets_team_id_and_name() {
        let item = Item::new_team_item("t1", "Deploy server");
        assert_eq!(item.team_id, Some("t1".to_string()));
        assert_eq!(item.name, "Deploy server");
        assert!(item.user_id.is_none());
        assert!(item.has_tasks);
    }

    #[test]
    fn new_items_are_not_complete() {
        let user_item = Item::new_user_item("u1", "task");
        let team_item = Item::new_team_item("t1", "task");
        assert!(!user_item.complete);
        assert!(!team_item.complete);
    }
}
