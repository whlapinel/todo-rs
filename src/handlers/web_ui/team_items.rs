use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemType};
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::error::ItemError;
use crate::service::team_items::{
    self as team_item_service, require_active_member, CreateTeamItemParams, UpdateTeamItemParams,
};
use crate::service::teams as team_service;
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

// Mirrors web_ui::items's form-parsing helpers exactly (same three-way None/empty/value read
// per field) — see the comment there for why. Not shared as a common function between the two
// files because `CreateTeamItemParams`/`UpdateTeamItemParams` and `CreateItemParams`/
// `UpdateItemParams` are distinct generated-adjacent structs with an extra `assigned_to_user_id`
// field here; a shared helper would need to abstract over that difference for no real benefit.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamItemForm {
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
    has_tasks: Option<String>,
    parent_item_id: Option<String>,
    item_type: Option<String>,
    event_type: Option<String>,
    assigned_to_user_id: Option<String>,
    show_complete: Option<String>,
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

fn overlay_has_tasks(form_value: &Option<String>, current: bool) -> bool {
    match form_value.as_deref() {
        Some("simple") => false,
        Some("tasks") => true,
        _ => current,
    }
}

fn overlay_has_due_time(form_time: &Option<String>, current: bool) -> bool {
    match form_time {
        None => current,
        Some(s) => !s.trim().is_empty(),
    }
}

fn parse_item_type(form_value: &Option<String>) -> Option<ItemType> {
    non_empty(form_value).and_then(|s| s.parse().ok())
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
        Some(s) => combine_local_to_utc(s.trim(), form_time.as_deref(), tz_offset_minutes, end_of_day()),
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
        Some(s) => combine_local_to_utc(s.trim(), form_time.as_deref(), tz_offset_minutes, start_of_day()),
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
        Some(s) => combine_local_to_utc(s.trim(), form_time.as_deref(), tz_offset_minutes, end_of_day()),
    }
}

fn create_params_from_form(
    team_id: &str,
    form: &TeamItemForm,
    tz: i32,
) -> CreateTeamItemParams {
    CreateTeamItemParams {
        team_id: team_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, None),
        scheduled_date: overlay_scheduled_date(&form.scheduled_date, &form.scheduled_time, tz, None),
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
        has_end_time: form.scheduled_end_time.as_deref().map(|t| !t.trim().is_empty()),
        has_tasks: match form.has_tasks.as_deref() {
            Some("simple") => Some(false),
            Some("tasks") => Some(true),
            _ => None,
        },
        parent_item_id: non_empty(&form.parent_item_id),
        item_type: parse_item_type(&form.item_type),
        event_type: non_empty(&form.event_type),
        due_offset_days: form
            .due_offset_days
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        assigned_to_user_id: non_empty(&form.assigned_to_user_id),
        timezone_offset_minutes: Some(tz),
    }
}

fn update_params_from_form(
    team_id: &str,
    item_id: &str,
    current: &Item,
    form: &TeamItemForm,
    tz: i32,
) -> UpdateTeamItemParams {
    UpdateTeamItemParams {
        team_id: team_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, current.due_date),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            current.scheduled_date,
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            current.scheduled_end_date,
        ),
        complete: overlay_bool(&form.complete, current.complete),
        recurrence: overlay_str(&form.recurrence, current.recurrence.clone()),
        recurrence_basis: overlay_str(&form.recurrence_basis, current.recurrence_basis.clone()),
        has_due_time: Some(overlay_has_due_time(&form.due_time, current.has_due_time)),
        has_scheduled_time: Some(overlay_has_due_time(&form.scheduled_time, current.has_scheduled_time)),
        has_end_time: Some(overlay_has_due_time(&form.scheduled_end_time, current.has_end_time)),
        has_tasks: Some(overlay_has_tasks(&form.has_tasks, current.has_tasks)),
        parent_item_id: current.parent_item_id.clone(),
        item_type: parse_item_type(&form.item_type),
        event_type: overlay_str(&form.event_type, current.event_type.clone()),
        due_offset_days: overlay_i32(&form.due_offset_days, current.due_offset_days),
        assigned_to_user_id: overlay_str(
            &form.assigned_to_user_id,
            current.assigned_to_user_id.clone(),
        ),
        timezone_offset_minutes: Some(tz),
    }
}

