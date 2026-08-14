use crate::domain::item_series::ItemSeries;
use crate::web_ui::to_local;
use askama::Template;

pub struct ProjectItemSeriesRow;

impl ProjectItemSeriesRow {
    pub fn from_series(s: &ItemSeries, tz: i32) -> Row {
        Row {
            name: s.name.clone(),
            recurrence: s.recurrence.clone(),
            event_type: s.event_type.clone(),
            anchor_date_label: to_local(s.anchor_date, tz)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            item_type_label: s.item_type.label(),
            item_type_badge_color: s.item_type.badge_color(),
        }
    }
}

#[derive(Template)]
#[template(path = "project_item_series/row.html")]
pub struct Row {
    pub name: String,
    pub recurrence: String,
    pub event_type: Option<String>,
    pub anchor_date_label: String,
    pub item_type_label: &'static str,
    pub item_type_badge_color: &'static str,
}

#[derive(Template)]
#[template(path = "project_item_series/list_page.html")]
pub struct ProjectItemSeriesListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_item_series/new_page.html")]
pub struct NewProjectItemSeriesPageTemplate {
    pub project_id: String,
    pub nav_html: String,
}
