use crate::domain::item::Item;
use askama::Template;

#[derive(Template)]
#[template(path = "item_list_component.html")]
pub struct ItemListComponent {
    pub id: String,
    pub name: String,
    pub complete: bool,
    pub due_date: Option<String>,
}

impl ItemListComponent {
    pub fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date
                .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()),
        }
    }
}
