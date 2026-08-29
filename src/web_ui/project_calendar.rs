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
    ActivityLogRepo, DueItem, ItemDependencyRepo, ItemRepo, ItemSeriesRepo, ProjectRepo,
    ReminderRepo, TeamRepo, UserRepo,
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

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

/// Duplicated from `dashboard.rs` rather than shared (that module's own equivalents are
/// private, and every other B5 sub-stage has duplicated its per-screen helpers rather than
/// widening a legacy module's visibility just to reuse a handful of small functions — see
/// e.g. `project_tasks/mod.rs`'s identical rationale). Merges `dashboard.rs`'s combined
/// Task/Event date semantics with `team_dashboard.rs`'s assignee display — see CLAUDE.md's
/// Scheduled start/end section for why Events are `scheduled_date`-primary here.
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

/// Stage 3's All/Tasks/Events drawer tab filter — `None` (unrecognized or absent, including
/// the explicit "all" value) means no filter. Kept separate from `active_type_label` below so
/// the filter predicate and the canonical display string can never drift out of sync with each
/// other (an unrecognized `?type=` value normalizes to the same "all" both functions agree on).
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

/// Eagerly renders `item`'s full descendant subtree for `Row::children_html`'s in-place expand
/// feature (issue #3 of docs/issues_and_features.md's calendar-view entries) — `None` for a
/// leaf item or an Event (Events can't have structural children), otherwise reuses
/// `project_tasks::render_expandable_children` unchanged rather than a calendar-specific copy,
/// same "no separate template to drift out of sync" rationale as `calendar_row` itself. An empty
/// `skip_urls` map is passed (unlike the flat Tasks list's own batch-built map) — a nested
/// child's own Skip action, if it's a materialized series occurrence, is a minor, acceptable gap
/// here since expansion is a secondary affordance, not this screen's primary listing.
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
            None,
        )
        .await?,
    ))
}

