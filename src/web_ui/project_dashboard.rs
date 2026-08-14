use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use super::nav::{self, ActiveContext, SidebarSection};
use super::{to_local, TzOffset};
use crate::service::item_series::{self as event_series_service, VirtualOccurrence};
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::service::error::ItemError;
use crate::storage::sqlite::{ActivityLogRepo, DueItem, ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::Html;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

/// Duplicated from the now-deleted legacy `dashboard.rs` rather than shared — that module's
/// own equivalent was `pub(crate)`-visible only to itself, and every other B5 sub-stage
/// duplicated its per-screen helpers rather than widening a legacy module's visibility just to
/// reuse a handful of small functions (see e.g. `project_tasks/mod.rs`'s identical rationale).
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

const PRESETS: [&str; 6] = ["All", "All with due date", "Today", "This Week", "Next 30 Days", "Overdue"];

/// Duplicated from `dashboard.rs` rather than shared (that module's own equivalents are
/// private, and every other B5 sub-stage has duplicated its per-screen helpers rather than
/// widening a legacy module's visibility just to reuse a handful of small functions — see
/// e.g. `project_tasks/mod.rs`'s identical rationale). Merges `dashboard.rs`'s combined
/// Task/Event date semantics with `team_dashboard.rs`'s assignee display — see CLAUDE.md's
/// Scheduled start/end section for why Events are `scheduled_date`-primary here.
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

/// Project-scoped counterpart to `dashboard::detail_url` — every item here already carries
/// the one `project_id` this whole page is scoped to, so there's no personal-vs-team branch
/// to dispatch on at all, unlike the legacy function it replaces. `pub(crate)` so
/// `project_item_series::handlers`' materialize route can reuse the same kind-to-URL mapping
/// rather than duplicating it.
pub(crate) fn detail_url(item: &Item, project_id: &str) -> String {
    match item.kind() {
        ItemKind::Task => format!("/web/projects/{project_id}/tasks/{}", item.id),
        ItemKind::Event => format!("/web/projects/{project_id}/events/{}", item.id),
        ItemKind::Simple => format!("/web/projects/{project_id}/simple-lists/{}", item.id),
        ItemKind::Template => format!("/web/projects/{project_id}/templates/{}", item.id),
    }
}

/// Shared by the list and calendar views below — the POST target that materializes a virtual
/// occurrence and redirects to its (now real) detail page. See
/// `web_ui::project_item_series::handlers::materialize_project_item_series_occurrence_form`.
fn materialize_url(project_id: &str, series_id: &str, occurrence_date: DateTime<Utc>) -> String {
    format!("/web/projects/{project_id}/series/{series_id}/occurrences/{}", occurrence_date.timestamp())
}

/// Stage 6 of docs/recurring-events-virtual-occurrences-rough-plan.md — the "Skip" button's
/// POST target, a sibling route to `materialize_url` above. See
/// `web_ui::project_item_series::handlers::skip_project_item_series_occurrence_form`.
fn skip_url(project_id: &str, series_id: &str, occurrence_date: DateTime<Utc>) -> String {
    format!(
        "/web/projects/{project_id}/series/{series_id}/occurrences/{}/skip",
        occurrence_date.timestamp()
    )
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
#[template(path = "project_dashboard/row.html")]
struct ProjectDashboardRow {
    item_id: String,
    name: String,
    complete: bool,
    date_label: Option<String>,
    date_kind_label: &'static str,
    overdue: bool,
    type_symbol: &'static str,
    parent_name: Option<String>,
    assignee_name: Option<String>,
    /// Mirrors `dashboard::DashboardRow`'s own `can_delete` — true only on a personal
    /// project (matching the legacy personal dashboard, whose items were always the
    /// caller's own); `team_dashboard/row.html` never had a delete affordance at all, so a
    /// team-backed project's rows don't get one here either.
    can_delete: bool,
    toggle_target: String,
    detail_link: String,
    toggle_complete_json: String,
}

impl ProjectDashboardRow {
    fn from_due_item(
        di: &DueItem,
        project_id: &str,
        names: &HashMap<String, String>,
        is_team_project: bool,
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
        let date_kind_label = if item.kind() == ItemKind::Event && item.scheduled_date().is_some()
        {
            "Scheduled"
        } else {
            "Due"
        };
        Self {
            item_id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            date_label,
            date_kind_label,
            overdue: item.is_overdue(Utc::now()),
            type_symbol: type_symbol(item.kind()),
            parent_name: if di.parent_name.is_empty() { None } else { Some(di.parent_name.clone()) },
            assignee_name: item.assigned_to_user_id().and_then(|id| names.get(&id).cloned()),
            can_delete: !is_team_project,
            toggle_target: format!("/web/projects/{project_id}/dashboard/items/{}", item.id),
            detail_link: detail_url(item, project_id),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "project_dashboard/virtual_row.html")]
struct ProjectDashboardVirtualRow {
    series_id: String,
    occurrence_ts: i64,
    name: String,
    date_label: String,
    date_kind_label: &'static str,
    type_symbol: &'static str,
    title: String,
    materialize_url: String,
    skip_url: String,
}

impl ProjectDashboardVirtualRow {
    fn from_occurrence(occ: &VirtualOccurrence, project_id: &str, tz: i32) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        let kind_name = if occ.item_type == ItemKind::Event { "Event" } else { "Task" };
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            name: occ.series_name.clone(),
            date_label: local.format("%Y-%m-%d %H:%M").to_string(),
            date_kind_label: if occ.item_type == ItemKind::Event { "Scheduled" } else { "Due" },
            type_symbol: type_symbol(occ.item_type),
            title: format!("{kind_name} (not yet created)"),
            materialize_url: materialize_url(project_id, &occ.series_id, occ.occurrence_date),
            skip_url: skip_url(project_id, &occ.series_id, occ.occurrence_date),
        }
    }
}

#[derive(Template)]
#[template(path = "project_dashboard/page.html")]
struct ProjectDashboardPageTemplate {
    project_id: String,
    rows: Vec<String>,
    show_complete: bool,
    presets: Vec<(&'static str, bool)>,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DashboardQuery {
    preset: Option<String>,
    show_complete: Option<String>,
}

/// Same Rust-side filtering rationale as `dashboard::render_rows` — an unfiltered SQL fetch
/// (both `after`/`before` `None`) followed by filtering against `dashboard_date` here, so an
/// Event showing up by `scheduled_date` isn't excluded for lacking a `due_date`.
#[allow(clippy::too_many_arguments)]
fn render_rows(
    items: &[DueItem],
    virtual_occurrences: &[VirtualOccurrence],
    project_id: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    preset: &str,
    show_complete: bool,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    let mut items: Vec<&DueItem> = items
        .iter()
        .filter(|di| show_complete || !di.item.complete)
        .filter(|di| preset != "All with due date" || dashboard_date(&di.item).is_some())
        .filter(|di| match dashboard_date(&di.item) {
            Some(d) => after.is_none_or(|a| d >= a) && before.is_none_or(|b| d <= b),
            None => after.is_none() && before.is_none(),
        })
        .collect();
    items.sort_by_key(|di| dashboard_date(&di.item).map(|d| d.timestamp()).unwrap_or(i64::MAX));

    let mut entries: Vec<(i64, String)> = items
        .iter()
        .map(|di| {
            let ts = dashboard_date(&di.item).map(|d| d.timestamp()).unwrap_or(i64::MAX);
            ProjectDashboardRow::from_due_item(di, project_id, names, is_team_project, tz)
                .render()
                .map(|html| (ts, html))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Virtual occurrences have no `complete`/due-date-presence concept, so
    // `show_complete`/"All with due date" are no-ops for them by construction — every entry
    // here is always dated and always incomplete in effect.
    //
    // Task-typed occurrences are additionally clamped to `occurrence_date >= now` (Stage 8) —
    // unlike an Event, a past-dated unmaterialized Task represents a real missed obligation,
    // and deciding what that means (backlog, catch-up, ...) is deferred to Stage 9's own
    // cursor-based design (see docs/recurring-events-virtual-occurrences-rough-plan.md). Until
    // then this view simply doesn't surface one rather than half-answering the question.
    let now = Utc::now();
    for occ in virtual_occurrences {
        if occ.item_type == ItemKind::Task && occ.occurrence_date < now {
            continue;
        }
        entries.push((
            occ.occurrence_date.timestamp(),
            ProjectDashboardVirtualRow::from_occurrence(occ, project_id, tz).render()?,
        ));
    }

    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

/// Sane default forward window for virtual-occurrence generation when a preset leaves
/// `(after, before)` open on one or both sides ("All", "All with due date", "Overdue" — see
/// `preset_range`). Real items are unaffected either way; this only bounds how far ahead an
/// indefinitely-repeating series (e.g. "every day") gets expanded for *display*. Chosen as 3x
/// the existing "Next 30 Days" preset — generous enough to be useful as a default, small
/// enough to cap a daily series at well under 100 rows.
const VIRTUAL_OCCURRENCE_DEFAULT_WINDOW_DAYS: i64 = 90;

/// "Overdue" (`None, Some(now)`) collapses `virtual_after`/`virtual_before` to a degenerate
/// `[now, now]` window deliberately: a virtual occurrence has never been "missed" in any
/// actionable sense (there's nothing to catch up on until someone materializes or skips it),
/// so it's excluded from "Overdue" rather than accumulating indefinitely into the past.
fn virtual_occurrence_window(after: Option<DateTime<Utc>>, before: Option<DateTime<Utc>>, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        after.unwrap_or(now),
        before.unwrap_or(now + Duration::days(VIRTUAL_OCCURRENCE_DEFAULT_WINDOW_DAYS)),
    )
}

pub async fn project_dashboard_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz_offset): TzOffset,
    Query(q): Query<DashboardQuery>,
) -> Result<Html<String>, ItemError> {
    let project = project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let preset = q.preset.unwrap_or_else(|| "Today".to_string());
    let show_complete = q.show_complete.is_some();
    let (after, before) = preset_range(&preset, Utc::now(), tz_offset);

    let due_items =
        project_item_service::list_due_project_items_unchecked(&repo, &project_id, None, None)
            .await?;
    let (virtual_after, virtual_before) = virtual_occurrence_window(after, before, Utc::now());
    let virtual_occurrences = if virtual_after <= virtual_before {
        event_series_service::list_virtual_occurrences_for_project_unchecked(
            &event_series,
            &project_id,
            virtual_after,
            virtual_before,
            tz_offset,
        )
        .await?
    } else {
        Vec::new()
    };
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let rows = render_rows(
        &due_items,
        &virtual_occurrences,
        &project_id,
        &names,
        project.team_id.is_some(),
        &preset,
        show_complete,
        after,
        before,
        tz_offset,
    )?;

    let presets = PRESETS.iter().map(|&p| (p, p == preset)).collect();
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::None,
    )
    .await?;
    render(ProjectDashboardPageTemplate {
        project_id,
        rows,
        show_complete,
        presets,
        nav_html,
    })
}

struct ProjectDashboardCalendarEntry {
    /// Unique across the whole grid — a real item's own id for a materialized entry, a
    /// `series_id`+`occurrence_ts` pair for a virtual one. Only actually needed as an
    /// `hx-target` for the Skip button below, but set unconditionally for both kinds so the
    /// template doesn't need an `Option` just to render an `id` attribute.
    entry_id: String,
    detail_link: String,
    name: String,
    time_label: Option<String>,
    type_symbol: &'static str,
    /// `Some(...)` only for a virtual (unmaterialized) occurrence — the template POSTs here
    /// instead of following `detail_link` (which is `"#"` in that case).
    materialize_url: Option<String>,
    /// `Some(...)` only for a virtual occurrence too (Stage 6) — a materialized occurrence's
    /// only "skip" affordance in this stage is deleting its item via the item's own detail
    /// page, per docs/recurring-events-virtual-occurrences-rough-plan.md's stage 6 write-up.
    skip_url: Option<String>,
    is_virtual: bool,
}

struct ProjectDashboardCalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    entries: Vec<ProjectDashboardCalendarEntry>,
}

