use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::error::ItemError;
use crate::service::team_items::{
    self as team_item_service, require_active_member, CreateTeamItemParams, UpdateTeamItemContext,
    UpdateTeamItemParams,
};
use crate::service::teams as team_service;
use crate::service::templates::{self as template_service, CreateTeamTemplateParams};
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being a Task, the same role
/// `tasks::require_task` plays for the personal screen — this screen's forms hardcode
/// `itemType: TASK` on every create/update (no Kind selector), so an Event or Simple
/// team item's id reaching one of these handlers must 404 rather than silently
/// reclassify it back to Task on save.
fn require_team_task(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Task {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Mirrors `team_items.rs`'s helper set exactly (see that file's comment on why this isn't
// shared with `tasks.rs`'s near-identical set instead) plus `tasks.rs`'s choice to hardcode
// `itemType: TASK` rather than exposing a Kind selector.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamTaskForm {
    name: Option<String>,
    due_date: Option<String>,
    due_time: Option<String>,
    scheduled_date: Option<String>,
    scheduled_time: Option<String>,
    scheduled_end_date: Option<String>,
    scheduled_end_time: Option<String>,
    complete: Option<String>,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    due_offset_days: Option<String>,
    parent_item_id: Option<String>,
    assigned_to_user_id: Option<String>,
    /// Admin-only — the service layer strips/preserves this for non-admins regardless of
    /// what's submitted (see `team_item_service::create_team_item`/`update_team_item`). No
    /// `<input>` renders this yet on either form (Stage 7 adds the actual template work); the
    /// parsing exists now so that stage only has to touch templates, not this module.
    points: Option<String>,
    show_complete: Option<String>,
    /// Present only on the standalone `/team-tasks/:team_id/new` page's forms — see
    /// `items.rs`'s identical field for the full rationale.
    redirect: Option<String>,
}

fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn overlay_str(form_value: &Option<String>, current: Option<String>) -> Option<String> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
}

fn overlay_required_str(form_value: &Option<String>, current: &str) -> String {
    match form_value {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => current.to_string(),
    }
}

fn overlay_i32(form_value: &Option<String>, current: Option<i32>) -> Option<i32> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => s.trim().parse().ok().or(current),
    }
}

fn overlay_bool(form_value: &Option<String>, current: bool) -> bool {
    match form_value.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => current,
    }
}

fn overlay_has_due_time(form_time: &Option<String>, current: bool) -> bool {
    match form_time {
        None => current,
        Some(s) => !s.trim().is_empty(),
    }
}

fn combine_local_to_utc(
    date: &str,
    time: Option<&str>,
    tz_offset_minutes: i32,
    default_time: chrono::NaiveTime,
) -> Option<DateTime<Utc>> {
    let naive_date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let naive_time = time
        .filter(|t| !t.trim().is_empty())
        .and_then(|t| chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M").ok())
        .unwrap_or(default_time);
    let naive = naive_date.and_time(naive_time);
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Some(as_utc + chrono::Duration::minutes(tz_offset_minutes as i64))
}

fn end_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap()
}

fn start_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}

fn overlay_due_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            end_of_day(),
        ),
    }
}

fn overlay_scheduled_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            start_of_day(),
        ),
    }
}

fn overlay_scheduled_end_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            end_of_day(),
        ),
    }
}

fn create_params_from_form(team_id: &str, form: &TeamTaskForm, tz: i32) -> CreateTeamItemParams {
    CreateTeamItemParams {
        team_id: team_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, None),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            None,
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            None,
        ),
        complete: form.complete.as_deref().map(|s| s == "true"),
        recurrence: non_empty(&form.recurrence),
        recurrence_basis: non_empty(&form.recurrence_basis),
        has_due_time: form.due_time.as_deref().map(|t| !t.trim().is_empty()),
        has_scheduled_time: form.scheduled_time.as_deref().map(|t| !t.trim().is_empty()),
        has_end_time: form
            .scheduled_end_time
            .as_deref()
            .map(|t| !t.trim().is_empty()),
        parent_item_id: non_empty(&form.parent_item_id),
        item_type: Some(ItemKind::Task),
        due_offset_days: form
            .due_offset_days
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        assigned_to_user_id: non_empty(&form.assigned_to_user_id),
        timezone_offset_minutes: Some(tz),
        points: form
            .points
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        ..Default::default()
    }
}

