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
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
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
pub(crate) fn preset_range(preset: &str, now: DateTime<Utc>, tz_offset_minutes: i32) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
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

/// The dashboard combines Tasks and Events into one due-date-sorted view, but the two types
/// don't agree on which field is their "primary" date (see CLAUDE.md's Scheduled start/end
/// section) — a Task's is `due_date`; an Event's is `scheduled_date`, falling back to
/// `due_date` only if unset, same fallback `events.rs`'s own `calendar_date` uses for its
/// calendar view. Everything else (Simple, Template) has no date concept here and returns
/// `None`, same as it always has.
fn dashboard_date(item: &Item) -> Option<DateTime<Utc>> {
    match item.kind() {
        ItemKind::Event => item.scheduled_date().or(item.due_date()),
        _ => item.due_date(),
    }
}

/// Companion to `dashboard_date` — whichever field supplied the date is also the field whose
/// `has_*_time` flag decides whether a time-of-day should be shown, mirroring
/// `events.rs`'s `calendar_has_time`.
fn dashboard_has_time(item: &Item) -> bool {
    match item.kind() {
        ItemKind::Event if item.scheduled_date().is_some() => item.has_scheduled_time(),
        ItemKind::Event => item.has_due_time(),
        _ => item.has_due_time(),
    }
}

/// One-letter badge distinguishing Task/Event rows in the combined list and calendar views —
/// the "short symbol indicating which is which" the feature request asked for. Simple/
/// Template never reach either view (see `dashboard_date` above), but the match stays
/// exhaustive rather than a wildcard so a future item kind can't silently fall through
/// unlabeled.
fn type_symbol(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Task => "T",
        ItemKind::Event => "E",
        ItemKind::Simple => "S",
        ItemKind::Template => "Tmpl",
    }
}

#[derive(Template)]
#[template(path = "dashboard/row.html")]
struct DashboardRow {
    item_id: String,
    name: String,
    complete: bool,
    date_label: Option<String>,
    date_kind_label: &'static str,
    overdue: bool,
    type_symbol: &'static str,
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
pub(crate) fn detail_url(item: &Item) -> String {
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

/// Dedicated-screen **list** URL for a given kind/team — `detail_url`'s counterpart for the
/// case a promote/subordinate reparent (see `tasks.rs`/`team_tasks.rs`/`simple_lists.rs`/
/// `team_simple_lists.rs`'s `promote_*_form`/`subordinate_*_form`) lands an item at top level,
/// where there's no single new-parent detail page to redirect to — just the type's own list.
pub(crate) fn list_url_for(kind: ItemKind, team_id: Option<&str>) -> String {
    match (team_id, kind) {
        (Some(team_id), ItemKind::Task) => format!("/web/team-tasks/{team_id}"),
        (Some(team_id), ItemKind::Event) => format!("/web/team-events/{team_id}"),
        (Some(team_id), ItemKind::Simple) => format!("/web/team-simple-lists/{team_id}"),
        (Some(team_id), ItemKind::Template) => format!("/web/team-templates/{team_id}"),
        (None, ItemKind::Task) => "/web/tasks".to_string(),
        (None, ItemKind::Event) => "/web/events".to_string(),
        (None, ItemKind::Simple) => "/web/simple-lists".to_string(),
        (None, ItemKind::Template) => "/web/templates".to_string(),
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
            from_badge: item.team_id.as_ref().map(|team_id| format!("from team {team_id}")),
            can_delete: !is_team_item,
            toggle_target,
            detail_link: detail_url(item),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

pub(crate) const PRESETS: [&str; 6] = ["All", "All with due date", "Today", "This Week", "Next 30 Days", "Overdue"];

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

/// Filters `items` down to `preset`'s date window and `show_complete`, then sorts and renders.
/// Unlike `list_due`'s own SQL-level `due_date` window (still used verbatim for the initial
/// fetch below — untouched, since CLI/MCP callers of the same repo method still want plain
/// due-date semantics), filtering here happens in Rust against `dashboard_date` so an Event
/// showing up by its `scheduled_date` isn't excluded just because it has no `due_date` at all.
/// `after`/`before` are the same `preset_range()` output the SQL-level query used to take
/// directly; recomputing the window comparison per-row here is what makes the combined-date
/// semantics possible.
fn render_rows(
    items: &[DueItem],
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
    items
        .iter()
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

    // Fetch the full personal+assigned candidate set unfiltered by due_date (both bounds
    // None), then apply the preset window in Rust via `render_rows` — see its doc comment for
    // why the SQL-level filter itself can't be reused for combined Task/Event semantics.
    let due_items = repo
        .list_due(&auth_user.user_id, None, None)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&due_items, &preset, show_complete, after, before, tz_offset)?;

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

struct DashboardCalendarEntry {
    detail_link: String,
    name: String,
    time_label: Option<String>,
    type_symbol: &'static str,
}

struct DashboardCalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    entries: Vec<DashboardCalendarEntry>,
}

#[derive(Template)]
#[template(path = "dashboard/calendar_page.html")]
struct DashboardCalendarPageTemplate {
    month_label: String,
    month_iso: String,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<DashboardCalendarDay>,
    nav_html: String,
}

fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 { (year - 1, 12) } else { (year, month - 1) }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 { (year + 1, 1) } else { (year, month + 1) }
}

/// Builds the 42-cell (6-week, Monday-start) grid for `year`/`month`, bucketing `due_items`
/// by local calendar day off `dashboard_date` — same fixed 6-row layout as `events.rs`'s/
/// `tasks.rs`'s own `build_calendar_days`. Unlike those, entries here carry a `type_symbol`
/// since this view mixes Task and Event rows together (the whole point of this view — see
/// the feature note this satisfies). Matches those two calendars' precedent of not filtering
/// by completion status — the calendar always shows everything for the month regardless of
/// the list view's "Show completed" toggle.
fn build_calendar_days(
    year: i32,
    month: u32,
    due_items: &[DueItem],
    tz: i32,
    today: NaiveDate,
) -> Vec<DashboardCalendarDay> {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    let grid_start = first_of_month - Duration::days(leading as i64);

    let mut by_date: std::collections::HashMap<NaiveDate, Vec<DashboardCalendarEntry>> =
        std::collections::HashMap::new();
    for di in due_items {
        let item = &di.item;
        if let Some(dt) = dashboard_date(item) {
            let local = to_local(dt, tz);
            let time_label = dashboard_has_time(item).then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(DashboardCalendarEntry {
                    detail_link: detail_url(item),
                    name: item.name.clone(),
                    time_label,
                    type_symbol: type_symbol(item.kind()),
                });
        }
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + Duration::days(i);
        let mut entries = by_date.remove(&date).unwrap_or_default();
        entries.sort_by(|a, b| a.time_label.cmp(&b.time_label));
        days.push(DashboardCalendarDay {
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

pub async fn dashboard_calendar_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let today = to_local(Utc::now(), tz).date_naive();
    let year = q.year.unwrap_or_else(|| today.year());
    let month = q
        .month
        .filter(|m| (1..=12).contains(m))
        .unwrap_or_else(|| today.month());

    let due_items = repo
        .list_due(&auth_user.user_id, None, None)
        .await
        .map_err(ItemError::from)?;
    let days = build_calendar_days(year, month, &due_items, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::None,
    )
    .await?;

    render(DashboardCalendarPageTemplate {
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