#[derive(Template)]
#[template(path = "project_dashboard/calendar_page.html")]
struct ProjectDashboardCalendarPageTemplate {
    project_id: String,
    month_label: String,
    month_iso: String,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<ProjectDashboardCalendarDay>,
    nav_html: String,
}

fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 { (year - 1, 12) } else { (year, month - 1) }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 { (year + 1, 1) } else { (year, month + 1) }
}

/// The first (Monday-start) cell of the 6-row grid for `year`/`month` — hoisted out of
/// `build_calendar_days` so the handler can compute the same grid's UTC date range before
/// calling it (to bound the virtual-occurrence lookup).
fn grid_start_for(year: i32, month: u32) -> NaiveDate {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    first_of_month - Duration::days(leading as i64)
}

/// Converts a local calendar date + time-of-day into the UTC instant it represents, given
/// `tz_offset_minutes` — same `local + offset = utc` convention as
/// `project_events::combine_local_to_utc`.
fn local_date_to_utc(date: NaiveDate, time: chrono::NaiveTime, tz_offset_minutes: i32) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(date.and_time(time), Utc) + Duration::minutes(tz_offset_minutes as i64)
}

fn start_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}

fn end_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap()
}

/// Mirrors `dashboard::build_calendar_days` exactly, project-scoped — same fixed 6-row
/// Monday-start grid, same no-completion-filter precedent. A calendar view is a genuinely
/// new capability for team-backed projects here (`team_dashboard.rs` never had one), same
/// framing as every prior B5 sub-stage's own calendar view. `virtual_occurrences` (Stage 5)
/// are bucketed into the same per-day map as real due items — see CLAUDE.md's Events section
/// for why a materialized occurrence never appears twice here: once materialized it's a real
/// `items` row already covered by `due_items`, so `virtual_occurrences` only ever contains
/// occurrences with no `event_occurrences` row at all (see
/// `event_series::list_virtual_occurrences_for_project_unchecked`).
fn build_calendar_days(
    year: i32,
    month: u32,
    project_id: &str,
    due_items: &[DueItem],
    virtual_occurrences: &[VirtualOccurrence],
    tz: i32,
    today: NaiveDate,
) -> Vec<ProjectDashboardCalendarDay> {
    let grid_start = grid_start_for(year, month);

    let mut by_date: std::collections::HashMap<NaiveDate, Vec<ProjectDashboardCalendarEntry>> =
        std::collections::HashMap::new();
    for di in due_items {
        let item = &di.item;
        if let Some(dt) = dashboard_date(item) {
            let local = to_local(dt, tz);
            let time_label = dashboard_has_time(item).then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(ProjectDashboardCalendarEntry {
                    entry_id: format!("cal-item-{}", item.id),
                    detail_link: detail_url(item, project_id),
                    name: item.name.clone(),
                    time_label,
                    type_symbol: type_symbol(item.kind()),
                    materialize_url: None,
                    skip_url: None,
                    is_virtual: false,
                });
        }
    }
    // Task-typed occurrences are clamped to `occurrence_date >= now` — see the matching
    // comment in `render_rows` above.
    let now = Utc::now();
    for occ in virtual_occurrences {
        if occ.item_type == ItemKind::Task && occ.occurrence_date < now {
            continue;
        }
        let local = to_local(occ.occurrence_date, tz);
        by_date
            .entry(local.date_naive())
            .or_default()
            .push(ProjectDashboardCalendarEntry {
                entry_id: format!("cal-virtual-{}-{}", occ.series_id, occ.occurrence_date.timestamp()),
                detail_link: "#".to_string(),
                name: occ.series_name.clone(),
                time_label: Some(local.format("%H:%M").to_string()),
                type_symbol: type_symbol(occ.item_type),
                materialize_url: Some(materialize_url(project_id, &occ.series_id, occ.occurrence_date)),
                skip_url: Some(skip_url(project_id, &occ.series_id, occ.occurrence_date)),
                is_virtual: true,
            });
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + Duration::days(i);
        let mut entries = by_date.remove(&date).unwrap_or_default();
        entries.sort_by(|a, b| a.time_label.cmp(&b.time_label));
        days.push(ProjectDashboardCalendarDay {
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

pub async fn project_dashboard_calendar_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
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

    let due_items = project_item_service::list_due_project_items(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        None,
        None,
    )
    .await?;
    let grid_start = grid_start_for(year, month);
    let range_start = local_date_to_utc(grid_start, start_of_day(), tz);
    let range_end = local_date_to_utc(grid_start + Duration::days(41), end_of_day(), tz);
    let virtual_occurrences = event_series_service::list_virtual_occurrences_for_project_unchecked(
        &event_series,
        &project_id,
        range_start,
        range_end,
        tz,
    )
    .await?;
    let days = build_calendar_days(year, month, &project_id, &due_items, &virtual_occurrences, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::None,
    )
    .await?;

    render(ProjectDashboardCalendarPageTemplate {
        project_id,
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

pub async fn toggle_project_dashboard_item_complete(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
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
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
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
        Ok(updated) => render(ProjectDashboardRow::from_due_item(
            &DueItem { parent_name: String::new(), item: updated },
            &project_id,
            &names,
            project.team_id.is_some(),
            tz,
        )),
        // Recurring item just completed and got replaced under a new id (see
        // service::items::update_item/team_items::update_team_item) — nothing to render
        // back for the old id. See CLAUDE.md/B5b's implementation notes: current
        // service-layer behavior actually keeps the original row and this branch may be
        // dead in practice, but every other screen in this codebase still carries it
        // verbatim, so this one does too rather than diverging unilaterally.
        Err(ItemError::NotFound) => Ok(Html(String::new())),
        Err(e) => Err(e),
    }
}