/// Builds a materialized calendar row's `components::row::Row` — reuses `ProjectTaskRow`/
/// `ProjectEventRow::from_item` exactly (rather than a calendar-specific template of its own)
/// so the day-drawer/flat calendar list get the same row-actions menu (Edit/Reschedule/Assign/
/// Duplicate/Delete), Skip, and confirm-then-fade behavior the Tasks/Events screens already
/// have, with no separate template to drift out of sync — see docs/issues_and_features.md's
/// "Row actions should be available on the calendar's day-drawer as well." `type_badge`/
/// `parent_name` are then overridden since a calendar row (unlike a single-kind Tasks/Events
/// list) interleaves both kinds by date and flattens across the whole parent/child hierarchy —
/// see `Row`'s own doc comments for why those two fields exist at all. `expanded_row` is forced
/// `true`: the date/parent/assignee context this second line carries matters far more in a
/// chronological, cross-item view than it does on a single-kind list.
///
/// This also fixes a pre-existing inconsistency: the old calendar-specific row only ever showed
/// Delete on a personal project (`can_delete: !is_team_project`), unlike the Tasks/Events
/// screens' shared `Row`, which allows Delete on a team-backed project too (gated only on
/// `is_imported`) — there was never a deliberate reason for the calendar to be stricter.
#[allow(clippy::too_many_arguments)]
pub(crate) fn calendar_row(
    item: &Item,
    parent_name: Option<String>,
    project_id: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    tz: i32,
    skip_url: Option<String>,
    show_complete: bool,
    confirmation: Option<String>,
    dismiss_after_ms: Option<u32>,
    children_html: Option<String>,
) -> Result<String, ItemError> {
    let mut row = match item.kind() {
        ItemKind::Event => {
            let mut row = ProjectEventRow::from_item(item, project_id, tz, skip_url);
            // `ProjectEventRow::from_item` always leaves `assignee_name: None` — an Event has
            // no "Assign" action on its own screen — but the calendar has always displayed an
            // Event's assignee (if any) alongside its date, so this is preserved here rather
            // than silently dropped by the switch to the shared builder.
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
            show_complete,
            confirmation,
            dismiss_after_ms,
        ),
    };
    row.type_badge = Some(type_symbol(item.kind()));
    row.parent_name = parent_name;
    row.expanded_row = true;
    // #3 of docs/issues_and_features.md's calendar-view entries — same in-place expansion
    // `project_tasks`'s flat list already has (see `Row::children_html`'s doc comment); an
    // Event is never eligible (Events can't have structural children) so this is only ever
    // `Some` when `item` is a Task with `has_children`, per `day_list_rows`'s own gate.
    row.children_html = children_html;
    // Previously forced `false` (deferred out of scope for Stage 1 of
    // docs/dialog-item-forms-plan.md) — the calendar now opts into the same dialog behavior
    // every other screen already defaults to via `ProjectTaskRow::from_item`. Nested atop the
    // day-drawer's own modal `<dialog>` this way is already a proven pattern: this same row's
    // `reschedule_url`/`assign_url` below have always opened `#action-dialog` from inside the
    // day-drawer unconditionally, so a second modal `<dialog>` stacking on top of `#day-drawer`
    // is known to work correctly (see docs/issues_and_features.md's calendar-dialog entries).
    row.detail_via_dialog = true;
    // Re-point the checkbox at this screen's own toggle route (rather than the item's own
    // resource PUT route `ProjectTaskRow::from_item` sets by default) so a subsequent toggle
    // keeps re-rendering with this function's calendar-flavored row instead of reverting to the
    // plain Tasks-list shape. `None` (Events) is left untouched.
    row.complete_url = row
        .complete_url
        .as_ref()
        .map(|_| format!("/web/projects/{project_id}/calendar/items/{}", item.id));
    // Reschedule/Assign still save through the item's own generic resource route (no
    // calendar-scoped route for those exists) — this query-string suffix is what lets
    // `update_project_task_form`/`update_project_event_form` know to re-render the saved row
    // via this same `calendar_row` overlay instead of the plain `ProjectTaskRow`/`ProjectEventRow`
    // shape. See `project_tasks::RowViewQuery`'s doc comment.
    row.reschedule_url = row
        .reschedule_url
        .map(|url| format!("{url}?view=project-calendar"));
    row.assign_url = row
        .assign_url
        .map(|url| format!("{url}?view=project-calendar"));
    Ok(row.render()?)
}

#[derive(Template)]
#[template(path = "project_calendar/virtual_row.html")]
struct ProjectCalendarVirtualRow {
    series_id: String,
    occurrence_ts: i64,
    name: String,
    date_label: String,
    date_kind_label: &'static str,
    type_symbol: &'static str,
    title: String,
    materialize_url: String,
    skip_url: String,
    /// Stage 9: whether this is the series' `current_occurrence_date` — see
    /// `service::item_series::current_occurrence_date`'s doc comment.
    is_current: bool,
    assignee_name: Option<String>,
    /// See `project_tasks::templates::ProjectTaskVirtualRow::is_skipped`'s identical
    /// rationale.
    is_skipped: bool,
    unskip_url: String,
    /// `Some` only for a Task-typed occurrence (`Item::validate` rejects `complete: true` for
    /// Events, mirroring `components::row::Row`'s identical `None`-`complete_url`-for-Event
    /// convention `calendar_row` relies on above) that's also `is_current`, giving the
    /// calendar's virtual row the same completability the Tasks list's `ProjectTaskVirtualRow`
    /// already has for the one occurrence per series that's actually completable. Without the
    /// `is_current` gate a Planned row's checkbox would reliably 400
    /// (`item_series::require_current_occurrence`'s "cannot settle this occurrence out of
    /// order" — confirmed live) instead of doing anything, so the gate keeps this from being a
    /// checkbox that predictably errors on every click but one. Reuses the exact same route
    /// `ProjectTaskVirtualRow::complete_url` points at
    /// (`complete_project_item_series_occurrence_form`).
    complete_url: Option<String>,
}

