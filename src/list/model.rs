use crate::list::{self, item::model::ListItem};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct List {
    pub name: String,
    pub id: Option<u64>,
    pub items: Vec<ListItem>,
}

impl List {
    /// if asc (ascending) is true, closest deadline will be first
    pub fn sort_by_deadline(&mut self, asc: bool) {
        if asc {
            self.items.sort_by_key(|i| i.deadline);
        } else {
            self.items.sort_by_key(|i| std::cmp::Reverse(i.deadline));
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Days, Utc};

    use super::*;

    pub fn test_item(name: &str, days_from_today: u64) -> ListItem {
        let mut item = ListItem::new(name);
        item.deadline = Utc::now().checked_add_days(Days::new(days_from_today));
        item
    }

    #[test]
    pub fn test_list() {
        let mut list = List::default();
        let items = vec![
            test_item("take out garbage", 2),
            test_item("break down boxes", 3),
            test_item("call spectrum", 4),
        ];
        for item in items {
            list.items.push(item);
        }
        list.sort_by_deadline(false);
        assert_eq!(
            list.items.first().unwrap().name,
            "call spectrum".to_string()
        );
    }
}