fn update_params_from_form(
    team_id: &str,
    item_id: &str,
    current: &Item,
    form: &TeamTaskForm,
    tz: i32,
) -> UpdateTeamItemParams {
    UpdateTeamItemParams {
        team_id: team_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, current.due_date()),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            current.scheduled_date(),
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            current.scheduled_end_date(),
        ),
        complete: overlay_bool(&form.complete, current.complete),
        recurrence: overlay_str(&form.recurrence, current.recurrence_pattern()),
        recurrence_basis: overlay_str(&form.recurrence_basis, current.recurrence_basis()),
        has_due_time: Some(overlay_has_due_time(&form.due_time, current.has_due_time())),
        has_scheduled_time: Some(overlay_has_due_time(
            &form.scheduled_time,
            current.has_scheduled_time(),
        )),
        has_end_time: Some(overlay_has_due_time(
            &form.scheduled_end_time,
            current.has_end_time(),
        )),
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(ItemKind::Task),
        due_offset_days: overlay_i32(&form.due_offset_days, current.due_offset_days()),
        assigned_to_user_id: overlay_str(
            &form.assigned_to_user_id,
            current.assigned_to_user_id(),
        ),
        timezone_offset_minutes: Some(tz),
        // No points input renders on this form yet (Stage 7 adds it) — `overlay_i32` falls
        // back to `current.points` when the form field is absent, so a plain edit here can't
        // silently wipe it. The service layer's own admin gate is what actually decides
        // whether a *changed* value would be honored.
        points: overlay_i32(&form.points, current.points()),
        ..Default::default()
    }
}

