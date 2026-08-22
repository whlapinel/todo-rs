use super::nav::{self, ActiveContext, SidebarSection};
use super::project_calendar::detail_url;
use super::{TzOffset, to_local};
use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{self as series_service, OccurrenceState, ProjectOccurrence};
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::storage::sqlite::{
    ActivityLogRepo, DueItem, ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo, UserRepo,
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

/// Duplicated from `project_calendar.rs` rather than shared — that module's own equivalents
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

fn calendar_date(item: &Item) -> Option<DateTime<Utc>> {
    match item.kind() {
        ItemKind::Event => item.scheduled_date().or(item.due_date()),
        _ => item.due_date(),
    }
}

fn calendar_has_time(item: &Item) -> bool {
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
/// here at all (mirrors `project_calendar::render_rows`'s own `ItemKind::Simple` exclusion,
/// widened to also exclude Template).
///
/// `assigned_to_any` (added Stage 4 of docs/calendar-day-drawer-plan.md, for the calendar's own
/// assigned-to-me toggle) *relaxes* the team-backed-project Task restriction when set — it has
/// no effect on personal-project tasks, which were never restricted in the first place, and no
/// effect on Events, which were never restricted either. The flat list view
/// (`list_main_calendar_rows`) has no such toggle and always passes `false` here, preserving
/// its existing behavior exactly.
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

/// Same rationale as `project_calendar::virtual_occurrence_window` — bounds how far ahead an
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
#[template(path = "main_calendar/row.html")]
struct MainCalendarRow {
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
    /// See `project_calendar::ProjectCalendarRow`'s identical fields/rationale.
    confirmation: Option<String>,
    dismiss_after_ms: Option<u32>,
}

impl MainCalendarRow {
    #[allow(clippy::too_many_arguments)]
    fn from_due_item(
        di: &DueItem,
        project_id: &str,
        project_name: &str,
        names: &HashMap<String, String>,
        tz: i32,
        confirmation: Option<String>,
        dismiss_after_ms: Option<u32>,
    ) -> Self {
        let item = &di.item;
        let date_label = calendar_date(item).map(|d| {
            let local = to_local(d, tz);
            if calendar_has_time(item) {
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
                .then(|| format!("/web/calendar/projects/{project_id}/items/{}", item.id)),
            detail_link: detail_url(item, project_id),
            toggle_complete_json: (!item.complete).to_string(),
            confirmation,
            dismiss_after_ms,
        }
    }
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
    /// See `project_calendar::ProjectCalendarVirtualRow::complete_url`'s identical rationale
    /// — same `is_current` gate (confirmed live: a non-current occurrence's checkbox otherwise
    /// 400s every time via `item_series::require_current_occurrence`), same route reuse. Now
    /// rebuilds `#main-calendar-list` in place via `list_query`/`in_list_view` below when
    /// `complete_project_item_series_occurrence_form`'s `view=main-calendar` branch handles it
    /// — this page's own row assembly (`list_main_calendar_rows`) spans every project the
    /// requester belongs to, so that branch re-runs the full cross-project gather rather than a
    /// single project's `list_calendar_rows_for_project`.
    complete_url: Option<String>,
    /// See `project_calendar::ProjectCalendarVirtualRow::in_list_view`'s identical rationale.
    in_list_view: bool,
}

impl MainCalendarVirtualRow {
    /// `list_query` is `Some(&query_string)` (baked from `calendar_list_query`) only when
    /// rendering for the flat list — `None` for the calendar day panel.
    fn from_occurrence(
        occ: &ProjectOccurrence,
        project_id: &str,
        project_name: &str,
        tz: i32,
        list_query: Option<&str>,
    ) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        let kind_name = if occ.item_type == ItemKind::Event {
            "Event"
        } else {
            "Task"
        };
        let suffix = list_query.unwrap_or("");
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
            skip_url: format!("{}{suffix}", occ.skip_url(project_id)),
            is_current: occ.is_current,
            assignee_name: occ.assigned_to_user_name.clone(),
            is_skipped: occ.is_skipped(),
            unskip_url: format!("{}{suffix}", occ.unskip_url(project_id)),
            complete_url: (occ.item_type == ItemKind::Task && occ.is_current)
                .then(|| format!("{}{suffix}", occ.complete_url(project_id))),
            in_list_view: list_query.is_some(),
        }
    }
}

/// Bakes `preset`/`show_complete` into a query-string suffix for a virtual row's checkbox/
/// Skip/Unskip URLs — see `project_calendar::calendar_list_query`'s identical rationale, one
/// filter dimension narrower (this screen has no `assignedToAny` toggle of its own).
fn calendar_list_query(preset: &str, show_complete: bool) -> String {
    let mut params = vec![
        "view=main-calendar".to_string(),
        format!("preset={}", preset.replace(' ', "%20")),
    ];
    if show_complete {
        params.push("showComplete=1".to_string());
    }
    format!("?{}", params.join("&"))
}

#[derive(Template)]
#[template(path = "main_calendar/page.html")]
struct MainCalendarListPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    presets: Vec<(&'static str, bool)>,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MainCalendarListQuery {
    preset: Option<String>,
    show_complete: Option<String>,
}

/// Cross-project row assembly, shared between the initial page load (`main_calendar_list_page`)
/// and the in-place `#main-calendar-list` rebuild the checkbox/Skip/Unskip handlers do on
/// `view=main-calendar` — mirrors `project_calendar::list_calendar_rows_for_project`'s
/// identical rationale, just re-running the full cross-project gather every time (this
/// screen's own filtering already spans every project the requester belongs to, so there's no
/// narrower single-project query to reuse).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn list_main_calendar_rows(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    users: &Arc<dyn UserRepo>,
    teams: &Arc<dyn TeamRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    preset: &str,
    show_complete: bool,
    tz_offset: i32,
    just_completed_item_id: Option<&str>,
) -> Result<Vec<String>, ItemError> {
    let (after, before) = preset_range(preset, Utc::now(), tz_offset);
    let (virtual_after, virtual_before) = virtual_occurrence_window(after, before, Utc::now());
    let list_query = calendar_list_query(preset, show_complete);

    let user_projects = project_service::list_projects(projects, requester_user_id).await?;

    let mut entries: Vec<(i64, String)> = Vec::new();
    for project in &user_projects {
        let is_team_project = project.team_id.is_some();
        let names = match &project.team_id {
            Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
            None => HashMap::new(),
        };
        let due_items =
            project_item_service::list_due_project_items_unchecked(repo, &project.id, None, None)
                .await?;
        let virtual_occurrences = if virtual_after <= virtual_before {
            series_service::list_occurrence_states_for_project(
                series,
                users,
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
            let just_completed = Some(item.id.as_str()) == just_completed_item_id;
            if !is_included(
                item.kind(),
                is_team_project,
                item.assigned_to_user_id().as_deref(),
                requester_user_id,
                false,
            ) {
                continue;
            }
            if !show_complete && item.complete && !just_completed {
                continue;
            }
            if preset == "All with due date" && calendar_date(item).is_none() {
                continue;
            }
            let d = calendar_date(item);
            let in_window = match d {
                Some(d) => after.is_none_or(|a| d >= a) && before.is_none_or(|b| d <= b),
                None => after.is_none() && before.is_none(),
            };
            if !in_window {
                continue;
            }
            let ts = d.map(|d| d.timestamp()).unwrap_or(i64::MAX);
            let confirmation = just_completed.then(|| "Completed".to_string());
            let dismiss_after_ms = (just_completed && !show_complete).then_some(1800u32);
            let html = MainCalendarRow::from_due_item(
                di,
                &project.id,
                &project.name,
                &names,
                tz_offset,
                confirmation,
                dismiss_after_ms,
            )
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
                requester_user_id,
                false,
            ) {
                continue;
            }
            entries.push((
                occ.occurrence_date.timestamp(),
                MainCalendarVirtualRow::from_occurrence(
                    occ,
                    &project.id,
                    &project.name,
                    tz_offset,
                    Some(&list_query),
                )
                .render()?,
            ));
        }
    }

    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

/// The `#main-calendar-list` innerHTML swap body — see `project_tasks::items_list_inner_html`'s
/// identical rationale, matching `main_calendar/page.html`'s own empty-state markup.
pub(crate) fn main_calendar_items_inner_html(rows: &[String]) -> String {
    if rows.is_empty() {
        "<li class=\"py-3 text-sm text-gray-500 dark:text-gray-400\">Nothing to show.</li>"
            .to_string()
    } else {
        rows.concat()
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
/// alive as a redirect to `.../calendar/list`, same rationale as `redirect_main_dashboard`.
pub async fn redirect_main_dashboard_list(RawQuery(query): RawQuery) -> Redirect {
    match query {
        Some(q) if !q.is_empty() => Redirect::to(&format!("/web/calendar/list?{q}")),
        _ => Redirect::to("/web/calendar/list"),
    }
}

pub async fn main_calendar_list_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz_offset): TzOffset,
    Query(q): Query<MainCalendarListQuery>,
) -> Result<Html<String>, ItemError> {
    let preset = q.preset.unwrap_or_else(|| "Today".to_string());
    let show_complete = q.show_complete.is_some();

    let rows = list_main_calendar_rows(
        &repo,
        &projects,
        &users,
        &teams,
        &series,
        &auth_user.user_id,
        &preset,
        show_complete,
        tz_offset,
        None,
    )
    .await?;

    let presets = PRESETS.iter().map(|&p| (p, p == preset)).collect();
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        ActiveContext::None,
        SidebarSection::None,
    )
    .await?;
    render(MainCalendarListPageTemplate {
        rows,
        show_complete,
        presets,
        nav_html,
    })
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
            selected_date_label: day_panel_label(d),
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
fn day_list_rows(
    due_items: &[(DueItem, String, String)],
    virtual_occurrences: &[(ProjectOccurrence, String, String)],
    names_by_project: &HashMap<String, HashMap<String, String>>,
    date: NaiveDate,
    tz: i32,
    type_filter: Option<ItemKind>,
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
        let names = names_by_project.get(project_id).unwrap_or(&empty);
        entries.push((
            dt.timestamp(),
            MainCalendarRow::from_due_item(di, project_id, project_name, names, tz, None, None)
                .render()?,
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
            MainCalendarVirtualRow::from_occurrence(occ, project_id, project_name, tz, None)
                .render()?,
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
    /// Stage 4's assigned-to-me toggle — `None`/absent = mine, present = everyone's, matching
    /// `project_calendar::ProjectCalendarListQuery::assigned_to_any`'s convention.
    assigned_to_any: Option<String>,
}

/// See `project_tasks::handlers::day_panel_label`'s identical rationale.
fn day_panel_label(date: NaiveDate) -> String {
    date.format("%A, %B %d, %Y").to_string()
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
        ActiveContext::None,
        SidebarSection::None,
    )
    .await?;
    let day_rows = match selected_date {
        Some(date) => day_list_rows(
            &due_bucket,
            &occ_bucket,
            &names_by_project,
            date,
            tz,
            type_filter,
        )?,
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
        &due_bucket,
        &occ_bucket,
        &names_by_project,
        date,
        tz,
        type_filter,
    )?;
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
        Ok(updated) => render(MainCalendarRow::from_due_item(
            &DueItem {
                parent_name: String::new(),
                item: updated,
            },
            &project_id,
            &project.name,
            &names,
            tz,
            None,
            None,
        )),
        // See `project_calendar::toggle_project_calendar_item_complete`'s identical
        // rationale for this branch.
        Err(ItemError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(e),
    }
}
