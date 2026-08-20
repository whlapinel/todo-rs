use super::nav::{self, ActiveContext, SidebarSection};
use super::project_dashboard::detail_url;
use super::{TzOffset, to_local};
use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{self as event_series_service, OccurrenceState, ProjectOccurrence};
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::storage::sqlite::{
    ActivityLogRepo, DueItem, ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo, UserRepo,
};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::Html;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Duplicated from `project_dashboard.rs` rather than shared — that module's own equivalents
/// are private, following the same precedent its own doc comments cite (every B5 sub-stage
/// duplicated small per-screen helpers rather than widening a sibling module's visibility just
/// to reuse a handful of functions).
fn preset_range(
    preset: &str,
    now: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let offset = Duration::minutes(tz_offset_minutes as i64);
    let local_now = now - offset;
    let local_date = local_now.date_naive();
    let to_utc = |naive: chrono::NaiveDateTime| {
        DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc) + offset
    };
    let today_start = to_utc(local_date.and_hms_opt(0, 0, 0).unwrap());

    match preset {
        "Today" => (
            Some(today_start),
            Some(to_utc(local_date.and_hms_opt(23, 59, 59).unwrap())),
        ),
        "This Week" => (Some(today_start), Some(today_start + Duration::days(7))),
        "Next 30 Days" => (Some(today_start), Some(today_start + Duration::days(30))),
        "Overdue" => (None, Some(now)),
        _ => (None, None),
    }
}

const PRESETS: [&str; 6] = [
    "All",
    "All with due date",
    "Today",
    "This Week",
    "Next 30 Days",
    "Overdue",
];

fn dashboard_date(item: &Item) -> Option<DateTime<Utc>> {
    match item.kind() {
        ItemKind::Event => item.scheduled_date().or(item.due_date()),
        _ => item.due_date(),
    }
}

fn dashboard_has_time(item: &Item) -> bool {
    match item.kind() {
        ItemKind::Event if item.scheduled_date().is_some() => item.has_scheduled_time(),
        ItemKind::Event => item.has_due_time(),
        _ => item.has_due_time(),
    }
}

fn type_symbol(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Task => "T",
        ItemKind::Event => "E",
        ItemKind::Simple => "S",
        ItemKind::Template => "Tmpl",
    }
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

/// Cross-project scoping rule (see docs/features.md's "Main dashboard" note): Events are
/// never assignment-gated (no personal/team distinction that matters for them, and no
/// `assignedToUserId` concept prevents a personal-project Event from having one anyway); a
/// Task is unrestricted on a personal project (single member, so "assigned to me" is moot)
/// but restricted to the requester's own assignment on a team-backed one — otherwise this
/// screen would show every team member's tasks, defeating its "what's mine, across every
/// project" purpose. Simple/Template items never carry a due/scheduled date worth showing
/// here at all (mirrors `project_dashboard::render_rows`'s own `ItemKind::Simple` exclusion,
/// widened to also exclude Template).
fn is_included(kind: ItemKind, is_team_project: bool, assigned_to: Option<&str>, user_id: &str) -> bool {
    match kind {
        ItemKind::Event => true,
        ItemKind::Task => !is_team_project || assigned_to == Some(user_id),
        ItemKind::Simple | ItemKind::Template => false,
    }
}

/// Same rationale as `project_dashboard::virtual_occurrence_window` — bounds how far ahead an
/// indefinitely-repeating series is expanded for display when a preset leaves the window open.
const VIRTUAL_OCCURRENCE_DEFAULT_WINDOW_DAYS: i64 = 90;

fn virtual_occurrence_window(
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        after.unwrap_or(now),
        before.unwrap_or(now + Duration::days(VIRTUAL_OCCURRENCE_DEFAULT_WINDOW_DAYS)),
    )
}

#[derive(Template)]
#[template(path = "main_dashboard/row.html")]
struct MainDashboardRow {
    item_id: String,
    project_name: String,
    name: String,
    complete: bool,
    date_label: Option<String>,
    date_kind_label: &'static str,
    overdue: bool,
    type_symbol: &'static str,
    parent_name: Option<String>,
    assignee_name: Option<String>,
    toggle_target: Option<String>,
    detail_link: String,
    toggle_complete_json: String,
}

