use super::nav::{self, ActiveContext, SidebarSection};
use super::project_events::templates::ProjectEventRow;
use super::project_tasks::templates::ProjectTaskRow;
use super::{TzOffset, format_display_date, format_display_naive_date, to_local};
use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{self as series_service, OccurrenceState, ProjectOccurrence};
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::storage::sqlite::{
    ActivityLogRepo, DueItem, ItemRepo, ItemSeriesRepo, ProjectRepo, ReminderRepo, TeamRepo,
    UserRepo,
};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query, RawQuery};
use axum::response::{Html, Redirect};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

fn calendar_date(item: &Item) -> Option<DateTime<Utc>> {
    match item.kind() {
        ItemKind::Event => item.scheduled_date().or(item.due_date()),
        _ => item.due_date(),
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

/// Stage 4's All/Tasks/Events drawer tab filter — see `project_calendar::parse_type_filter`'s
/// identical rationale (duplicated rather than shared, matching this file's own established
/// precedent of duplicating small per-screen helpers rather than widening a sibling module's
/// visibility).
fn parse_type_filter(raw: Option<&str>) -> Option<ItemKind> {
    match raw {
        Some("task") => Some(ItemKind::Task),
        Some("event") => Some(ItemKind::Event),
        _ => None,
    }
}

fn active_type_label(type_filter: Option<ItemKind>) -> &'static str {
    match type_filter {
        Some(ItemKind::Task) => "task",
        Some(ItemKind::Event) => "event",
        _ => "all",
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

/// Cross-project scoping rule (see docs/archived/archived_issues_and_features.md's "Main dashboard" note): Events are
/// never assignment-gated (no personal/team distinction that matters for them, and no
/// `assignedToUserId` concept prevents a personal-project Event from having one anyway); a
/// Task is unrestricted on a personal project (single member, so "assigned to me" is moot)
/// but restricted to the requester's own assignment on a team-backed one — otherwise this
/// screen would show every team member's tasks, defeating its "what's mine, across every
/// project" purpose. Simple/Template items never carry a due/scheduled date worth showing
/// here at all.
///
/// `assigned_to_any` (added Stage 4 of docs/calendar-day-drawer-plan.md, for the calendar's own
/// assigned-to-me toggle) *relaxes* the team-backed-project Task restriction when set — it has
/// no effect on personal-project tasks, which were never restricted in the first place, and no
/// effect on Events, which were never restricted either.
fn is_included(
    kind: ItemKind,
    is_team_project: bool,
    assigned_to: Option<&str>,
    user_id: &str,
    assigned_to_any: bool,
) -> bool {
    match kind {
        ItemKind::Event => true,
        ItemKind::Task => !is_team_project || assigned_to_any || assigned_to == Some(user_id),
        ItemKind::Simple | ItemKind::Template => false,
    }
}

/// See `project_calendar::children_html_for`'s identical rationale — the cross-project
/// counterpart, otherwise unchanged.
async fn children_html_for(
    repo: &Arc<dyn ItemRepo>,
    item: &Item,
    project_id: &str,
    names: &HashMap<String, String>,
    tz: i32,
    is_team_project: bool,
) -> Result<Option<String>, ItemError> {
    if !item.has_children {
        return Ok(None);
    }
    Ok(Some(
        super::project_tasks::render_expandable_children(
            repo,
            &item.id,
            project_id,
            names,
            true,
            tz,
            &HashMap::new(),
            is_team_project,
            1,
        )
        .await?,
    ))
}

/// Cross-project counterpart to `project_calendar::calendar_row` — see that function's doc
/// comment for the full rationale (reuses `ProjectTaskRow`/`ProjectEventRow::from_item` rather
/// than a calendar-specific template, to bring the row-actions menu here with no drift risk).
/// The one addition here is `project_name`, since this screen (unlike the per-project calendar)
/// mixes rows from every project the requester belongs to.
#[allow(clippy::too_many_arguments)]
pub(crate) fn calendar_row(
    item: &Item,
    parent_name: Option<String>,
    project_id: &str,
    project_name: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    tz: i32,
    skip_url: Option<String>,
    confirmation: Option<String>,
    dismiss_after_ms: Option<u32>,
    children_html: Option<String>,
) -> Result<String, ItemError> {
    let mut row = match item.kind() {
        ItemKind::Event => {
            let mut row = ProjectEventRow::from_item(item, project_id, tz, skip_url);
            row.assignee_name = item
                .assigned_to_user_id()
                .and_then(|id| names.get(&id).cloned());
            row.confirmation = confirmation;
            row.dismiss_after_ms = dismiss_after_ms;
            row
        }
        _ => ProjectTaskRow::from_item(
            item,
            project_id,
            names,
            &[],
            tz,
            skip_url,
            is_team_project,
            false,
            confirmation,
            dismiss_after_ms,
        ),
    };
    row.type_badge = Some(type_symbol(item.kind()));
    row.parent_name = parent_name;
    row.project_name = Some(project_name.to_string());
    row.expanded_row = true;
    // See `project_calendar::calendar_row`'s identical `children_html_for`-built rationale (#3
    // of docs/issues_and_features.md's calendar-view entries).
    row.children_html = children_html;
    // See `project_calendar::calendar_row`'s identical rationale — previously forced `false`
    // (deferred out of scope for Stage 1 of docs/dialog-item-forms-plan.md), now opted in since
    // `reschedule_url`/`assign_url` below already prove nested `#action-dialog`-atop-`#day-drawer`
    // works.
    row.detail_via_dialog = true;
    row.complete_url = row
        .complete_url
        .as_ref()
        .map(|_| format!("/web/calendar/projects/{project_id}/items/{}", item.id));
    // See `project_calendar::calendar_row`'s identical rationale for this suffix.
    row.reschedule_url = row
        .reschedule_url
        .map(|url| format!("{url}?view=main-calendar"));
    row.assign_url = row
        .assign_url
        .map(|url| format!("{url}?view=main-calendar"));
    Ok(row.render()?)
}

#[derive(Template)]
#[template(path = "main_calendar/virtual_row.html")]
struct MainCalendarVirtualRow {
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
    /// See `project_calendar::ProjectCalendarVirtualRow::complete_url`'s identical rationale —
    /// same `is_current` gate (confirmed live: a non-current occurrence's checkbox otherwise
    /// 400s every time via `item_series::require_current_occurrence`), same route reuse.
    complete_url: Option<String>,
}

impl MainCalendarVirtualRow {
    fn from_occurrence(
        occ: &ProjectOccurrence,
        project_id: &str,
        project_name: &str,
        tz: i32,
    ) -> Self {
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
            date_label: format_display_date(local, true),
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
            complete_url: (occ.item_type == ItemKind::Task && occ.is_current)
                .then(|| occ.complete_url(project_id)),
        }
    }
}

/// Stage 8 of docs/calendar-day-drawer-plan.md: `.../dashboard`/`.../dashboard/calendar` were
/// this screen's base/legacy-calendar paths before the "Dashboard" → "Calendar" route rename —
/// kept alive only as redirects (cheap insurance against a stale link or bookmark), forwarding
/// whatever query string it was given so a bookmarked `?year=...&date=...` still lands on the
/// same day.
pub async fn redirect_main_dashboard(RawQuery(query): RawQuery) -> Redirect {
    match query {
        Some(q) if !q.is_empty() => Redirect::to(&format!("/web/calendar?{q}")),
        _ => Redirect::to("/web/calendar"),
    }
}

/// Stage 8: `.../dashboard/list` was this screen's list-view path before the rename — kept
/// alive as a redirect, same rationale as `redirect_main_dashboard`. Stage 1 of
/// docs/all-projects-landing-plan.md retargeted this from the now-removed cross-project
/// calendar list (`.../calendar/list`) to `/web/tasks`, the new landing page.
pub async fn redirect_main_dashboard_list(RawQuery(query): RawQuery) -> Redirect {
    match query {
        Some(q) if !q.is_empty() => Redirect::to(&format!("/web/tasks?{q}")),
        _ => Redirect::to("/web/tasks"),
    }
}

/// Redesign per docs/issues_and_features.md's calendar-view entry: a day cell only shows a
/// count hint now, not the items themselves — see `MainCalendarPageTemplate`'s doc comment
/// for where the full list moved to, mirroring `project_tasks::templates::CalendarDay`.
struct MainCalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    is_selected: bool,
    entry_count: usize,
}

#[derive(Template)]
#[template(path = "main_calendar/calendar_page.html")]
struct MainCalendarPageTemplate {
    year: i32,
    month: u32,
    month_label: String,
    month_iso: String,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<MainCalendarDay>,
    /// Stage 4 (docs/calendar-day-drawer-plan.md): whether the day-drawer fragment below
    /// actually has a day to show — see `project_calendar::ProjectCalendarPageTemplate::
    /// has_selected_date`'s identical rationale (gates the inline `showModal()` script for a
    /// hard/bookmarked `?date=...` load).
    has_selected_date: bool,
    /// The `#day-drawer` dialog's initial innerHTML — see `project_calendar::
    /// ProjectCalendarPageTemplate::day_drawer_html`'s identical rationale.
    day_drawer_html: String,
    /// The page-level assigned-to-me toggle — see `project_calendar`'s identical field.
    assigned_to_any: bool,
    /// Selected day's ISO date, `None` when no day is selected — same rationale as
    /// `project_calendar`'s identical field.
    selected_date_iso: Option<String>,
    active_type: &'static str,
    nav_html: String,
}

/// Stage 4's day-drawer header data — see `project_calendar::DayDrawerData`'s identical
/// rationale. No `project_id` field here (unlike that struct): this screen's URLs are all
/// `/web/calendar...`, not project-scoped, so the template needs no project id to build them.
struct DayDrawerData {
    date_iso: String,
    date_year: i32,
    date_month: u32,
    selected_date_label: String,
    prev_date: String,
    prev_year: i32,
    prev_month: u32,
    next_date: String,
    next_year: i32,
    next_month: u32,
    active_type: &'static str,
    assigned_to_any: bool,
}

/// See `project_calendar::ProjectCalendarDayPanelTemplate`'s identical rationale — also the
/// `#day-drawer` dialog's own innerHTML for the `.../calendar/day` fragment route, not just
/// the calendar page's initial embed.
#[derive(Template)]
#[template(path = "main_calendar/calendar_day_panel.html")]
struct MainCalendarDayPanelTemplate {
    drawer: Option<DayDrawerData>,
    day_rows: Vec<String>,
}

/// Builds the `#day-drawer` dialog's innerHTML, shared between the calendar page's initial
/// embed and the `.../calendar/day` fragment route — see `project_calendar::render_day_drawer`'s
/// identical rationale.
fn render_day_drawer(
    date: Option<NaiveDate>,
    day_rows: Vec<String>,
    type_filter: Option<ItemKind>,
    assigned_to_any: bool,
) -> Result<String, ItemError> {
    let drawer = date.map(|d| {
        let prev = d - Duration::days(1);
        let next = d + Duration::days(1);
        DayDrawerData {
            date_iso: d.format("%Y-%m-%d").to_string(),
            date_year: d.year(),
            date_month: d.month(),
            selected_date_label: format_display_naive_date(d),
            prev_date: prev.format("%Y-%m-%d").to_string(),
            prev_year: prev.year(),
            prev_month: prev.month(),
            next_date: next.format("%Y-%m-%d").to_string(),
            next_year: next.year(),
            next_month: next.month(),
            active_type: active_type_label(type_filter),
            assigned_to_any,
        }
    });
    Ok(MainCalendarDayPanelTemplate { drawer, day_rows }.render()?)
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
    let leading = first_of_month.weekday().num_days_from_sunday();
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

/// Cross-project counterpart to `project_calendar::build_calendar_days` — takes each due
/// item/occurrence pre-tagged with the project it came from (`is_included` filtering already
/// applied by the caller), since a single grid here can mix rows from every project the user
/// belongs to. Per docs/issues_and_features.md's calendar-view entry, a cell only needs a
/// tally now — the full list for a clicked day renders separately via `day_list_rows`.
fn build_calendar_days(
    year: i32,
    month: u32,
    due_items: &[(DueItem, String, String)],
    virtual_occurrences: &[(ProjectOccurrence, String, String)],
    tz: i32,
    today: NaiveDate,
    selected_date: Option<NaiveDate>,
) -> Vec<MainCalendarDay> {
    let grid_start = grid_start_for(year, month);

    let mut counts: HashMap<NaiveDate, usize> = HashMap::new();
    for (di, _, _) in due_items {
        if let Some(dt) = calendar_date(&di.item) {
            *counts.entry(to_local(dt, tz).date_naive()).or_default() += 1;
        }
    }
    for (occ, _, _) in virtual_occurrences {
        let local = to_local(occ.occurrence_date, tz);
        *counts.entry(local.date_naive()).or_default() += 1;
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + Duration::days(i);
        days.push(MainCalendarDay {
            date: date.format("%Y-%m-%d").to_string(),
            day_number: date.day(),
            is_current_month: date.month() == month && date.year() == year,
            is_today: date == today,
            is_selected: Some(date) == selected_date,
            entry_count: counts.remove(&date).unwrap_or(0),
        });
    }
    days
}

/// The calendar's per-day panel — see `project_tasks::day_list_rows`'s identical rationale.
/// `due_items`/`virtual_occurrences` are the same pre-tagged, `is_included`-filtered buckets
/// `build_calendar_days` takes, just narrowed to `date`. `type_filter` is Stage 4's All/Tasks/
/// Events drawer tab — unlike `project_calendar::day_list_rows`, there's no separate `mine`
/// closure here: `is_included`'s `assigned_to_any`-aware filtering already happened once in
/// `gather_calendar_data` (the caller), so `due_items`/`virtual_occurrences` arrive pre-filtered
/// for both the caller and this function.
async fn day_list_rows(
    repo: &Arc<dyn ItemRepo>,
    due_items: &[(DueItem, String, String)],
    virtual_occurrences: &[(ProjectOccurrence, String, String)],
    names_by_project: &HashMap<String, HashMap<String, String>>,
    date: NaiveDate,
    tz: i32,
    type_filter: Option<ItemKind>,
    series: &Arc<dyn ItemSeriesRepo>,
) -> Result<Vec<String>, ItemError> {
    let mut entries: Vec<(i64, String)> = Vec::new();
    for (di, project_id, project_name) in due_items {
        let item = &di.item;
        if type_filter.is_some_and(|k| item.kind() != k) {
            continue;
        }
        let Some(dt) = calendar_date(item) else {
            continue;
        };
        if to_local(dt, tz).date_naive() != date {
            continue;
        }
        let empty = HashMap::new();
        // `names_by_project` only ever gets an entry for a team-backed project (see
        // `gather_calendar_data`), so its presence doubles as the `is_team_project` flag this
        // needs — cheaper than threading a third field through the whole bucket just for this.
        let is_team_project = names_by_project.contains_key(project_id);
        let names = names_by_project.get(project_id).unwrap_or(&empty);
        let skip_url = series_service::skip_url_for_item(series, item, project_id).await?;
        let parent_name = (!di.parent_name.is_empty()).then(|| di.parent_name.clone());
        let children_html =
            children_html_for(repo, item, project_id, names, tz, is_team_project).await?;
        entries.push((
            dt.timestamp(),
            calendar_row(
                item,
                parent_name,
                project_id,
                project_name,
                names,
                is_team_project,
                tz,
                skip_url,
                None,
                None,
                children_html,
            )?,
        ));
    }
    for (occ, project_id, project_name) in virtual_occurrences {
        if type_filter.is_some_and(|k| occ.item_type != k) {
            continue;
        }
        if to_local(occ.occurrence_date, tz).date_naive() != date {
            continue;
        }
        entries.push((
            occ.occurrence_date.timestamp(),
            MainCalendarVirtualRow::from_occurrence(occ, project_id, project_name, tz).render()?,
        ));
    }
    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MainCalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
    /// See `project_tasks::handlers::CalendarQuery::date`'s identical rationale.
    date: Option<String>,
    /// Stage 4's drawer type tab (`all`/`task`/`event`) — parsed via `parse_type_filter`. Only
    /// meaningful alongside `date`; ignored when no day is selected.
    r#type: Option<String>,
    /// Stage 4's assigned-to-me toggle — `None`/absent = mine, present = everyone's.
    assigned_to_any: Option<String>,
}

/// Shared by the full calendar page and its `.../calendar/day` fragment route — both need the
/// same `is_included`-filtered, per-project-tagged buckets `build_calendar_days`/
/// `day_list_rows` take, plus each project's assignee-name map for `day_list_rows`.
#[allow(clippy::type_complexity)]
async fn gather_calendar_data(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    users: &Arc<dyn UserRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    tz: i32,
    assigned_to_any: bool,
) -> Result<
    (
        Vec<(DueItem, String, String)>,
        Vec<(ProjectOccurrence, String, String)>,
        HashMap<String, HashMap<String, String>>,
    ),
    ItemError,
> {
    let user_projects = project_service::list_projects(projects, requester_user_id).await?;

    let mut due_bucket: Vec<(DueItem, String, String)> = Vec::new();
    let mut occ_bucket: Vec<(ProjectOccurrence, String, String)> = Vec::new();
    let mut names_by_project: HashMap<String, HashMap<String, String>> = HashMap::new();

    for project in &user_projects {
        let is_team_project = project.team_id.is_some();
        if let Some(team_id) = &project.team_id {
            names_by_project.insert(
                project.id.clone(),
                names_for(teams, team_id, requester_user_id).await?,
            );
        }
        let due_items =
            project_item_service::list_due_project_items_unchecked(repo, &project.id, None, None)
                .await?;
        for di in due_items {
            if is_included(
                di.item.kind(),
                is_team_project,
                di.item.assigned_to_user_id().as_deref(),
                requester_user_id,
                assigned_to_any,
            ) {
                due_bucket.push((di, project.id.clone(), project.name.clone()));
            }
        }
        let occurrences = series_service::list_occurrence_states_for_project(
            series,
            users,
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
                requester_user_id,
                assigned_to_any,
            )
        });
        for occ in occurrences {
            occ_bucket.push((occ, project.id.clone(), project.name.clone()));
        }
    }
    Ok((due_bucket, occ_bucket, names_by_project))
}

