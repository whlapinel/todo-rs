#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
}

impl User {
    pub fn new(first_name: &str, last_name: &str) -> Self {
        Self {
            id: String::new(),
            first_name: first_name.to_string(),
            last_name: last_name.to_string(),
        }
    }
}
