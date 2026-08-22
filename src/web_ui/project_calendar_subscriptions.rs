use super::nav::{self, ActiveContext, SidebarSection};
use super::{TzOffset, to_local};
use crate::auth::AuthUser;
use crate::domain::calendar_subscription::CalendarSubscription;
use crate::service::calendar_subscriptions as calendar_subscriptions_service;
use crate::service::error::ItemError;
use crate::service::projects as project_service;
use crate::storage::sqlite::{CalendarSubscriptionRepo, ItemRepo, ProjectRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::Html;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Shows enough of the URL for the admin to recognize *which* calendar this is
/// without exposing the whole secret address on screen (it's still sent to the
/// browser in the DOM either way — this is legibility, not actual secrecy).
fn mask_url(url: &str) -> String {
    const KEEP: usize = 40;
    if url.chars().count() <= KEEP {
        url.to_string()
    } else {
        let head: String = url.chars().take(KEEP).collect();
        format!("{head}…")
    }
}

/// Swapped into the shared `#action-dialog` (see base.html) — purely static content,
/// no server data, but still fetched via hx-get rather than a plain client-side
/// `showModal()` call: base.html's `htmx:afterSwap` listener auto-opens any `<dialog>`
/// found inside a swapped target, so a `<dialog>` embedded directly in this screen's own
/// page content (which lives inside `#page`, itself swapped on every boosted navigation
/// here) would pop open on every page load, not just on click.
#[derive(Template)]
#[template(path = "project_calendar_subscriptions/ical_help_dialog.html")]
struct IcalHelpDialog;

pub async fn ical_help_dialog_fragment() -> Result<Html<String>, ItemError> {
    render(IcalHelpDialog)
}

#[derive(Template)]
#[template(path = "project_calendar_subscriptions/row.html")]
struct CalendarSubscriptionRow {
    id: String,
    project_id: String,
    masked_url: String,
    last_synced_label: String,
    last_sync_error: Option<String>,
    is_admin: bool,
}

impl CalendarSubscriptionRow {
    fn from_subscription(sub: &CalendarSubscription, is_admin: bool, tz: i32) -> Self {
        let last_synced_label = match sub.last_synced_at {
            Some(t) => format!("Synced {}", to_local(t, tz).format("%Y-%m-%d %H:%M")),
            None => "Not synced yet".to_string(),
        };
        Self {
            id: sub.id.clone(),
            project_id: sub.project_id.clone(),
            masked_url: mask_url(&sub.ical_url),
            last_synced_label,
            last_sync_error: sub.last_sync_error.clone(),
            is_admin,
        }
    }
}

#[derive(Template)]
#[template(path = "project_calendar_subscriptions/page.html")]
struct ProjectCalendarSubscriptionsPageTemplate {
    project_id: String,
    rows: Vec<String>,
    is_admin: bool,
    nav_html: String,
}

async fn render_page(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    calendar_repo: &Arc<dyn CalendarSubscriptionRepo>,
    project_id: &str,
    requester_user_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let is_admin =
        project_service::is_project_admin(projects, teams, project_id, requester_user_id).await;
    let subs = calendar_subscriptions_service::list_calendar_subscriptions(
        projects,
        teams,
        calendar_repo,
        project_id,
        requester_user_id,
    )
    .await?;
    let rows = subs
        .iter()
        .map(|s| CalendarSubscriptionRow::from_subscription(s, is_admin, tz).render())
        .collect::<Result<Vec<_>, _>>()?;
    let active = ActiveContext::Project(project_id.to_string());
    let nav_html =
        nav::build_nav_html(projects, requester_user_id, active, SidebarSection::Events).await?;
    render(ProjectCalendarSubscriptionsPageTemplate {
        project_id: project_id.to_string(),
        rows,
        is_admin,
        nav_html,
    })
}

pub async fn project_calendar_subscriptions_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(calendar_repo): Extension<Arc<dyn CalendarSubscriptionRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    render_page(
        &projects,
        &teams,
        &calendar_repo,
        &project_id,
        &auth_user.user_id,
        tz,
    )
    .await
}

#[derive(serde::Deserialize)]
pub struct CreateCalendarSubscriptionForm {
    ical_url: String,
}

pub async fn create_project_calendar_subscription_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(calendar_repo): Extension<Arc<dyn CalendarSubscriptionRepo>>,
    Extension(item_repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<CreateCalendarSubscriptionForm>,
) -> Result<Html<String>, ItemError> {
    calendar_subscriptions_service::create_calendar_subscription(
        &projects,
        &teams,
        &calendar_repo,
        &item_repo,
        &project_id,
        &auth_user.user_id,
        form.ical_url.trim(),
    )
    .await?;
    render_page(
        &projects,
        &teams,
        &calendar_repo,
        &project_id,
        &auth_user.user_id,
        tz,
    )
    .await
}

pub async fn delete_project_calendar_subscription_form(
    Path((project_id, subscription_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(calendar_repo): Extension<Arc<dyn CalendarSubscriptionRepo>>,
    Extension(item_repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    calendar_subscriptions_service::delete_calendar_subscription(
        &projects,
        &teams,
        &calendar_repo,
        &item_repo,
        &project_id,
        &auth_user.user_id,
        &subscription_id,
    )
    .await?;
    render_page(
        &projects,
        &teams,
        &calendar_repo,
        &project_id,
        &auth_user.user_id,
        tz,
    )
    .await
}