pub async fn main_calendar_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<MainCalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let today = to_local(Utc::now(), tz).date_naive();
    let year = q.year.unwrap_or_else(|| today.year());
    let month = q
        .month
        .filter(|m| (1..=12).contains(m))
        .unwrap_or_else(|| today.month());
    let selected_date = q
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let assigned_to_any = q.assigned_to_any.is_some();
    let type_filter = parse_type_filter(q.r#type.as_deref());

    let grid_start = grid_start_for(year, month);
    let range_start = local_date_to_utc(grid_start, start_of_day(), tz);
    let range_end = local_date_to_utc(grid_start + Duration::days(41), end_of_day(), tz);

    let (due_bucket, occ_bucket, names_by_project) = gather_calendar_data(
        &repo,
        &projects,
        &teams,
        &users,
        &series,
        &auth_user.user_id,
        range_start,
        range_end,
        tz,
        assigned_to_any,
    )
    .await?;

    let days = build_calendar_days(
        year,
        month,
        &due_bucket,
        &occ_bucket,
        tz,
        today,
        selected_date,
    );
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        ActiveContext::AllProjects,
        SidebarSection::None,
    )
    .await?;
    let day_rows = match selected_date {
        Some(date) => {
            day_list_rows(
                &repo,
                &due_bucket,
                &occ_bucket,
                &names_by_project,
                date,
                tz,
                type_filter,
                &series,
            )
            .await?
        }
        None => Vec::new(),
    };
    let day_drawer_html = render_day_drawer(selected_date, day_rows, type_filter, assigned_to_any)?;

    render(MainCalendarPageTemplate {
        year,
        month,
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
        has_selected_date: selected_date.is_some(),
        day_drawer_html,
        assigned_to_any,
        selected_date_iso: selected_date.map(|d| d.format("%Y-%m-%d").to_string()),
        active_type: active_type_label(type_filter),
        nav_html,
    })
}

