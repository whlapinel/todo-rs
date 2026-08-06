use crate::auth::AuthUser;
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::items::{self as item_service, ItemError};
use crate::service::team_items::{self as team_item_service, UpdateTeamItemParams};
use crate::storage::sqlite::{DueItem, ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::http::StatusCode;
use axum::response::Html;
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;

fn repo_status(e: RepoError) -> StatusCode {
    match e {
        RepoError::NotFound => StatusCode::NOT_FOUND,
        RepoError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn service_status(e: ItemError) -> StatusCode {
    match e {
        ItemError::NotFound => StatusCode::NOT_FOUND,
        ItemError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ItemError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn render<T: Template>(t: T) -> Result<Html<String>, StatusCode> {
    t.render().map(Html).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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
    parent_name: Option<String>,
    from_badge: Option<String>,
    can_delete: bool,
    toggle_target: String,
    // Team items don't have a web_ui detail page yet (Stage 3) — route those to the SPA's
    // still-functional `/teams/:teamId/items/:itemId` view instead of `/web/items/:id`,
    // which is scoped to personal items and would 404 for a team-owned id.
    detail_link: String,
    toggle_complete_json: String,
}

impl DashboardRow {
    fn from_due_item(di: &DueItem, tz: i32) -> Self {
        let item = &di.item;
        let is_team_item = item.team_id.is_some();
        let (toggle_target, detail_link) = match &item.team_id {
            Some(team_id) => (
                format!("/web/dashboard/team-items/{team_id}/{}", item.id),
                format!("/teams/{team_id}/items/{}", item.id),
            ),
            None => (
                format!("/web/dashboard/items/{}", item.id),
                format!("/web/items/{}", item.id),
            ),
        };
        // Personal items returned by `list_due` are always the caller's own (the query
        // scopes on `user_id = ?`), so the delete affordance only ever applies to those —
        // team items surface here purely via assignment and were never something this user
        // could unilaterally delete, matching `frontend/src/main.ts`'s `isOwn` gating.
        Self {
            item_id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item.due_date.map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            parent_name: if di.parent_name.is_empty() { None } else { Some(di.parent_name.clone()) },
            from_badge: item.team_id.as_ref().map(|team_id| format!("from team {team_id}")),
            can_delete: !is_team_item,
            toggle_target,
            detail_link,
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
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DashboardQuery {
    preset: Option<String>,
    show_complete: Option<String>,
}

fn render_rows(items: &[DueItem], preset: &str, show_complete: bool, tz: i32) -> Result<Vec<String>, StatusCode> {
    items
        .iter()
        .filter(|di| show_complete || !di.item.complete)
        .filter(|di| preset != "All with due date" || di.item.due_date.is_some())
        .map(|di| DashboardRow::from_due_item(di, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn dashboard_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz_offset): TzOffset,
    Query(q): Query<DashboardQuery>,
) -> Result<Html<String>, StatusCode> {
    let preset = q.preset.unwrap_or_else(|| "Today".to_string());
    let show_complete = q.show_complete.is_some();
    let (after, before) = preset_range(&preset, Utc::now(), tz_offset);

    let due_items = repo
        .list_due(&auth_user.user_id, after.map(|d| d.timestamp()), before.map(|d| d.timestamp()))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = render_rows(&due_items, &preset, show_complete, tz_offset)?;

    let presets = PRESETS.iter().map(|&p| (p, p == preset)).collect();
    render(DashboardPageTemplate {
        rows,
        show_complete,
        presets,
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
) -> Result<Html<String>, StatusCode> {
    let current = repo.get(&auth_user.user_id, &item_id).await.map_err(repo_status)?;
    let params = item_service::UpdateItemParams {
        user_id: auth_user.user_id.clone(),
        item_id: item_id.clone(),
        name: current.name.clone(),
        due_date: current.due_date,
        complete: form.complete.as_deref() == Some("true"),
        recurrence: current.recurrence.clone(),
        recurrence_basis: current.recurrence_basis.clone(),
        has_due_time: Some(current.has_due_time),
        has_tasks: Some(current.has_tasks),
        parent_item_id: current.parent_item_id.clone(),
        due_offset_days: current.due_offset_days,
        timezone_offset_minutes: Some(tz),
    };
    item_service::update_item(&repo, params).await.map_err(service_status)?;

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
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn toggle_team_item_complete(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ToggleForm>,
) -> Result<Html<String>, StatusCode> {
    let current = repo.get_team_item(&team_id, &item_id).await.map_err(repo_status)?;
    let params = UpdateTeamItemParams {
        team_id: team_id.clone(),
        item_id: item_id.clone(),
        name: current.name.clone(),
        due_date: current.due_date,
        complete: form.complete.as_deref() == Some("true"),
        recurrence: current.recurrence.clone(),
        recurrence_basis: current.recurrence_basis.clone(),
        has_due_time: Some(current.has_due_time),
        has_tasks: Some(current.has_tasks),
        parent_item_id: current.parent_item_id.clone(),
        due_offset_days: current.due_offset_days,
        assigned_to_user_id: current.assigned_to_user_id.clone(),
        timezone_offset_minutes: Some(tz),
    };
    team_item_service::update_team_item(&repo, &teams, &auth_user.user_id, params)
        .await
        .map_err(service_status)?;

    match repo.get_team_item(&team_id, &item_id).await {
        Ok(updated) => render(DashboardRow::from_due_item(
            &DueItem {
                parent_name: String::new(),
                item: updated,
            },
            tz,
        )),
        Err(RepoError::NotFound) => Ok(Html(String::new())),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