/// (user_id, display name) for every *active* member of `team_id` — the assignee dropdown's
/// candidate list, resolved server-side at render time (unlike the SPA's separate
/// `loadTeamMembers` fetch).
async fn active_member_options(
    teams: &Arc<dyn TeamRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<Vec<(String, String)>, ItemError> {
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;
    Ok(members
        .into_iter()
        .filter(|m| m.status == "ACTIVE")
        .map(|m| (m.user.id, format!("{} {}", m.user.first_name, m.user.last_name)))
        .collect())
}

// ---- templates --------------------------------------------------------------

#[derive(Template)]
#[template(path = "team_items/row.html")]
struct TeamItemRow {
    id: String,
    team_id: String,
    name: String,
    complete: bool,
    due_date: Option<String>,
    scheduled_date: Option<String>,
    has_children: bool,
    offset_label: Option<String>,
    recurrence: Option<String>,
    assignee_name: Option<String>,
    toggle_complete_json: String,
}

impl TeamItemRow {
    fn from_item(item: &Item, team_id: &str, names: &HashMap<String, String>, tz: i32) -> Self {
        let offset_label = item
            .parent_item_id
            .as_ref()
            .map(|_| match item.due_offset_days {
                Some(0) => "on due date".to_string(),
                Some(n) if n > 0 => format!("+{n}d"),
                Some(n) => format!("{n}d"),
                None => "no offset".to_string(),
            });
        Self {
            id: item.id.clone(),
            team_id: team_id.to_string(),
            name: item.name.clone(),
            complete: item.complete,
            due_date: item
                .due_date
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            scheduled_date: item.scheduled_date.map(|d| {
                let local = to_local(d, tz);
                if item.has_scheduled_time {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            has_children: item.has_children,
            offset_label,
            recurrence: item.recurrence.clone(),
            assignee_name: item
                .assigned_to_user_id
                .as_ref()
                .map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone())),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

fn format_offset_input(due_offset_days: Option<i32>) -> String {
    due_offset_days.map(|d| d.to_string()).unwrap_or_default()
}

#[derive(Template)]
#[template(path = "team_items/detail_fields.html")]
struct DetailFields {
    id: String,
    team_id: String,
    name: String,
    complete: bool,
    has_tasks: bool,
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
    is_event: bool,
    event_type_input: String,
    assignee_options: Vec<(String, String)>,
    assigned_to_user_id: Option<String>,
    just_saved: bool,
}

impl DetailFields {
    fn from_item(
        item: &Item,
        team_id: &str,
        assignee_options: Vec<(String, String)>,
        tz: i32,
        just_saved: bool,
    ) -> Self {
        let local_due_date = item.due_date.map(|d| to_local(d, tz));
        let due_date_input = local_due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time {
            local_due_date.map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_date = item.scheduled_date.map(|d| to_local(d, tz));
        let scheduled_date_input = local_scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_time_input = if item.has_scheduled_time {
            local_scheduled_date.map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_end_date = item.scheduled_end_date.map(|d| to_local(d, tz));
        let scheduled_end_date_input = local_scheduled_end_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_end_time_input = if item.has_end_time {
            local_scheduled_end_date.map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            team_id: team_id.to_string(),
            name: item.name.clone(),
            complete: item.complete,
            has_tasks: item.has_tasks,
            is_top_level: item.parent_item_id.is_none(),
            due_date_input,
            due_time_input,
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            recurrence: item.recurrence.clone(),
            recurrence_basis: item.recurrence_basis.clone(),
            due_offset_days_input: format_offset_input(item.due_offset_days),
            is_event: item.item_type == ItemType::Event,
            event_type_input: item.event_type.clone().unwrap_or_default(),
            assignee_options,
            assigned_to_user_id: item.assigned_to_user_id.clone(),
            just_saved,
        }
    }
}

#[derive(Template)]
#[template(path = "team_items/rows_fragment.html")]
struct RowsFragmentTemplate {
    rows: Vec<String>,
    empty_message: String,
}

#[derive(Template)]
#[template(path = "team_items/list_page.html")]
struct TeamItemsListPageTemplate {
    team_id: String,
    rows: Vec<String>,
    show_complete: bool,
    assignee_options: Vec<(String, String)>,
    blank_recurrence: Option<String>,
    blank_recurrence_basis: Option<String>,
    blank_due_offset_days_input: String,
    blank_event_type_input: String,
    blank_scheduled_date_input: String,
    blank_scheduled_time_input: String,
    blank_scheduled_end_date_input: String,
    blank_scheduled_end_time_input: String,
}

#[derive(Template)]
#[template(path = "team_items/detail_page.html")]
struct TeamItemDetailPageTemplate {
    id: String,
    team_id: String,
    name: String,
    fields: String,
}

// ---- shared rendering helpers ------------------------------------------------

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
        .map(|i| TeamItemRow::from_item(i, team_id, names, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
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
            repo.list_children(parent_id).await.map_err(ItemError::from)?,
            "No sub-items yet.",
        )
    } else {
        (
            repo.list_team_items(team_id, None).await.map_err(ItemError::from)?,
            "No items yet.",
        )
    };
    let names = names_for(teams, team_id, requester_user_id).await?;
    let rows = render_rows(&items, team_id, &names, parent_item_id.is_some() || show_complete, tz)?;
    render(RowsFragmentTemplate {
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

pub async fn team_items_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let show_complete = q.show_complete.is_some();
    let items = repo
        .list_team_items(&team_id, None)
        .await
        .map_err(ItemError::from)?;
    let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
    let rows = render_rows(&items, &team_id, &names, show_complete, tz)?;
    let assignee_options = active_member_options(&teams, &team_id, &auth_user.user_id).await?;
    render(TeamItemsListPageTemplate {
        team_id,
        rows,
        show_complete,
        assignee_options,
        blank_recurrence: None,
        blank_recurrence_basis: Some("SCHEDULED_DATE".to_string()),
        blank_due_offset_days_input: String::new(),
        blank_event_type_input: String::new(),
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
    })
}

pub async fn team_item_detail_page(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let item = repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    let assignee_options = active_member_options(&teams, &team_id, &auth_user.user_id).await?;
    let fields = DetailFields::from_item(&item, &team_id, assignee_options, tz, false).render()?;
    render(TeamItemDetailPageTemplate {
        id: item.id,
        team_id,
        name: item.name,
        fields,
    })
}

pub async fn team_item_children_fragment(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    // Ownership gate: list_children isn't scoped by team, so confirm the parent actually
    // belongs to this team before listing its children (mirrors web_ui::items's equivalent).
    repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    let children = repo.list_children(&item_id).await.map_err(ItemError::from)?;
    let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
    let rows = render_rows(&children, &team_id, &names, true, tz)?;
    render(RowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

pub async fn create_team_item_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TeamItemForm>,
) -> Result<Html<String>, ItemError> {
    let show_complete = form.show_complete.is_some();
    let params = create_params_from_form(&team_id, &form, tz);
    let parent_item_id = params.parent_item_id.clone();
    team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;
    render_scope_fragment(
        &repo,
        &teams,
        &team_id,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        show_complete,
        tz,
    )
    .await
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchForm {
    names: String,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
}

pub async fn create_team_items_batch(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchForm>,
) -> Result<Html<String>, ItemError> {
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
            timezone_offset_minutes: Some(tz),
            ..Default::default()
        };
        team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;
    }
    render_scope_fragment(
        &repo,
        &teams,
        &team_id,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
        tz,
    )
    .await
}

pub async fn update_team_item_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TeamItemForm>,
) -> Result<Response, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let current = repo.get_team_item(&team_id, &item_id).await.map_err(ItemError::from)?;
    let params = update_params_from_form(&team_id, &item_id, &current, &form, tz);
    team_item_service::update_team_item(&repo, &teams, &auth_user.user_id, params).await?;

    match repo.get_team_item(&team_id, &item_id).await {
        Ok(updated) => {
            let names = names_for(&teams, &team_id, &auth_user.user_id).await?;
            let row = TeamItemRow::from_item(&updated, &team_id, &names, tz).render()?;
            let assignee_options = active_member_options(&teams, &team_id, &auth_user.user_id).await?;
            let fields =
                DetailFields::from_item(&updated, &team_id, assignee_options, tz, true).render()?;
            Ok(Html(format!("{row}{fields}")).into_response())
        }
        // Recurring item just completed and got replaced under a new id (see
        // service::team_items::update_team_item) — nothing to swap back in under the old id.
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

pub async fn delete_team_item_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    team_item_service::delete_team_item(&repo, &teams, &auth_user.user_id, &team_id, &item_id)
        .await?;
    Ok(Html(String::new()))
}