/// (user_id, display name) for every *active* member of `team_id` — the assignee dropdown's
/// candidate list, mirroring `team_items.rs`'s `active_member_options`.
async fn active_member_options(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<Vec<(String, String)>, ItemError> {
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;
    Ok(members
        .into_iter()
        .filter(|m| m.status == "ACTIVE")
        .map(|m| {
            (
                m.user.id,
                format!("{} {}", m.user.first_name, m.user.last_name),
            )
        })
        .collect())
}

async fn names_for(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<HashMap<String, String>, ItemError> {
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;
    Ok(members
        .into_iter()
        .map(|m| {
            (
                m.user.id.clone(),
                format!("{} {}", m.user.first_name, m.user.last_name),
            )
        })
        .collect())
}

// ---- templates --------------------------------------------------------------

fn recurrence_basis_label(recurrence_basis: &Option<String>) -> String {
    match recurrence_basis.as_deref() {
        Some("COMPLETION_DATE") => "completion date".to_string(),
        Some("SCHEDULED_DATE") => "scheduled date".to_string(),
        Some(other) if other != "DUE_DATE" => other.to_string(),
        _ => "due date".to_string(),
    }
}

fn offset_label_for(item: &Item) -> Option<String> {
    item.parent_item_id
        .as_ref()
        .map(|_| match item.due_offset_days() {
            Some(0) => "on due date".to_string(),
            Some(n) if n > 0 => format!("+{n}d"),
            Some(n) => format!("{n}d"),
            None => "no offset".to_string(),
        })
}

fn format_offset_input(due_offset_days: Option<i32>) -> String {
    due_offset_days.map(|d| d.to_string()).unwrap_or_default()
}

fn format_points_input(points: Option<i32>) -> String {
    points.map(|p| p.to_string()).unwrap_or_default()
}

#[derive(Template)]
#[template(path = "team_tasks/row.html")]
struct TeamTaskRow {
    id: String,
    team_id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    overdue: bool,
    scheduled_date: Option<String>,
    has_children: bool,
    offset_label: Option<String>,
    recurrence: Option<String>,
    assignee_name: Option<String>,
    toggle_complete_json: String,
}

impl TeamTaskRow {
    fn from_item(item: &Item, team_id: &str, names: &HashMap<String, String>, tz: i32) -> Self {
        Self {
            id: item.id.clone(),
            team_id: team_id.to_string(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date()
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            overdue: item.is_overdue(Utc::now()),
            scheduled_date: item.scheduled_date().map(|d| {
                let local = to_local(d, tz);
                if item.has_scheduled_time() {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            has_children: item.has_children,
            offset_label: offset_label_for(item),
            recurrence: item.recurrence_pattern(),
            assignee_name: item
                .assigned_to_user_id()
                .map(|id| names.get(&id).cloned().unwrap_or(id)),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "team_tasks/detail_fields.html")]
struct TeamTaskDetailFields {
    id: String,
    team_id: String,
    name: String,
    complete: bool,
    is_top_level: bool,
    due_date_input: String,
    due_time_input: String,
    scheduled_date_input: String,
    scheduled_time_input: String,
    scheduled_end_date_input: String,
    scheduled_end_time_input: String,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    due_offset_days_input: String,
    assignee_options: Vec<(String, String)>,
    assigned_to_user_id: Option<String>,
    /// Gates the admin-only `points` input — see `macros::points_field` and
    /// `service::teams::require_team_admin`, whose result populates this at render time.
    is_team_admin: bool,
    points_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    just_saved: bool,
}

impl TeamTaskDetailFields {
    fn from_item(
        item: &Item,
        team_id: &str,
        assignee_options: Vec<(String, String)>,
        is_team_admin: bool,
        tz: i32,
        just_saved: bool,
    ) -> Self {
        let local_due_date = item.due_date().map(|d| to_local(d, tz));
        let due_date_input = local_due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time() {
            local_due_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_date = item.scheduled_date().map(|d| to_local(d, tz));
        let scheduled_date_input = local_scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_time_input = if item.has_scheduled_time() {
            local_scheduled_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_end_date = item.scheduled_end_date().map(|d| to_local(d, tz));
        let scheduled_end_date_input = local_scheduled_end_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_end_time_input = if item.has_end_time() {
            local_scheduled_end_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            team_id: team_id.to_string(),
            name: item.name.clone(),
            complete: item.complete,
            is_top_level: item.parent_item_id.is_none(),
            due_date_input,
            due_time_input,
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            recurrence: item.recurrence_pattern(),
            recurrence_basis: item.recurrence_basis(),
            due_offset_days_input: format_offset_input(item.due_offset_days()),
            assignee_options,
            assigned_to_user_id: item.assigned_to_user_id(),
            is_team_admin,
            points_input: format_points_input(item.points()),
            just_saved,
        }
    }
}

/// Read-only counterpart to `TeamTaskDetailFields` — see `items.rs`'s `DetailView` for the
/// row-editing convention this mirrors (complete-toggle lives here too).
#[derive(Template)]
#[template(path = "team_tasks/detail_view.html")]
struct TeamTaskDetailView {
    id: String,
    team_id: String,
    complete: bool,
    toggle_complete_json: String,
    due_date: Option<String>,
    overdue: bool,
    scheduled_date: Option<String>,
    scheduled_end_date: Option<String>,
    is_top_level: bool,
    recurrence: Option<String>,
    recurrence_basis_label: String,
    offset_label: Option<String>,
    assignee_name: Option<String>,
}

impl TeamTaskDetailView {
    fn from_item(item: &Item, team_id: &str, names: &HashMap<String, String>, tz: i32) -> Self {
        let due_date = item.due_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_due_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_date = item.scheduled_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_scheduled_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_end_date = item.scheduled_end_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_end_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        Self {
            id: item.id.clone(),
            team_id: team_id.to_string(),
            complete: item.complete,
            toggle_complete_json: (!item.complete).to_string(),
            due_date,
            overdue: item.is_overdue(Utc::now()),
            scheduled_date,
            scheduled_end_date,
            is_top_level: item.parent_item_id.is_none(),
            recurrence: item.recurrence_pattern(),
            recurrence_basis_label: recurrence_basis_label(&item.recurrence_basis()),
            offset_label: offset_label_for(item),
            assignee_name: item
                .assigned_to_user_id()
                .map(|id| names.get(&id).cloned().unwrap_or(id)),
        }
    }
}

#[derive(Template)]
#[template(path = "team_tasks/rows_fragment.html")]
struct TeamTaskRowsFragmentTemplate {
    rows: Vec<String>,
    empty_message: String,
}

#[derive(Template)]
#[template(path = "team_tasks/list_page.html")]
struct TeamTasksListPageTemplate {
    team_id: String,
    rows: Vec<String>,
    show_complete: bool,
    /// The viewer's own point balance on this team — see `service::teams::member_points`.
    points_label: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_tasks/new_page.html")]
struct NewTeamTaskPageTemplate {
    team_id: String,
    show_complete: bool,
    assignee_options: Vec<(String, String)>,
    blank_recurrence: Option<String>,
    blank_recurrence_basis: Option<String>,
    blank_due_offset_days_input: String,
    blank_scheduled_date_input: String,
    blank_scheduled_time_input: String,
    blank_scheduled_end_date_input: String,
    blank_scheduled_end_time_input: String,
    is_team_admin: bool,
    blank_points_input: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_tasks/detail_page.html")]
struct TeamTaskDetailPageTemplate {
    id: String,
    team_id: String,
    name: String,
    complete: bool,
    view: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_tasks/edit_page.html")]
struct TeamTaskEditPageTemplate {
    id: String,
    team_id: String,
    name: String,
    fields: String,
    nav_html: String,
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(
    items: &[Item],
    team_id: &str,
    names: &HashMap<String, String>,
    show_complete: bool,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .map(|i| TeamTaskRow::from_item(i, team_id, names, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// `repo.list_team_items` already scopes to top-level, non-Template items — this narrows
/// further to `Task` and sorts by due date (undated tasks last), mirroring `tasks.rs`'s
/// `sort_key`.
fn sort_key(item: &Item) -> i64 {
    item.due_date().map(|d| d.timestamp()).unwrap_or(i64::MAX)
}

async fn list_team_tasks(repo: &Arc<dyn ItemRepo>, team_id: &str) -> Result<Vec<Item>, ItemError> {
    let mut items = repo
        .list_team_items(team_id, None)
        .await
        .map_err(ItemError::from)?;
    items.retain(|i| i.kind() == ItemKind::Task);
    items.sort_by_key(sort_key);
    Ok(items)
}

async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
    parent_item_id: Option<&str>,
    show_complete: bool,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            repo.list_children(parent_id)
                .await
                .map_err(ItemError::from)?,
            "No sub-items yet.",
        )
    } else {
        (list_team_tasks(repo, team_id).await?, "No tasks yet.")
    };
    let names = names_for(teams, team_id, requester_user_id).await?;
    let rows = render_rows(
        &items,
        team_id,
        &names,
        parent_item_id.is_some() || show_complete,
        tz,
    )?;
    render(TeamTaskRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
}

// ---- handlers -----------------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowCompleteQuery {
    show_complete: Option<String>,
}

pub async fn team_tasks_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let show_complete = q.show_complete.is_some();
    let items = list_team_tasks(&repo, &team_id).await?;
    let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
    let rows = render_rows(&items, &team_id, &names, show_complete, tz)?;
    let points = team_service::member_points(&teams, &team_id, &auth_user.user_id).await?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Tasks,
    )
    .await?;
    render(TeamTasksListPageTemplate {
        team_id,
        rows,
        show_complete,
        points_label: format!("{points} pts"),
        nav_html,
    })
}

pub async fn new_team_task_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let assignee_options = active_member_options(&teams, &team_id, &auth_user.user_id).await?;
    let is_team_admin = team_service::is_team_admin(&teams, &team_id, &auth_user.user_id).await;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Tasks,
    )
    .await?;
    render(NewTeamTaskPageTemplate {
        team_id,
        show_complete: q.show_complete.is_some(),
        assignee_options,
        blank_recurrence: None,
        blank_recurrence_basis: Some("SCHEDULED_DATE".to_string()),
        blank_due_offset_days_input: String::new(),
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
        is_team_admin,
        blank_points_input: String::new(),
        nav_html,
    })
}

pub async fn team_task_detail_page(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let item = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_team_task(item)?;
    let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
    let view = TeamTaskDetailView::from_item(&item, &team_id, &names, tz).render()?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Tasks,
    )
    .await?;
    render(TeamTaskDetailPageTemplate {
        id: item.id,
        team_id,
        name: item.name,
        complete: item.complete,
        view,
        nav_html,
    })
}

pub async fn team_task_edit_page(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let item = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_team_task(item)?;
    let assignee_options = active_member_options(&teams, &team_id, &auth_user.user_id).await?;
    let is_team_admin = team_service::is_team_admin(&teams, &team_id, &auth_user.user_id).await;
    let fields = TeamTaskDetailFields::from_item(
        &item,
        &team_id,
        assignee_options,
        is_team_admin,
        tz,
        false,
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::Tasks,
    )
    .await?;
    render(TeamTaskEditPageTemplate {
        id: item.id,
        team_id,
        name: item.name,
        fields,
        nav_html,
    })
}

/// Renders a parent item's children as `TeamTaskRow`s — mirrors
/// `tasks::render_children_fragment` team-scoped; reused directly by `team_events.rs`
/// (which has no children concept of its own) since a team item's children are always
/// plain `Task`-typed regardless of their parent's type. Callers are responsible for their
/// own membership/ownership gate before calling this (see `team_task_children_fragment`).
pub(crate) async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    parent_item_id: &str,
    requester_user_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let children = repo
        .list_children(parent_item_id)
        .await
        .map_err(ItemError::from)?;
    let names = names_for(teams, team_id, requester_user_id).await?;
    let rows = render_rows(&children, team_id, &names, true, tz)?;
    render(TeamTaskRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

pub async fn team_task_children_fragment(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    // Ownership gate: list_children isn't scoped by team, so confirm the parent actually
    // belongs to this team before listing its children (mirrors team_items.rs's equivalent).
    repo.get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    render_children_fragment(&repo, &teams, &team_id, &item_id, &auth_user.user_id, tz).await
}

/// Redirect back to the team's tasks list (via the `hx-redirect` header) after a create from
/// the standalone `/team-tasks/:team_id/new` page. Mirrors `tasks.rs::redirect_to_tasks`.
fn redirect_to_team_tasks(team_id: &str, show_complete: bool) -> Response {
    let location = if show_complete {
        format!("/web/team-tasks/{team_id}?showComplete=1")
    } else {
        format!("/web/team-tasks/{team_id}")
    };
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            location,
        )],
        Html(String::new()),
    )
        .into_response()
}

pub async fn create_team_task_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TeamTaskForm>,
) -> Result<Response, ItemError> {
    let show_complete = form.show_complete.is_some();
    let redirect = form.redirect.is_some();
    let params = create_params_from_form(&team_id, &form, tz);
    let parent_item_id = params.parent_item_id.clone();
    team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;
    if redirect {
        return Ok(redirect_to_team_tasks(&team_id, show_complete));
    }
    Ok(render_scope_fragment(
        &repo,
        &teams,
        &team_id,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        show_complete,
        tz,
    )
    .await?
    .into_response())
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchForm {
    names: String,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    redirect: Option<String>,
}

pub async fn create_team_tasks_batch(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchForm>,
) -> Result<Response, ItemError> {
    let parent_item_id = non_empty(&form.parent_item_id);
    for line in form.names.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let params = CreateTeamItemParams {
            team_id: team_id.clone(),
            name: name.to_string(),
            parent_item_id: parent_item_id.clone(),
            item_type: Some(ItemKind::Task),
            timezone_offset_minutes: Some(tz),
            ..Default::default()
        };
        team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;
    }
    if form.redirect.is_some() {
        return Ok(redirect_to_team_tasks(
            &team_id,
            form.show_complete.is_some(),
        ));
    }
    Ok(render_scope_fragment(
        &repo,
        &teams,
        &team_id,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
        tz,
    )
    .await?
    .into_response())
}

pub async fn update_team_task_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TeamTaskForm>,
) -> Result<Response, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let current = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_team_task(current)?;
    let params = update_params_from_form(&team_id, &item_id, &current, &form, tz);
    team_item_service::update_team_item(
        &repo,
        &UpdateTeamItemContext {
            teams: teams.clone(),
            activity_log,
        },
        &auth_user.user_id,
        params,
    )
    .await?;

    match repo.get_team_item(&team_id, &item_id).await {
        Ok(updated) => {
            let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
            let row = TeamTaskRow::from_item(&updated, &team_id, &names, tz).render()?;
            let assignee_options =
                active_member_options(&teams, &team_id, &auth_user.user_id).await?;
            let is_team_admin =
                team_service::is_team_admin(&teams, &team_id, &auth_user.user_id).await;
            let fields = TeamTaskDetailFields::from_item(
                &updated,
                &team_id,
                assignee_options,
                is_team_admin,
                tz,
                true,
            )
            .render()?;
            let view = TeamTaskDetailView::from_item(&updated, &team_id, &names, tz).render()?;
            Ok(Html(format!("{row}{fields}{view}")).into_response())
        }
        // The task was recurring, just got marked complete, and the service layer replaced
        // it with a fresh successor under a new id (see service::team_items::update_team_item)
        // — same situation `team_items.rs`'s `update_team_item_form` handles.
        Err(RepoError::NotFound) => Ok((
            [(
                axum::http::header::HeaderName::from_static("hx-refresh"),
                "true",
            )],
            Html(String::new()),
        )
            .into_response()),
        Err(e) => Err(ItemError::from(e)),
    }
}

pub async fn delete_team_task_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    require_team_task(current)?;
    team_item_service::delete_team_item(&repo, &teams, &auth_user.user_id, &team_id, &item_id)
        .await?;
    Ok(Html(String::new()))
}

pub async fn save_team_task_as_template(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    template_service::create_team_template(
        &repo,
        &teams,
        CreateTeamTemplateParams {
            team_id,
            requester_user_id: auth_user.user_id,
            name: item.name.clone(),
            source_item_id: Some(item_id),
            event_type: None,
        },
    )
    .await?;
    Ok(Html(
        r#"<span class="text-xs text-green-600">Saved</span>"#.to_string(),
    ))
}
