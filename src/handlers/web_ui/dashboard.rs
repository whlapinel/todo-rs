use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::items::{self as item_service, ItemError};
use crate::service::team_items::{self as team_item_service, UpdateTeamItemContext, UpdateTeamItemParams};
use crate::storage::sqlite::{ActivityLogRepo, DueItem, ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::Html;
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Mirrors `presetRange()` in `frontend/src/main.ts` — computes a due-date window using the
/// *user's local* calendar day boundaries (derived from `tzOffsetMinutes`), not UTC ones, so
/// "Today" means today where the user actually is. Same `local = utc - offset` convention as
/// `domain::recurrence::apply_end_of_day`. "All" and "All with due date" both return an
/// unrestricted range here — the latter's extra "must have a due date at all" condition is a
/// separate post-filter applied by the caller, not a date-range concern.
fn preset_range(preset: &str, now: DateTime<Utc>, tz_offset_minutes: i32) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let offset = Duration::minutes(tz_offset_minutes as i64);
    let local_now = now - offset;
    let local_date = local_now.date_naive();
    let to_utc = |naive: chrono::NaiveDateTime| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc) + offset;
    let today_start = to_utc(local_date.and_hms_opt(0, 0, 0).unwrap());

    match preset {
        "Today" => (Some(today_start), Some(to_utc(local_date.and_hms_opt(23, 59, 59).unwrap()))),
        "This Week" => (Some(today_start), Some(today_start + Duration::days(7))),
        "Next 30 Days" => (Some(today_start), Some(today_start + Duration::days(30))),
        "Overdue" => (None, Some(now)),
        _ => (None, None),
    }
}

#[derive(Template)]
#[template(path = "dashboard/row.html")]
struct DashboardRow {
    item_id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    overdue: bool,
    parent_name: Option<String>,
    from_badge: Option<String>,
    can_delete: bool,
    toggle_target: String,
    // Every item type now has its own dedicated web_ui screen (personal and team-scoped) —
    // this links straight there rather than to any generic catch-all.
    detail_link: String,
    toggle_complete_json: String,
}

/// Dedicated-screen URL for `item`, dispatched by its actual type — shared by both this
/// row's own `detail_link`/delete target and `assigned_items.rs`'s equivalent (team-only
/// subset there).
fn detail_url(item: &Item) -> String {
    match (&item.team_id, item.kind()) {
        (Some(team_id), ItemKind::Task) => format!("/web/team-tasks/{team_id}/{}", item.id),
        (Some(team_id), ItemKind::Event) => format!("/web/team-events/{team_id}/{}", item.id),
        (Some(team_id), ItemKind::Simple) => {
            format!("/web/team-simple-lists/{team_id}/{}", item.id)
        }
        (Some(team_id), ItemKind::Template) => {
            format!("/web/team-templates/{team_id}/{}", item.id)
        }
        (None, ItemKind::Task) => format!("/web/tasks/{}", item.id),
        (None, ItemKind::Event) => format!("/web/events/{}", item.id),
        (None, ItemKind::Simple) => format!("/web/simple-lists/{}", item.id),
        (None, ItemKind::Template) => format!("/web/templates/{}", item.id),
    }
}

