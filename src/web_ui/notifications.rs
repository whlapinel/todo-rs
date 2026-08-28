use super::{TzOffset, reminder_labels};
use crate::auth::AuthUser;
use crate::service::error::ItemError;
use crate::service::reminders::list_due_notifications_for_user;
use crate::storage::sqlite::{ItemRepo, ReminderRepo};
use askama::Template;
use axum::extract::{Extension, Path};
use axum::http::HeaderMap;
use axum::response::Html;
use chrono::Utc;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

#[derive(Template)]
#[template(path = "notifications/badge.html")]
struct NotificationBadge {
    count: usize,
}

#[derive(Template)]
#[template(path = "notifications/row.html")]
struct NotificationRow {
    id: String,
    item_name: String,
    detail_url: String,
    label: String,
}

#[derive(Template)]
#[template(path = "notifications/list.html")]
struct NotificationList {
    rows: Vec<String>,
}

async fn build_list(
    reminders: &Arc<dyn ReminderRepo>,
    items: &Arc<dyn ItemRepo>,
    user_id: &str,
    tz: i32,
) -> Result<NotificationList, ItemError> {
    let due = list_due_notifications_for_user(reminders, items, user_id, Utc::now()).await?;
    let rows = due
        .into_iter()
        .map(|n| {
            let label = reminder_labels(std::slice::from_ref(&n.reminder), tz)
                .into_iter()
                .next()
                .unwrap_or_default();
            NotificationRow {
                id: n.reminder.id,
                item_name: n.item_name,
                detail_url: n.detail_url,
                label,
            }
            .render()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NotificationList { rows })
}

pub async fn notifications_badge(
    Extension(auth_user): Extension<AuthUser>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(items): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    let due =
        list_due_notifications_for_user(&reminders, &items, &auth_user.user_id, Utc::now()).await?;
    render(NotificationBadge { count: due.len() })
}

pub async fn notifications_list(
    Extension(auth_user): Extension<AuthUser>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(items): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let list = build_list(&reminders, &items, &auth_user.user_id, tz).await?;
    render(list)
}

pub async fn dismiss_notification_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(items): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Path(id): Path<String>,
) -> Result<(HeaderMap, Html<String>), ItemError> {
    reminders.dismiss(&id, &auth_user.user_id).await?;
    let list = build_list(&reminders, &items, &auth_user.user_id, tz).await?;
    let mut headers = HeaderMap::new();
    headers.insert("HX-Trigger", "refresh-badge".parse().unwrap());
    Ok((headers, render(list)?))
}
