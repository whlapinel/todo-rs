#[derive(Debug, Clone, Default, PartialEq)]
pub struct List {
    pub id: String,
    pub user_id: String,
    pub name: String,
}

impl List {
    pub fn new(user_id: &str, name: &str) -> Self {
        Self {
            id: String::new(),
            user_id: user_id.to_string(),
            name: name.to_string(),
        }
    }
}