impl DashboardRow {
    fn from_due_item(di: &DueItem, tz: i32) -> Self {
        let item = &di.item;
        let is_team_item = item.team_id.is_some();
        let toggle_target = match &item.team_id {
            Some(team_id) => format!("/web/dashboard/team-items/{team_id}/{}", item.id),
            None => format!("/web/dashboard/items/{}", item.id),
        };
        // Personal items returned by `list_due` are always the caller's own (the query
        // scopes on `user_id = ?`), so the delete affordance only ever applies to those —
        // team items surface here purely via assignment and were never something this user
        // could unilaterally delete, matching `frontend/src/main.ts`'s `isOwn` gating.
        Self {
            item_id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item.due_date().map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            overdue: item.is_overdue(Utc::now()),
            parent_name: if di.parent_name.is_empty() { None } else { Some(di.parent_name.clone()) },
            from_badge: item.team_id.as_ref().map(|team_id| format!("from team {team_id}")),
            can_delete: !is_team_item,
            toggle_target,
            detail_link: detail_url(item),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

const PRESETS: [&str; 6] = ["All", "All with due date", "Today", "This Week", "Next 30 Days", "Overdue"];

#[derive(Template)]
#[template(path = "dashboard/page.html")]
struct DashboardPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    /// (option label, is currently selected) — precomputed so the template never has to
    /// compare strings itself.
    presets: Vec<(&'static str, bool)>,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DashboardQuery {
    preset: Option<String>,
    show_complete: Option<String>,
}

fn render_rows(items: &[DueItem], preset: &str, show_complete: bool, tz: i32) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .filter(|di| show_complete || !di.item.complete)
        .filter(|di| preset != "All with due date" || di.item.due_date().is_some())
        .map(|di| DashboardRow::from_due_item(di, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

pub async fn dashboard_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz_offset): TzOffset,
    Query(q): Query<DashboardQuery>,
) -> Result<Html<String>, ItemError> {
    let preset = q.preset.unwrap_or_else(|| "Today".to_string());
    let show_complete = q.show_complete.is_some();
    let (after, before) = preset_range(&preset, Utc::now(), tz_offset);

    let due_items = repo
        .list_due(&auth_user.user_id, after.map(|d| d.timestamp()), before.map(|d| d.timestamp()))
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&due_items, &preset, show_complete, tz_offset)?;

    let presets = PRESETS.iter().map(|&p| (p, p == preset)).collect();
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::None,
    )
    .await?;
    render(DashboardPageTemplate {
        rows,
        show_complete,
        presets,
        nav_html,
    })
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToggleForm {
    complete: Option<String>,
}

pub async fn toggle_item_complete(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ToggleForm>,
) -> Result<Html<String>, ItemError> {
    let current = repo.get(&auth_user.user_id, &item_id).await.map_err(ItemError::from)?;
    let params = item_service::UpdateItemParams {
        user_id: auth_user.user_id.clone(),
        item_id: item_id.clone(),
        name: current.name.clone(),
        due_date: current.due_date(),
        scheduled_date: current.scheduled_date(),
        scheduled_end_date: current.scheduled_end_date(),
        complete: form.complete.as_deref() == Some("true"),
        recurrence: current.recurrence_pattern(),
        recurrence_basis: current.recurrence_basis(),
        has_due_time: Some(current.has_due_time()),
        has_scheduled_time: Some(current.has_scheduled_time()),
        has_end_time: Some(current.has_end_time()),
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(current.kind()),
        event_type: current.event_type(),
        due_offset_days: current.due_offset_days(),
        timezone_offset_minutes: Some(tz),
    };
    item_service::update_item(&repo, params).await?;

    match repo.get(&auth_user.user_id, &item_id).await {
        Ok(updated) => render(DashboardRow::from_due_item(
            &DueItem {
                parent_name: String::new(),
                item: updated,
            },
            tz,
        )),
        // Recurring item just completed and got replaced under a new id (see
        // service::items::update_item) — nothing to render back for the old id.
        Err(RepoError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(ItemError::from(e)),
    }
}

pub async fn toggle_team_item_complete(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ToggleForm>,
) -> Result<Html<String>, ItemError> {
    let current = repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    let params = UpdateTeamItemParams {
        team_id: team_id.clone(),
        item_id: item_id.clone(),
        name: current.name.clone(),
        due_date: current.due_date(),
        scheduled_date: current.scheduled_date(),
        scheduled_end_date: current.scheduled_end_date(),
        complete: form.complete.as_deref() == Some("true"),
        recurrence: current.recurrence_pattern(),
        recurrence_basis: current.recurrence_basis(),
        has_due_time: Some(current.has_due_time()),
        has_scheduled_time: Some(current.has_scheduled_time()),
        has_end_time: Some(current.has_end_time()),
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(current.kind()),
        event_type: current.event_type(),
        due_offset_days: current.due_offset_days(),
        assigned_to_user_id: current.assigned_to_user_id(),
        timezone_offset_minutes: Some(tz),
        points: current.points(),
    };
    team_item_service::update_team_item(
        &repo,
        &UpdateTeamItemContext { teams, activity_log },
        &auth_user.user_id,
        params,
    )
    .await?;

    match repo.get_team_item(&team_id, &item_id).await {
        Ok(updated) => render(DashboardRow::from_due_item(
            &DueItem {
                parent_name: String::new(),
                item: updated,
            },
            tz,
        )),
        Err(RepoError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(ItemError::from(e)),
    }
}