pub async fn main_calendar_day_fragment(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<MainCalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let date = q
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or(ItemError::Invalid("date is required".to_string()))?;
    let assigned_to_any = q.assigned_to_any.is_some();
    let type_filter = parse_type_filter(q.r#type.as_deref());
    let range_start = local_date_to_utc(date, start_of_day(), tz);
    let range_end = local_date_to_utc(date, end_of_day(), tz);
    let (due_bucket, occ_bucket, names_by_project) = gather_calendar_data(
        &repo,
        &projects,
        &teams,
        &users,
        &series,
        &auth_user.user_id,
        range_start,
        range_end,
        tz,
        assigned_to_any,
    )
    .await?;
    let day_rows = day_list_rows(
        &repo,
        &due_bucket,
        &occ_bucket,
        &names_by_project,
        date,
        tz,
        type_filter,
        &series,
    )
    .await?;
    Ok(Html(render_day_drawer(
        Some(date),
        day_rows,
        type_filter,
        assigned_to_any,
    )?))
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MainCalendarToggleForm {
    complete: Option<String>,
}

pub async fn toggle_main_calendar_item_complete(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<MainCalendarToggleForm>,
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
        &series,
        &reminders,
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
        Ok(updated) => {
            let skip_url =
                series_service::skip_url_for_item(&series, &updated, &project_id).await?;
            let children_html = children_html_for(
                &repo,
                &updated,
                &project_id,
                &names,
                tz,
                project.team_id.is_some(),
            )
            .await?;
            Ok(Html(calendar_row(
                &updated,
                None,
                &project_id,
                &project.name,
                &names,
                project.team_id.is_some(),
                tz,
                skip_url,
                None,
                None,
                children_html,
            )?))
        }
        // See `project_calendar::toggle_project_calendar_item_complete`'s identical
        // rationale for this branch.
        Err(ItemError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(e),
    }
}
