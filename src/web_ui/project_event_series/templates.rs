use crate::domain::item_series::ItemSeries;
use crate::web_ui::to_local;
use askama::Template;

pub struct ProjectEventSeriesRow;

impl ProjectEventSeriesRow {
    pub fn from_series(s: &ItemSeries, tz: i32) -> Row {
        Row {
            name: s.name.clone(),
            recurrence: s.recurrence.clone(),
            event_type: s.event_type.clone(),
            anchor_date_label: to_local(s.anchor_date, tz)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "project_event_series/row.html")]
pub struct Row {
    pub name: String,
    pub recurrence: String,
    pub event_type: Option<String>,
    pub anchor_date_label: String,
}

#[derive(Template)]
#[template(path = "project_event_series/list_page.html")]
pub struct ProjectEventSeriesListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_event_series/new_page.html")]
pub struct NewProjectEventSeriesPageTemplate {
    pub project_id: String,
    pub nav_html: String,
}