impl MainDashboardRow {
    fn from_due_item(
        di: &DueItem,
        project_id: &str,
        project_name: &str,
        names: &HashMap<String, String>,
        tz: i32,
    ) -> Self {
        let item = &di.item;
        let date_label = dashboard_date(item).map(|d| {
            let local = to_local(d, tz);
            if dashboard_has_time(item) {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let date_kind_label = if item.kind() == ItemKind::Event && item.scheduled_date().is_some() {
            "Scheduled"
        } else {
            "Due"
        };
        Self {
            item_id: item.id.clone(),
            project_name: project_name.to_string(),
            name: item.name.clone(),
            complete: item.complete,
            date_label,
            date_kind_label,
            overdue: item.is_overdue(Utc::now()),
            type_symbol: type_symbol(item.kind()),
            parent_name: if di.parent_name.is_empty() {
                None
            } else {
                Some(di.parent_name.clone())
            },
            assignee_name: item
                .assigned_to_user_id()
                .and_then(|id| names.get(&id).cloned()),
            toggle_target: (item.kind() != ItemKind::Event)
                .then(|| format!("/web/dashboard/projects/{project_id}/items/{}", item.id)),
            detail_link: detail_url(item, project_id),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "main_dashboard/virtual_row.html")]
struct MainDashboardVirtualRow {
    series_id: String,
    occurrence_ts: i64,
    project_name: String,
    name: String,
    date_label: String,
    date_kind_label: &'static str,
    type_symbol: &'static str,
    title: String,
    materialize_url: String,
    skip_url: String,
    is_current: bool,
    assignee_name: Option<String>,
    is_skipped: bool,
    unskip_url: String,
}

impl MainDashboardVirtualRow {
    fn from_occurrence(occ: &ProjectOccurrence, project_id: &str, project_name: &str, tz: i32) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        let kind_name = if occ.item_type == ItemKind::Event {
            "Event"
        } else {
            "Task"
        };
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            project_name: project_name.to_string(),
            name: occ.series_name.clone(),
            date_label: local.format("%Y-%m-%d %H:%M").to_string(),
            date_kind_label: if occ.item_type == ItemKind::Event {
                "Scheduled"
            } else {
                "Due"
            },
            type_symbol: type_symbol(occ.item_type),
            title: format!("{kind_name} (not yet created)"),
            materialize_url: occ.materialize_url(project_id),
            skip_url: occ.skip_url(project_id),
            is_current: occ.is_current,
            assignee_name: occ.assigned_to_user_name.clone(),
            is_skipped: occ.is_skipped(),
            unskip_url: occ.unskip_url(project_id),
        }
    }
}

#[derive(Template)]
#[template(path = "main_dashboard/page.html")]
struct MainDashboardPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    presets: Vec<(&'static str, bool)>,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MainDashboardQuery {
    preset: Option<String>,
    show_complete: Option<String>,
}

pub async fn main_dashboard_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz_offset): TzOffset,
    Query(q): Query<MainDashboardQuery>,
) -> Result<Html<String>, ItemError> {
    let preset = q.preset.unwrap_or_else(|| "Today".to_string());
    let show_complete = q.show_complete.is_some();
    let (after, before) = preset_range(&preset, Utc::now(), tz_offset);
    let (virtual_after, virtual_before) = virtual_occurrence_window(after, before, Utc::now());

    let user_projects = project_service::list_projects(&projects, &auth_user.user_id).await?;

    let mut entries: Vec<(i64, String)> = Vec::new();
    for project in &user_projects {
        let is_team_project = project.team_id.is_some();
        let names = match &project.team_id {
            Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
            None => HashMap::new(),
        };
        let due_items =
            project_item_service::list_due_project_items_unchecked(&repo, &project.id, None, None)
                .await?;
        let virtual_occurrences = if virtual_after <= virtual_before {
            event_series_service::list_occurrence_states_for_project(
                &event_series,
                &users,
                &project.id,
                virtual_after,
                virtual_before,
                tz_offset,
            )
            .await?
        } else {
            Vec::new()
        };

        for di in &due_items {
            let item = &di.item;
            if !is_included(
                item.kind(),
                is_team_project,
                item.assigned_to_user_id().as_deref(),
                &auth_user.user_id,
            ) {
                continue;
            }
            if !show_complete && item.complete {
                continue;
            }
            if preset == "All with due date" && dashboard_date(item).is_none() {
                continue;
            }
            let d = dashboard_date(item);
            let in_window = match d {
                Some(d) => after.is_none_or(|a| d >= a) && before.is_none_or(|b| d <= b),
                None => after.is_none() && before.is_none(),
            };
            if !in_window {
                continue;
            }
            let ts = d.map(|d| d.timestamp()).unwrap_or(i64::MAX);
            let html = MainDashboardRow::from_due_item(di, &project.id, &project.name, &names, tz_offset)
                .render()?;
            entries.push((ts, html));
        }

        for occ in &virtual_occurrences {
            if matches!(occ.state, OccurrenceState::Materialized { .. }) {
                continue;
            }
            if !is_included(
                occ.item_type,
                is_team_project,
                occ.assigned_to_user_id.as_deref(),
                &auth_user.user_id,
            ) {
                continue;
            }
            entries.push((
                occ.occurrence_date.timestamp(),
                MainDashboardVirtualRow::from_occurrence(occ, &project.id, &project.name, tz_offset)
                    .render()?,
            ));
        }
    }

    entries.sort_by_key(|(ts, _)| *ts);
    let rows = entries.into_iter().map(|(_, html)| html).collect();

    let presets = PRESETS.iter().map(|&p| (p, p == preset)).collect();
    let nav_html =
        nav::build_nav_html(&projects, &auth_user.user_id, ActiveContext::None, SidebarSection::None)
            .await?;
    render(MainDashboardPageTemplate {
        rows,
        show_complete,
        presets,
        nav_html,
    })
}

