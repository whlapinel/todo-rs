use super::nav::{self, ActiveContext, SidebarSection};
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
    /// `None` for an Event row — `Item::validate` rejects `complete: true` for
    /// `ItemType::Event` outright, so there's no toggle to render (mirrors
    /// `components::row::Row`'s `complete_url` escape hatch used everywhere else).
    toggle_target: Option<String>,
    detail_link: String,
    toggle_complete_json: String,
    /// `components::row::Row`-style confirm-then-fade badge — see that struct's identical
    /// fields. Only ever set by `render_rows`'s `just_completed_item_id` force-include, when
    /// this row is the materialized result of completing a virtual series occurrence from
    /// this same screen (see `ProjectDashboardVirtualRow::complete_url`) — `None` everywhere
    /// else, including `toggle_project_dashboard_item_complete`'s own direct-item checkbox,
    /// which is unrelated to this pass. See the archived "dashboard checkbox parity" follow-up
    /// (2026-08-21).
    confirmation: Option<String>,
    dismiss_after_ms: Option<u32>,
}

impl ProjectDashboardRow {
    #[allow(clippy::too_many_arguments)]
    fn from_due_item(
        di: &DueItem,
        project_id: &str,
        names: &HashMap<String, String>,
        is_team_project: bool,
        tz: i32,
        confirmation: Option<String>,
        dismiss_after_ms: Option<u32>,
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
            can_delete: !is_team_project,
            toggle_target: (item.kind() != ItemKind::Event)
                .then(|| format!("/web/projects/{project_id}/dashboard/items/{}", item.id)),
            detail_link: detail_url(item, project_id),
            toggle_complete_json: (!item.complete).to_string(),
            confirmation,
            dismiss_after_ms,
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
    /// Stage 9: whether this is the series' `current_occurrence_date` — see
    /// `service::item_series::current_occurrence_date`'s doc comment.
    is_current: bool,
    assignee_name: Option<String>,
    /// See `project_tasks::templates::ProjectTaskVirtualRow::is_skipped`'s identical
    /// rationale.
    is_skipped: bool,
    unskip_url: String,
    /// `Some` only for a Task-typed occurrence (`Item::validate` rejects `complete: true` for
    /// Events, mirroring `ProjectDashboardRow::toggle_target`'s identical `None`-for-Event
    /// convention above) that's also `is_current`, giving the dashboard's virtual row the same
    /// completability the Tasks list's `ProjectTaskVirtualRow` already has for the one
    /// occurrence per series that's actually completable. Unlike the flat Tasks list (which
    /// only ever lists the current occurrence to begin with — see
    /// `project_tasks::handlers::project_tasks_page`), this page also shows future "Planned"
    /// occurrences; without the `is_current` gate a Planned row's checkbox would reliably 400
    /// (`item_series::require_current_occurrence`'s "cannot settle this occurrence out of
    /// order" — confirmed live) instead of doing anything, so the gate keeps this from being a
    /// checkbox that predictably errors on every click but one. Reuses the exact same route
    /// `ProjectTaskVirtualRow::complete_url` points at (`complete_project_item_series_
    /// occurrence_form`) — that handler's `view=project-dashboard` branch (see its doc comment)
    /// rebuilds `#project-dashboard-list` in place via `list_query`/`in_list_view` below,
    /// mirroring the flat Tasks list's own `view=tasks-list` treatment (see the archived
    /// "extend confirm-then-fade to virtual occurrences" entry, 2026-08-21) now that this
    /// screen's `preset`/`show_complete`/`assigned_to_any` filter state is baked into the URL
    /// too, closing the gap the original "dashboard checkbox parity" follow-up (2026-08-21)
    /// left open.
    complete_url: Option<String>,
    /// `true` only when this row is being rendered for the flat Dashboard list (`render_rows`),
    /// never the calendar day panel (`day_list_rows`) — gates whether the checkbox/Skip/Unskip
    /// carry `hx-target="#project-dashboard-list"` at all, mirroring `ProjectTaskVirtualRow::
    /// in_list_view`'s identical rationale. The calendar day panel has no such container in its
    /// DOM, so those buttons there fall back to the pre-existing whole-page `redirect_to_
    /// current_page` behavior.
    in_list_view: bool,
}

impl ProjectDashboardVirtualRow {
    /// `list_query` is `Some(&query_string)` (baked from `dashboard_list_query`) only when
    /// rendering for the flat list — `None` for the calendar day panel, matching
    /// `ProjectTaskVirtualRow::from_occurrence`'s identical `in_list_view`-gated pattern.
    fn from_occurrence(occ: &ProjectOccurrence, project_id: &str, tz: i32, list_query: Option<&str>) -> Self {
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

#[derive(Template)]
#[template(path = "project_dashboard/page.html")]
struct ProjectDashboardPageTemplate {
    project_id: String,
    rows: Vec<String>,
    show_complete: bool,
    assigned_to_any: bool,
    presets: Vec<(&'static str, bool)>,
    nav_html: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DashboardQuery {
    preset: Option<String>,
    show_complete: Option<String>,
    assigned_to_any: Option<String>,
}

/// Bakes this screen's three filter dimensions into a query-string suffix for a virtual row's
/// checkbox/Skip/Unskip URLs, so an in-place `#project-dashboard-list` rebuild
/// (`complete_project_item_series_occurrence_form`'s/Skip's/Unskip's `view=project-dashboard`
/// branch) knows how to re-render "the same view" — mirrors `ProjectTaskVirtualRow::
/// from_occurrence`'s `list_query`, extended with this screen's two extra filters (`preset`,
/// `assignedToAny`) since, unlike the flat Tasks list, this view has more than one filter
/// dimension to round-trip. `preset`'s value is a closed, hardcoded set (`PRESETS` above) of
/// plain-ASCII-plus-spaces strings, so a bare space-to-%20 substitution is sufficient escaping
/// — no general percent-encoding is needed for this fixed vocabulary.
fn dashboard_list_query(preset: &str, show_complete: bool, assigned_to_any: bool) -> String {
    let mut params = vec![
        "view=project-dashboard".to_string(),
        format!("preset={}", preset.replace(' ', "%20")),
    ];
    if show_complete {
        params.push("showComplete=1".to_string());
    }
    if assigned_to_any {
        params.push("assignedToAny=1".to_string());
    }
    format!("?{}", params.join("&"))
}

/// Same Rust-side filtering rationale as `dashboard::render_rows` — an unfiltered SQL fetch
/// (both `after`/`before` `None`) followed by filtering against `dashboard_date` here, so an
/// Event showing up by `scheduled_date` isn't excluded for lacking a `due_date`.
#[allow(clippy::too_many_arguments)]
fn render_rows(
    user_id: &str,
    items: &[DueItem],
    virtual_occurrences: &[ProjectOccurrence],
    project_id: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    preset: &str,
    show_complete: bool,
    assigned_to_any: bool,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    tz: i32,
    // See `project_tasks::render_rows_with_virtual`'s identical rationale — force-includes a
    // just-completed item (the materialized result of completing a virtual occurrence from
    // this same screen) so it gets a moment to display its confirm-then-fade badge even when
    // `show_complete` would otherwise filter it straight back out.
    just_completed_item_id: Option<&str>,
) -> Result<Vec<String>, ItemError> {
    let mut items: Vec<&DueItem> = items
        .iter()
        .filter(|di| di.item.kind() != ItemKind::Simple)
        .filter(|di| assigned_to_any || di.item.assigned_to_user_id() == Some(user_id.to_string()))
        .filter(|di| {
            show_complete || !di.item.complete || Some(di.item.id.as_str()) == just_completed_item_id
        })
        .filter(|di| preset != "All with due date" || dashboard_date(&di.item).is_some())
        .filter(|di| match dashboard_date(&di.item) {
            Some(d) => after.is_none_or(|a| d >= a) && before.is_none_or(|b| d <= b),
            None => after.is_none() && before.is_none(),
        })
        .collect();
    items.sort_by_key(|di| {
        dashboard_date(&di.item)
            .map(|d| d.timestamp())
            .unwrap_or(i64::MAX)
    });

    let mut entries: Vec<(i64, String)> = items
        .iter()
        .map(|di| {
            let ts = dashboard_date(&di.item)
                .map(|d| d.timestamp())
                .unwrap_or(i64::MAX);
            let just_completed = Some(di.item.id.as_str()) == just_completed_item_id;
            let confirmation = just_completed.then(|| "Completed".to_string());
            let dismiss_after_ms = (just_completed && !show_complete).then_some(1800u32);
            ProjectDashboardRow::from_due_item(
                di,
                project_id,
                names,
                is_team_project,
                tz,
                confirmation,
                dismiss_after_ms,
            )
            .render()
            .map(|html| (ts, html))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Virtual occurrences have no `complete`/due-date-presence concept, so
    // `show_complete`/"All with due date" are no-ops for them by construction — every entry
    // here is always dated and always incomplete in effect.
    //
    // The Task-typed past-date clamp (Stage 8) and `is_current` computation (Stage 9) now
    // live inside `item_series::list_occurrence_states_for_project` itself (mirroring
    // `list_virtual_occurrences_for_project_unchecked`'s own logic) — a backlogged Task
    // series' one settleable occurrence surfaces here like any other entry, just marked
    // `is_current` for the template to badge.
    //
    // Stage B of docs/unify-virtual-materialized-occurrences-plan.md: `virtual_occurrences`
    // now also carries already-settled dates (materialized or skipped) — materialized ones
    // are excluded here since they already render via `items` above (a real `DueItem` row),
    // leaving Virtual and Skipped to render as before, the latter now struck through with
    // an Unskip button rather than simply never appearing.
    let filtered_occurrences: Vec<&ProjectOccurrence> = virtual_occurrences
        .iter()
        .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
        .filter(|occ| assigned_to_any || occ.assigned_to_user_id == Some(user_id.to_string()))
        .collect();
    let list_query = dashboard_list_query(preset, show_complete, assigned_to_any);
    for occ in filtered_occurrences {
        entries.push((
            occ.occurrence_date.timestamp(),
            ProjectDashboardVirtualRow::from_occurrence(occ, project_id, tz, Some(&list_query))
                .render()?,
        ));
    }

    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

/// The calendar's per-day panel — see `project_tasks::day_list_rows`'s identical rationale.
/// Unlike `render_rows` above (the flat dashboard page's preset/assignment-filtered view),
/// this shows every item/occurrence on `date` unfiltered, matching what the calendar grid's
/// day cells already aggregated from before this redesign — reusing
/// `ProjectDashboardRow`/`ProjectDashboardVirtualRow` as-is (no feature upgrade to virtual-row
/// completability; that's tracked separately in docs/issues_and_features.md).
fn day_list_rows(
    due_items: &[DueItem],
    virtual_occurrences: &[ProjectOccurrence],
    project_id: &str,
    names: &HashMap<String, String>,
    is_team_project: bool,
    date: NaiveDate,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    let mut entries: Vec<(i64, String)> = due_items
        .iter()
        .filter(|di| di.item.kind() != ItemKind::Simple)
        .filter(|di| {
            dashboard_date(&di.item).map(|d| to_local(d, tz).date_naive()) == Some(date)
        })
        .map(|di| {
            let ts = dashboard_date(&di.item)
                .map(|d| d.timestamp())
                .unwrap_or(i64::MAX);
            ProjectDashboardRow::from_due_item(di, project_id, names, is_team_project, tz, None, None)
                .render()
                .map(|html| (ts, html))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for occ in virtual_occurrences
        .iter()
        .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }))
        .filter(|occ| to_local(occ.occurrence_date, tz).date_naive() == date)
    {
        entries.push((
            occ.occurrence_date.timestamp(),
            ProjectDashboardVirtualRow::from_occurrence(occ, project_id, tz, None).render()?,
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

/// The flat Dashboard list's row assembly, shared between the initial page load
/// (`project_dashboard_page`) and the in-place `#project-dashboard-list` rebuild the
/// checkbox/Skip/Unskip handlers do on `view=project-dashboard` (`complete_project_item_series_
/// occurrence_form`, `skip_project_item_series_occurrence_form`, `unskip_project_item_series_
/// occurrence_form`) — mirrors `project_tasks::list_task_rows_for_project`'s identical
/// rationale, so a mutation's response and a fresh reload can never drift apart.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn list_dashboard_rows_for_project(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    users: &Arc<dyn UserRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    project_id: &str,
    requester_user_id: &str,
    preset: &str,
    show_complete: bool,
    assigned_to_any: bool,
    tz: i32,
    just_completed_item_id: Option<&str>,
) -> Result<Vec<String>, ItemError> {
    let project = project_service::get_project(projects, teams, project_id, requester_user_id).await?;
    let (after, before) = preset_range(preset, Utc::now(), tz);
    let due_items =
        project_item_service::list_due_project_items_unchecked(repo, project_id, None, None).await?;
    let (virtual_after, virtual_before) = virtual_occurrence_window(after, before, Utc::now());
    let virtual_occurrences = if virtual_after <= virtual_before {
        series_service::list_occurrence_states_for_project(
            series,
            users,
            project_id,
            virtual_after,
            virtual_before,
            tz,
        )
        .await?
    } else {
        Vec::new()
    };
    let names = match &project.team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    render_rows(
        requester_user_id,
        &due_items,
        &virtual_occurrences,
        project_id,
        &names,
        project.team_id.is_some(),
        preset,
        show_complete,
        assigned_to_any,
        after,
        before,
        tz,
        just_completed_item_id,
    )
}

/// The `#project-dashboard-list` innerHTML swap body — see `project_tasks::
/// items_list_inner_html`'s identical rationale, matching `project_dashboard/page.html`'s own
/// empty-state markup so an in-place rebuild that empties the list looks identical to a fresh
/// load that finds nothing.
pub(crate) fn dashboard_items_inner_html(rows: &[String]) -> String {
    if rows.is_empty() {
        "<li class=\"py-3 text-sm text-gray-500 dark:text-gray-400\">No items.</li>".to_string()
    } else {
        rows.concat()
    }
}

pub async fn project_dashboard_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz_offset): TzOffset,
    Query(q): Query<DashboardQuery>,
) -> Result<Html<String>, ItemError> {
    let preset = q.preset.unwrap_or_else(|| "Today".to_string());
    let show_complete = q.show_complete.is_some();
    let assigned_to_any = q.assigned_to_any.is_some();

    let rows = list_dashboard_rows_for_project(
        &repo,
        &projects,
        &teams,
        &users,
        &series,
        &project_id,
        &auth_user.user_id,
        &preset,
        show_complete,
        assigned_to_any,
        tz_offset,
        None,
    )
    .await?;

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
        assigned_to_any,
        presets,
        nav_html,
    })
}

/// Redesign per docs/issues_and_features.md's calendar-view entry: a day cell only shows a
/// count hint now, not the items themselves — see `ProjectDashboardCalendarPageTemplate`'s
/// doc comment for where the full list moved to, mirroring
/// `project_tasks::templates::CalendarDay`.
struct ProjectDashboardCalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    is_selected: bool,
    entry_count: usize,
}

#[derive(Template)]
#[template(path = "project_dashboard/calendar_page.html")]
struct ProjectDashboardCalendarPageTemplate {
    project_id: String,
    month_label: String,
    month_iso: String,
    year: i32,
    month: u32,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<ProjectDashboardCalendarDay>,
    /// Stage 2: whether the day-drawer fragment below actually has a day to show — gates the
    /// inline `showModal()` script (a hard/bookmarked load of `?date=...` needs to open the
    /// drawer without any htmx swap ever firing `htmx:afterSwap`, so nothing else would open it).
    has_selected_date: bool,
    /// The `#day-drawer` dialog's initial innerHTML — rendered via `ProjectDashboardCalendarDayPanelTemplate`
    /// the same way `day_rows` renders individual rows, so the page template doesn't need to
    /// duplicate the drawer's own fields (header, arrows, list) itself.
    day_drawer_html: String,
    nav_html: String,
}

/// Stage 2: the day-drawer's header data (date label + prev/next-day arrows) — `None` renders
/// nothing (the drawer starts closed with empty content until a day is actually picked), `Some`
/// only when a date is selected, so every field below is guaranteed present together rather than
/// requiring a separate `{% if let Some(..) %}` per field in the template.
struct DayDrawerData {
    date_iso: String,
    selected_date_label: String,
    prev_date: String,
    prev_year: i32,
    prev_month: u32,
    next_date: String,
    next_year: i32,
    next_month: u32,
}

/// See `project_tasks::templates::ProjectTasksCalendarDayPanelTemplate`'s identical rationale —
/// this is also the `#day-drawer` dialog's own innerHTML for the `.../calendar/day` fragment
/// route, not just the calendar page's initial embed (Stage 2's drawer shell reuses one template
/// for both, matching `#action-dialog`/`#error-dialog`'s existing "swap innerHTML of a persistent
/// dialog" convention).
#[derive(Template)]
#[template(path = "project_dashboard/calendar_day_panel.html")]
struct ProjectDashboardCalendarDayPanelTemplate {
    project_id: String,
    drawer: Option<DayDrawerData>,
    day_rows: Vec<String>,
}

/// Builds the `#day-drawer` dialog's innerHTML, shared between the calendar page's initial embed
/// and the `.../calendar/day` fragment route — see `ProjectDashboardCalendarDayPanelTemplate`'s
/// doc comment.
fn render_day_drawer(
    project_id: &str,
    date: Option<NaiveDate>,
    day_rows: Vec<String>,
) -> Result<String, ItemError> {
    let drawer = date.map(|d| {
        let prev = d - Duration::days(1);
        let next = d + Duration::days(1);
        DayDrawerData {
            date_iso: d.format("%Y-%m-%d").to_string(),
            selected_date_label: day_panel_label(d),
            prev_date: prev.format("%Y-%m-%d").to_string(),
            prev_year: prev.year(),
            prev_month: prev.month(),
            next_date: next.format("%Y-%m-%d").to_string(),
            next_year: next.year(),
            next_month: next.month(),
        }
    });
    Ok(ProjectDashboardCalendarDayPanelTemplate {
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
/// list for a clicked day renders separately via `day_list_rows`. Unlike the list view
/// (`project_dashboard_page`'s `show_complete` filter), this never filters out completed
/// items — matching the tally this replaced, which also counted them. A materialized
/// occurrence never appears in `virtual_occurrences` — it's already a real `items` row
/// covered by `due_items` (see `series::list_virtual_occurrences_for_project_unchecked`).
fn build_calendar_days(
    year: i32,
    month: u32,
    due_items: &[DueItem],
    virtual_occurrences: &[ProjectOccurrence],
    tz: i32,
    today: NaiveDate,
    selected_date: Option<NaiveDate>,
) -> Vec<ProjectDashboardCalendarDay> {
    let grid_start = grid_start_for(year, month);

    let mut counts: std::collections::HashMap<NaiveDate, usize> =
        std::collections::HashMap::new();
    for di in due_items {
        let item = &di.item;
        if item.kind() == ItemKind::Simple {
            continue;
        }
        if let Some(dt) = dashboard_date(item) {
            *counts.entry(to_local(dt, tz).date_naive()).or_default() += 1;
        }
    }
    for occ in virtual_occurrences {
        let local = to_local(occ.occurrence_date, tz);
        *counts.entry(local.date_naive()).or_default() += 1;
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + Duration::days(i);
        days.push(ProjectDashboardCalendarDay {
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
pub struct CalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
    /// See `project_tasks::handlers::CalendarQuery::date`'s identical rationale.
    date: Option<String>,
}

/// See `project_tasks::handlers::day_panel_label`'s identical rationale.
fn day_panel_label(date: NaiveDate) -> String {
    date.format("%A, %B %d, %Y").to_string()
}

pub async fn project_dashboard_calendar_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
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
        Some(date) => day_list_rows(
            &due_items,
            &virtual_occurrences,
            &project_id,
            &names,
            project.team_id.is_some(),
            date,
            tz,
        )?,
        None => Vec::new(),
    };
    let day_drawer_html = render_day_drawer(&project_id, selected_date, day_rows)?;

    render(ProjectDashboardCalendarPageTemplate {
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
        nav_html,
    })
}

pub async fn project_dashboard_calendar_day_fragment(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let date = q
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or(ItemError::Invalid("date is required".to_string()))?;
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
        &due_items,
        &virtual_occurrences,
        &project_id,
        &names,
        project.team_id.is_some(),
        date,
        tz,
    )?;
    Ok(Html(render_day_drawer(&project_id, Some(date), day_rows)?))
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
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
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
        Ok(updated) => render(ProjectDashboardRow::from_due_item(
            &DueItem {
                parent_name: String::new(),
                item: updated,
            },
            &project_id,
            &names,
            project.team_id.is_some(),
            tz,
            None,
            None,
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