impl ProjectCalendarVirtualRow {
    fn from_occurrence(occ: &ProjectOccurrence, project_id: &str, tz: i32) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        let kind_name = if occ.item_type == ItemKind::Event {
            "Event"
        } else {
            "Task"
        };
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
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

/// The calendar's per-day panel — see `project_tasks::day_list_rows`'s identical rationale.
/// Unlike `render_rows` above (the flat calendar page's preset-filtered view), this shows
/// every item/occurrence on `date`, optionally narrowed by the drawer's own All/Tasks/Events
/// tab (`type_filter`) and assigned-to-me toggle (`assigned_to_any`) — reusing `calendar_row`/
/// `ProjectCalendarVirtualRow` (no feature upgrade to virtual-row completability; that's
/// tracked separately in docs/issues_and_features.md — this only brings materialized rows'
/// action menu here, not a new capability for still-virtual occurrences).
///
/// The assigned-to-me filter is gated on `is_team_project`: a personal-project item never
/// carries a `TeamAssignment` at all (see CLAUDE.md's Points section — `assigned_to_user_id`/
/// `points` are "only meaningful on a team-backed project"), so applying that filter ungated
/// would silently empty every personal project's calendar under the new "mine" default. It's
/// also exempted for `ItemKind::Event` entirely (mirrors `main_calendar::is_included`'s
/// `Event => true` arm — see CLAUDE.md's Cross-project scoping rule doc comment): an Event or
/// Event-series occurrence has no meaningful "not mine" state, and most carry no assignee at
/// all, so gating them the same as Tasks silently hid every Event series occurrence on a
/// team-backed project (2026-08-24 bug fix).
#[allow(clippy::too_many_arguments)]
async fn day_list_rows(
    repo: &Arc<dyn ItemRepo>,
    due_items: &[DueItem],
    virtual_occurrences: &[ProjectOccurrence],
    project_id: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    date: NaiveDate,
    tz: i32,
    user_id: &str,
    assigned_to_any: bool,
    type_filter: Option<ItemKind>,
    series: &Arc<dyn ItemSeriesRepo>,
) -> Result<Vec<String>, ItemError> {
    let mine = |assigned: Option<String>| {
        !is_team_project || assigned_to_any || assigned == Some(user_id.to_string())
    };
    let day_items: Vec<&DueItem> = due_items
        .iter()
        .filter(|di| di.item.kind() != ItemKind::Simple)
        .filter(|di| type_filter.is_none_or(|k| di.item.kind() == k))
        // Events are never assignment-gated — see build_calendar_days' identical carve-out.
        .filter(|di| di.item.kind() == ItemKind::Event || mine(di.item.assigned_to_user_id()))
        .filter(|di| calendar_date(&di.item).map(|d| to_local(d, tz).date_naive()) == Some(date))
        .collect();
    let mut entries: Vec<(i64, String)> = Vec::with_capacity(day_items.len());
    for di in day_items {
        let ts = calendar_date(&di.item)
            .map(|d| d.timestamp())
            .unwrap_or(i64::MAX);
        let skip_url = series_service::skip_url_for_item(series, &di.item, project_id).await?;
        let parent_name = (!di.parent_name.is_empty()).then(|| di.parent_name.clone());
        let children_html =
            children_html_for(repo, &di.item, project_id, names, tz, is_team_project).await?;
        let html = calendar_row(
            &di.item,
            parent_name,
            project_id,
            names,
            is_team_project,
            tz,
            skip_url,
            true,
            None,
            None,
            children_html,
        )?;
        entries.push((ts, html));
    }
    for occ in virtual_occurrences
        .iter()
        .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
        .filter(|occ| type_filter.is_none_or(|k| occ.item_type == k))
        // Events are never assignment-gated — see build_calendar_days' identical carve-out.
        .filter(|occ| occ.item_type == ItemKind::Event || mine(occ.assigned_to_user_id.clone()))
        .filter(|occ| to_local(occ.occurrence_date, tz).date_naive() == date)
    {
        entries.push((
            occ.occurrence_date.timestamp(),
            ProjectCalendarVirtualRow::from_occurrence(occ, project_id, tz).render()?,
        ));
    }
    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

/// Stage 8 of docs/calendar-day-drawer-plan.md: `.../dashboard`/`.../dashboard/calendar` were
/// this screen's base/legacy-calendar paths before the "Dashboard" → "Calendar" route rename —
/// kept alive only as redirects (cheap insurance against a stale link or bookmark), forwarding
/// whatever query string it was given so a bookmarked `?year=...&date=...` still lands on the
/// same day.
pub async fn redirect_project_dashboard(
    Path(project_id): Path<String>,
    RawQuery(query): RawQuery,
) -> Redirect {
    let base = format!("/web/projects/{project_id}/calendar");
    match query {
        Some(q) if !q.is_empty() => Redirect::to(&format!("{base}?{q}")),
        _ => Redirect::to(&base),
    }
}

/// Stage 8: `.../dashboard/list` was this screen's list-view path before the rename — kept
/// alive as a redirect, same rationale as `redirect_project_dashboard`. Stage 1 of
/// docs/all-projects-landing-plan.md retargeted this from the now-removed per-project
/// calendar list (`.../calendar/list`) to that project's Tasks screen.
pub async fn redirect_project_dashboard_list(
    Path(project_id): Path<String>,
    RawQuery(query): RawQuery,
) -> Redirect {
    let base = format!("/web/projects/{project_id}/tasks");
    match query {
        Some(q) if !q.is_empty() => Redirect::to(&format!("{base}?{q}")),
        _ => Redirect::to(&base),
    }
}

/// Redesign per docs/issues_and_features.md's calendar-view entry: a day cell only shows a
/// count hint now, not the items themselves — see `ProjectCalendarPageTemplate`'s doc comment
/// for where the full list moved to, mirroring `project_tasks::templates::CalendarDay`.
struct ProjectCalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    is_selected: bool,
    entry_count: usize,
}

#[derive(Template)]
#[template(path = "project_calendar/calendar_page.html")]
struct ProjectCalendarPageTemplate {
    project_id: String,
    month_label: String,
    month_iso: String,
    year: i32,
    month: u32,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<ProjectCalendarDay>,
    /// Stage 2: whether the day-drawer fragment below actually has a day to show — gates the
    /// inline `showModal()` script (a hard/bookmarked load of `?date=...` needs to open the
    /// drawer without any htmx swap ever firing `htmx:afterSwap`, so nothing else would open it).
    has_selected_date: bool,
    /// The `#day-drawer` dialog's initial innerHTML — rendered via `ProjectCalendarDayPanelTemplate`
    /// the same way `day_rows` renders individual rows, so the page template doesn't need to
    /// duplicate the drawer's own fields (header, arrows, list) itself.
    day_drawer_html: String,
    /// Stage 3: the page-level assigned-to-me toggle — reloads this whole page (see
    /// `build_calendar_days`'s doc comment for why the tally needs this too, unlike the
    /// drawer's own per-tab type filter).
    assigned_to_any: bool,
    /// Selected day's ISO date, `None` when no day is selected — lets the header's
    /// assigned-to-me checkbox and New-item links carry the current day/type forward across a
    /// full-page reload instead of silently closing the drawer.
    selected_date_iso: Option<String>,
    active_type: &'static str,
    nav_html: String,
}

/// Stage 2: the day-drawer's header data (date label + prev/next-day arrows) — `None` renders
/// nothing (the drawer starts closed with empty content until a day is actually picked), `Some`
/// only when a date is selected, so every field below is guaranteed present together rather than
/// requiring a separate `{% if let Some(..) %}` per field in the template.
///
/// Stage 3 additions: `date_year`/`date_month` (the *selected* date's own year/month, not the
/// grid's currently-displayed one — same rationale as `prev_year`/`prev_month` below) so the
/// All/Tasks/Events tabs can build a correct `hx-push-url` without the template needing the
/// grid's month separately; `active_type`/`assigned_to_any` so the tabs and prev/next-day arrows
/// can all carry the drawer's current filter state forward on every request, rather than each
/// click silently resetting it.
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

/// See `project_tasks::templates::ProjectTasksCalendarDayPanelTemplate`'s identical rationale —
/// this is also the `#day-drawer` dialog's own innerHTML for the `.../calendar/day` fragment
/// route, not just the calendar page's initial embed (Stage 2's drawer shell reuses one template
/// for both, matching `#action-dialog`/`#error-dialog`'s existing "swap innerHTML of a persistent
/// dialog" convention).
#[derive(Template)]
#[template(path = "project_calendar/calendar_day_panel.html")]
struct ProjectCalendarDayPanelTemplate {
    project_id: String,
    drawer: Option<DayDrawerData>,
    day_rows: Vec<String>,
}

/// Builds the `#day-drawer` dialog's innerHTML, shared between the calendar page's initial embed
/// and the `.../calendar/day` fragment route — see `ProjectCalendarDayPanelTemplate`'s doc
/// comment.
fn render_day_drawer(
    project_id: &str,
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
    Ok(ProjectCalendarDayPanelTemplate {
        project_id: project_id.to_string(),
        drawer,
        day_rows,
    }
    .render()?)
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

/// The first (Sunday-start) cell of the 6-row grid for `year`/`month` — hoisted out of
/// `build_calendar_days` so the handler can compute the same grid's UTC date range before
/// calling it (to bound the virtual-occurrence lookup).
fn grid_start_for(year: i32, month: u32) -> NaiveDate {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_sunday();
    first_of_month - Duration::days(leading as i64)
}

/// Converts a local calendar date + time-of-day into the UTC instant it represents, given
/// `tz_offset_minutes` — same `local + offset = utc` convention as
/// `project_events::combine_local_to_utc`.
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

/// Mirrors `project_tasks::build_calendar_days`'s identical redesign rationale (per
/// docs/issues_and_features.md's calendar-view entry): a cell only needs a tally now, the full
/// list for a clicked day renders separately via `day_list_rows`. This never filters out
/// completed items — matching the tally this replaced, which also counted them. A materialized
/// occurrence never appears in `virtual_occurrences` — it's already a real `items` row
/// covered by `due_items` (see `series::list_virtual_occurrences_for_project_unchecked`).
///
/// Stage 3: the tally *does* apply the assigned-to-me filter (unlike the drawer's own
/// All/Tasks/Events type tab, which deliberately leaves the tally unfiltered — see
/// docs/calendar-day-drawer-plan.md's "Confirmed design decisions") since toggling
/// assigned-to-me reloads this whole page. Same `is_team_project` gate as `day_list_rows` —
/// see that function's doc comment for why an ungated version of this filter would be wrong
/// here.
#[allow(clippy::too_many_arguments)]
fn build_calendar_days(
    year: i32,
    month: u32,
    due_items: &[DueItem],
    virtual_occurrences: &[ProjectOccurrence],
    tz: i32,
    today: NaiveDate,
    selected_date: Option<NaiveDate>,
    user_id: &str,
    assigned_to_any: bool,
    is_team_project: bool,
) -> Vec<ProjectCalendarDay> {
    let grid_start = grid_start_for(year, month);
    let mine = |assigned: Option<String>| {
        !is_team_project || assigned_to_any || assigned == Some(user_id.to_string())
    };

    let mut counts: std::collections::HashMap<NaiveDate, usize> = std::collections::HashMap::new();
    for di in due_items {
        let item = &di.item;
        if item.kind() == ItemKind::Simple {
            continue;
        }
        // Events are never assignment-gated (see CLAUDE.md's Cross-project scoping rule /
        // main_calendar::is_included's identical carve-out) — an Event or Event-series
        // occurrence has no meaningful "not mine" state, and most don't carry an assignee
        // at all, so gating them here silently hid every Event series occurrence on a
        // team-backed project.
        if item.kind() != ItemKind::Event && !mine(item.assigned_to_user_id()) {
            continue;
        }
        if let Some(dt) = calendar_date(item) {
            *counts.entry(to_local(dt, tz).date_naive()).or_default() += 1;
        }
    }
    for occ in virtual_occurrences {
        if occ.item_type != ItemKind::Event && !mine(occ.assigned_to_user_id.clone()) {
            continue;
        }
        let local = to_local(occ.occurrence_date, tz);
        *counts.entry(local.date_naive()).or_default() += 1;
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + Duration::days(i);
        days.push(ProjectCalendarDay {
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
    /// See `project_tasks::handlers::CalendarQuery::date`'s identical rationale.
    date: Option<String>,
    /// Stage 3's drawer type tab (`all`/`task`/`event`) — parsed via `parse_type_filter`.
    /// Only meaningful alongside `date`; ignored when no day is selected.
    r#type: Option<String>,
    /// Stage 3's assigned-to-me toggle — `None`/absent = mine, present = everyone's.
    assigned_to_any: Option<String>,
}

pub async fn project_calendar_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ProjectCalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
    // Stage B of docs/unify-virtual-materialized-occurrences-plan.md: see the list view's
    // identical rationale above — `Materialized` entries are filtered out here (those already
    // render via `due_items`, fed into `build_calendar_days` separately), leaving Virtual and
    // Skipped visible.
    let virtual_occurrences = series_service::list_occurrence_states_for_project(
        &series,
        &users,
        &project_id,
        range_start,
        range_end,
        tz,
    )
    .await?
    .into_iter()
    .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
    .collect::<Vec<_>>();
    let days = build_calendar_days(
        year,
        month,
        &due_items,
        &virtual_occurrences,
        tz,
        today,
        selected_date,
        &auth_user.user_id,
        assigned_to_any,
        project.team_id.is_some(),
    );
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::None,
    )
    .await?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let day_rows = match selected_date {
        Some(date) => {
            day_list_rows(
                &repo,
                &due_items,
                &virtual_occurrences,
                &project_id,
                &names,
                project.team_id.is_some(),
                date,
                tz,
                &auth_user.user_id,
                assigned_to_any,
                type_filter,
                &series,
            )
            .await?
        }
        None => Vec::new(),
    };
    let day_drawer_html = render_day_drawer(
        &project_id,
        selected_date,
        day_rows,
        type_filter,
        assigned_to_any,
    )?;

    render(ProjectCalendarPageTemplate {
        project_id,
        month_label: NaiveDate::from_ymd_opt(year, month, 1)
            .unwrap()
            .format("%B %Y")
            .to_string(),
        month_iso: format!("{year:04}-{month:02}"),
        year,
        month,
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

pub async fn project_calendar_day_fragment(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ProjectCalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let date = q
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or(ItemError::Invalid("date is required".to_string()))?;
    let assigned_to_any = q.assigned_to_any.is_some();
    let type_filter = parse_type_filter(q.r#type.as_deref());
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
    let range_start = local_date_to_utc(date, start_of_day(), tz);
    let range_end = local_date_to_utc(date, end_of_day(), tz);
    let virtual_occurrences = series_service::list_occurrence_states_for_project(
        &series,
        &users,
        &project_id,
        range_start,
        range_end,
        tz,
    )
    .await?
    .into_iter()
    .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
    .collect::<Vec<_>>();
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let day_rows = day_list_rows(
        &repo,
        &due_items,
        &virtual_occurrences,
        &project_id,
        &names,
        project.team_id.is_some(),
        date,
        tz,
        &auth_user.user_id,
        assigned_to_any,
        type_filter,
        &series,
    )
    .await?;
    Ok(Html(render_day_drawer(
        &project_id,
        Some(date),
        day_rows,
        type_filter,
        assigned_to_any,
    )?))
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCalendarToggleForm {
    complete: Option<String>,
}

pub async fn toggle_project_calendar_item_complete(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectCalendarToggleForm>,
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
        priority: current.priority(),
        depends_on_item_ids: None,
    };
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &reminders,
        &item_dependencies,
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
                &names,
                project.team_id.is_some(),
                tz,
                skip_url,
                false,
                None,
                None,
                children_html,
            )?))
        }
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
