use crate::auth::AuthUser;
use super::dashboard::{detail_url, preset_range, PRESETS};
use super::nav::{self, ActiveContext, SidebarSection};
use super::{to_local, TzOffset};
use crate::service::team_items::{
    self as team_item_service, require_active_member, UpdateTeamItemContext, UpdateTeamItemParams,
};
use crate::service::teams as team_service;
use crate::service::error::ItemError;
use crate::storage::sqlite::{ActivityLogRepo, DueItem, ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::Html;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

async fn names_for(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<HashMap<String, String>, ItemError> {
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;
    Ok(members
        .into_iter()
        .map(|m| (m.user.id.clone(), format!("{} {}", m.user.first_name, m.user.last_name)))
        .collect())
}

#[derive(Template)]
#[template(path = "team_dashboard/row.html")]
struct TeamDashboardRow {
    item_id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    overdue: bool,
    parent_name: Option<String>,
    assignee_name: Option<String>,
    toggle_target: String,
    detail_link: String,
    toggle_complete_json: String,
}

impl TeamDashboardRow {
    fn from_due_item(di: &DueItem, team_id: &str, names: &HashMap<String, String>, tz: i32) -> Self {
        let item = &di.item;
        Self {
            item_id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item.due_date().map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            overdue: item.is_overdue(Utc::now()),
            parent_name: if di.parent_name.is_empty() { None } else { Some(di.parent_name.clone()) },
            assignee_name: item.assigned_to_user_id().and_then(|id| names.get(&id).cloned()),
            toggle_target: format!("/web/team-dashboard/{team_id}/items/{}", item.id),
            detail_link: detail_url(item),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "team_dashboard/page.html")]
struct TeamDashboardPageTemplate {
    team_id: String,
    rows: Vec<String>,
    show_complete: bool,
    presets: Vec<(&'static str, bool)>,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamDashboardQuery {
    preset: Option<String>,
    show_complete: Option<String>,
}

fn render_rows(
    items: &[DueItem],
    team_id: &str,
    names: &HashMap<String, String>,
    preset: &str,
    show_complete: bool,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .filter(|di| show_complete || !di.item.complete)
        .filter(|di| preset != "All with due date" || di.item.due_date().is_some())
        .map(|di| TeamDashboardRow::from_due_item(di, team_id, names, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

pub async fn team_dashboard_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz_offset): TzOffset,
    Query(q): Query<TeamDashboardQuery>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let preset = q.preset.unwrap_or_else(|| "Today".to_string());
    let show_complete = q.show_complete.is_some();
    let (after, before) = preset_range(&preset, Utc::now(), tz_offset);

    let due_items = repo
        .list_due_team_items(&team_id, after.map(|d| d.timestamp()), before.map(|d| d.timestamp()))
        .await
        .map_err(ItemError::from)?;
    let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
    let rows = render_rows(&due_items, &team_id, &names, &preset, show_complete, tz_offset)?;

    let presets = PRESETS.iter().map(|&p| (p, p == preset)).collect();
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::None,
    )
    .await?;
    render(TeamDashboardPageTemplate {
        team_id,
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

pub async fn toggle_team_dashboard_item_complete(
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
        description: current.description.clone(),
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
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        points: current.points(),
    };
    team_item_service::update_team_item(
        &repo,
        &UpdateTeamItemContext { teams: teams.clone(), activity_log },
        &auth_user.user_id,
        params,
    )
    .await?;

    let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
    match repo.get_team_item(&team_id, &item_id).await {
        Ok(updated) => render(TeamDashboardRow::from_due_item(
            &DueItem { parent_name: String::new(), item: updated },
            &team_id,
            &names,
            tz,
        )),
        // Recurring item just completed and got replaced under a new id (see
        // service::team_items::update_team_item) — nothing to render back for the old id.
        Err(RepoError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(ItemError::from(e)),
    }
}