struct MainDashboardCalendarEntry {
    entry_id: String,
    detail_link: String,
    name: String,
    project_name: String,
    time_label: Option<String>,
    type_symbol: &'static str,
    materialize_url: Option<String>,
    skip_url: Option<String>,
    is_virtual: bool,
    is_current: bool,
    complete: bool,
    is_skipped: bool,
    unskip_url: Option<String>,
}

struct MainDashboardCalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    entries: Vec<MainDashboardCalendarEntry>,
}

#[derive(Template)]
#[template(path = "main_dashboard/calendar_page.html")]
struct MainDashboardCalendarPageTemplate {
    month_label: String,
    month_iso: String,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<MainDashboardCalendarDay>,
    nav_html: String,
}

fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

fn grid_start_for(year: i32, month: u32) -> NaiveDate {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    first_of_month - Duration::days(leading as i64)
}

fn local_date_to_utc(
    date: NaiveDate,
    time: chrono::NaiveTime,
    tz_offset_minutes: i32,
) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(date.and_time(time), Utc)
        + Duration::minutes(tz_offset_minutes as i64)
}

fn start_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}

fn end_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap()
}

/// Cross-project counterpart to `project_dashboard::build_calendar_days` — takes each due
/// item/occurrence pre-tagged with the project it came from (`is_included` filtering already
/// applied by the caller), since a single grid here can mix rows from every project the user
/// belongs to.
fn build_calendar_days(
    year: i32,
    month: u32,
    due_items: &[(DueItem, String, String)],
    virtual_occurrences: &[(ProjectOccurrence, String, String)],
    tz: i32,
    today: NaiveDate,
) -> Vec<MainDashboardCalendarDay> {
    let grid_start = grid_start_for(year, month);

    let mut by_date: HashMap<NaiveDate, Vec<MainDashboardCalendarEntry>> = HashMap::new();
    for (di, project_id, project_name) in due_items {
        let item = &di.item;
        if let Some(dt) = dashboard_date(item) {
            let local = to_local(dt, tz);
            let time_label = dashboard_has_time(item).then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(MainDashboardCalendarEntry {
                    entry_id: format!("main-cal-item-{}", item.id),
                    detail_link: detail_url(item, project_id),
                    name: item.name.clone(),
                    project_name: project_name.clone(),
                    time_label,
                    type_symbol: type_symbol(item.kind()),
                    materialize_url: None,
                    skip_url: None,
                    is_virtual: false,
                    is_current: false,
                    complete: item.complete,
                    is_skipped: false,
                    unskip_url: None,
                });
        }
    }
    for (occ, project_id, project_name) in virtual_occurrences {
        let local = to_local(occ.occurrence_date, tz);
        by_date
            .entry(local.date_naive())
            .or_default()
            .push(MainDashboardCalendarEntry {
                entry_id: occ.calendar_entry_id(),
                detail_link: "#".to_string(),
                name: occ.series_name.clone(),
                project_name: project_name.clone(),
                time_label: Some(local.format("%H:%M").to_string()),
                type_symbol: type_symbol(occ.item_type),
                materialize_url: Some(occ.materialize_url(project_id)),
                skip_url: Some(occ.skip_url(project_id)),
                is_virtual: true,
                is_current: occ.is_current,
                complete: false,
                is_skipped: occ.is_skipped(),
                unskip_url: Some(occ.unskip_url(project_id)),
            });
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + Duration::days(i);
        let mut entries = by_date.remove(&date).unwrap_or_default();
        entries.sort_by(|a, b| a.time_label.cmp(&b.time_label));
        days.push(MainDashboardCalendarDay {
            date: date.format("%Y-%m-%d").to_string(),
            day_number: date.day(),
            is_current_month: date.month() == month && date.year() == year,
            is_today: date == today,
            entries,
        });
    }
    days
}

#[derive(serde::Deserialize)]
pub struct CalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
}

pub async fn main_dashboard_calendar_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let today = to_local(Utc::now(), tz).date_naive();
    let year = q.year.unwrap_or_else(|| today.year());
    let month = q
        .month
        .filter(|m| (1..=12).contains(m))
        .unwrap_or_else(|| today.month());

    let grid_start = grid_start_for(year, month);
    let range_start = local_date_to_utc(grid_start, start_of_day(), tz);
    let range_end = local_date_to_utc(grid_start + Duration::days(41), end_of_day(), tz);

    let user_projects = project_service::list_projects(&projects, &auth_user.user_id).await?;

    let mut due_bucket: Vec<(DueItem, String, String)> = Vec::new();
    let mut occ_bucket: Vec<(ProjectOccurrence, String, String)> = Vec::new();

    for project in &user_projects {
        let is_team_project = project.team_id.is_some();
        let due_items =
            project_item_service::list_due_project_items_unchecked(&repo, &project.id, None, None)
                .await?;
        for di in due_items {
            if is_included(
                di.item.kind(),
                is_team_project,
                di.item.assigned_to_user_id().as_deref(),
                &auth_user.user_id,
            ) {
                due_bucket.push((di, project.id.clone(), project.name.clone()));
            }
        }
        let occurrences = event_series_service::list_occurrence_states_for_project(
            &event_series,
            &users,
            &project.id,
            range_start,
            range_end,
            tz,
        )
        .await?
        .into_iter()
        .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
        .filter(|occ| {
            is_included(
                occ.item_type,
                is_team_project,
                occ.assigned_to_user_id.as_deref(),
                &auth_user.user_id,
            )
        });
        for occ in occurrences {
            occ_bucket.push((occ, project.id.clone(), project.name.clone()));
        }
    }

    let days = build_calendar_days(year, month, &due_bucket, &occ_bucket, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html =
        nav::build_nav_html(&projects, &auth_user.user_id, ActiveContext::None, SidebarSection::None)
            .await?;

    render(MainDashboardCalendarPageTemplate {
        month_label: NaiveDate::from_ymd_opt(year, month, 1)
            .unwrap()
            .format("%B %Y")
            .to_string(),
        month_iso: format!("{year:04}-{month:02}"),
        prev_year,
        prev_month,
        next_year,
        next_month,
        days,
        nav_html,
    })
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToggleForm {
    complete: Option<String>,
}

pub async fn toggle_main_dashboard_item_complete(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ToggleForm>,
) -> Result<Html<String>, ItemError> {
    let current = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let params = UpdateProjectItemParams {
        project_id: project_id.clone(),
        item_id: item_id.clone(),
        name: current.name.clone(),
        description: current.description.clone(),
        due_date: current.due_date(),
        scheduled_date: current.scheduled_date(),
        scheduled_end_date: current.scheduled_end_date(),
        complete: form.complete.as_deref() == Some("true"),
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
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &event_series,
        &auth_user.user_id,
        params,
    )
    .await?;

    let project = project_item_service::get_project_unchecked(&projects, &project_id).await?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    match project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await {
        Ok(updated) => render(MainDashboardRow::from_due_item(
            &DueItem {
                parent_name: String::new(),
                item: updated,
            },
            &project_id,
            &project.name,
            &names,
            tz,
        )),
        // See `project_dashboard::toggle_project_dashboard_item_complete`'s identical
        // rationale for this branch.
        Err(ItemError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(e),
    }
}
