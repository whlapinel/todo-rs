pub mod handlers;
pub mod templates;

use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{self as item_series_service, ProjectOccurrence};
use crate::service::project_items::list_project_items_unchecked;
use crate::service::teams as team_service;
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, TeamRepo, UserRepo};
use crate::web_ui::list_filters::{ListFilterQuery, ListFilters};
use crate::web_ui::project_tasks::templates::{
    ProjectTaskRow, ProjectTaskRowsFragmentTemplate, ProjectTaskVirtualRow,
};
use askama::Template;
use async_recursion::async_recursion;
use axum::response::Html;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being a Task — mirrors
/// `tasks::require_task`/`team_tasks::require_team_task`.
pub(crate) fn require_task(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Task {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Duplicated from tasks/team_tasks rather than shared, matching the precedent those two
// modules already set for this exact helper set.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskForm {
    name: Option<String>,
    description: Option<String>,
    due_date: Option<String>,
    due_time: Option<String>,
    scheduled_date: Option<String>,
    scheduled_time: Option<String>,
    scheduled_end_date: Option<String>,
    scheduled_end_time: Option<String>,
    complete: Option<String>,
    due_offset_days: Option<String>,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    /// Stage 2 of `docs/list-filtering-plan.md`: the list screen's non-default filters at the
    /// moment this form was rendered, pre-encoded via `ListFilters::query_string()` — a single
    /// opaque `key=value&key2=value2` fragment, not individual `ListFilterQuery`-shaped
    /// fields. Individual fields aren't an option here: `ListFilterQuery::due_date`'s wire name
    /// (`dueDate`) collides with this same form's own item-due-date input, since both this
    /// filter round-trip and the real item fields live in the same `<form>` — see
    /// `templates/project_tasks/new_page.html`'s "New task" dialog. Only actually consumed by
    /// the `redirect` branch of `create_project_task_form` (via `redirect_to_project_tasks`,
    /// which appends it to the redirect URL as-is); harmless and unread everywhere else
    /// `ProjectTaskForm` is posted (update forms have no redirect-to-list branch at all).
    filters_query: Option<String>,
    /// Only present/honored server-side on a team-backed project — see
    /// `service::team_items::create_team_item`/`update_team_item`'s own admin gate.
    assigned_to_user_id: Option<String>,
    /// Same team-only caveat as `assigned_to_user_id`.
    points: Option<String>,
    /// See `tasks::TaskForm`'s identical field for the redirect-vs-in-place-fragment
    /// rationale.
    redirect: Option<String>,
}

/// Set only when a Reschedule/Assign dialog (or its own save PUT) was reached from a
/// calendar screen's row — see `project_calendar::calendar_row`/`main_calendar::calendar_row`'s
/// override of `Row::reschedule_url`/`assign_url`, which append this as a query-string suffix.
/// Lets `update_project_task_form`/`update_project_event_form` re-render the saved row via that
/// screen's own `calendar_row` (badge/parent/project overlay) instead of the plain
/// `ProjectTaskRow`/`ProjectEventRow` shape — closing the gap noted in
/// docs/issues_and_features.md ("a row saved via Reschedule/Assign from a calendar day-drawer
/// briefly loses that styling"). Deliberately narrower than `OccurrenceRowActionQuery` (no
/// `preset`/`showComplete`/`assignedToAny`): unlike completing a series occurrence, a plain
/// reschedule/assign never shifts a series cursor, so it only ever needs a single-row swap, not
/// a whole-list rebuild.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RowViewQuery {
    view: Option<String>,
}

/// Only `"project-calendar"`/`"main-calendar"`/`"all-tasks"`/`"all-events"` are ever forwarded —
/// anything else (including a hand-crafted query value) normalizes to `None`, mirroring
/// `OccurrenceRowActionQuery`'s literal-match-or-fall-through convention rather than reflecting
/// arbitrary input back out. `"all-tasks"`/`"all-events"` were added in
/// `docs/all-projects-landing-plan.md` Stage 4 — `project_events/handlers.rs` reuses this same
/// function for its own `RowViewQuery`, so both cross-project screens' suffixes are recognized
/// here rather than via a second, parallel normalizer.
pub(crate) fn normalize_row_view(q: RowViewQuery) -> Option<String> {
    if matches!(
        q.view.as_deref(),
        Some("project-calendar") | Some("main-calendar") | Some("all-tasks") | Some("all-events")
    ) {
        q.view
    } else {
        None
    }
}

/// Builds a `ListFilters` from five loose `Option<String>` parts rather than requiring the
/// caller to already have a `ListFilterQuery` value — used by handlers whose own
/// `#[derive(Deserialize)] struct` carries the same five fields alongside a `view`/other param
/// that isn't part of the shared vocabulary (`OccurrenceRowActionQuery` in this module and in
/// `project_item_series::handlers`), so those structs can't just wrap/deref a `ListFilterQuery`
/// directly. Stage 2 of `docs/list-filtering-plan.md`.
pub(crate) fn list_filters_from_parts(
    show_complete: &Option<String>,
    assigned_to: &Option<String>,
    due_date: &Option<String>,
    schedule: &Option<String>,
    recurring: &Option<String>,
) -> ListFilters {
    ListFilters::from_query(ListFilterQuery {
        show_complete: show_complete.clone(),
        assigned_to: assigned_to.clone(),
        due_date: due_date.clone(),
        schedule: schedule.clone(),
        recurring: recurring.clone(),
    })
}

pub(crate) fn non_empty(v: &Option<String>) -> Option<String> {
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

pub(crate) fn create_params_from_form(
    project_id: &str,
    form: &ProjectTaskForm,
    tz: i32,
) -> crate::service::project_items::CreateProjectItemParams {
    crate::service::project_items::CreateProjectItemParams {
        project_id: project_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        description: non_empty(&form.description),
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
        points: form
            .points
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    }
}

pub(crate) fn update_params_from_form(
    project_id: &str,
    item_id: &str,
    current: &Item,
    form: &ProjectTaskForm,
    tz: i32,
) -> crate::service::project_items::UpdateProjectItemParams {
    crate::service::project_items::UpdateProjectItemParams {
        project_id: project_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        description: overlay_str(&form.description, current.description.clone()),
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
        assigned_to_user_id: overlay_str(&form.assigned_to_user_id, current.assigned_to_user_id()),
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        // No points input renders on a non-admin's/personal-project's form — `overlay_i32`
        // falls back to `current.points` when absent, mirroring `team_tasks.rs`'s identical
        // comment: a plain edit here can't silently wipe it, and the service layer's own
        // admin gate is what actually decides whether a *changed* value is honored.
        points: overlay_i32(&form.points, current.points()),
        event_type: current.event_type(),
    }
}

/// (user_id, display name) for every *active* member of `team_id` — the assignee dropdown's
/// candidate list. `None` inputs (personal project) are never called with this.
pub(crate) async fn active_member_options(
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

/// Unfiltered id -> display-name map (including inactive members), for resolving an
/// already-assigned member's name even if they've since left the team.
pub(crate) async fn names_for(
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

// ---- shared rendering helpers ------------------------------------------------

/// Fixed left-padding class for a nested row at `depth` levels below the flat list's own
/// top-level rows (`Row::indent_class`) — Tailwind's compiler only picks up class names that
/// appear literally in the source (no arbitrary computed `pl-{depth}`), so nesting visually
/// caps at 3 indent steps; anything deeper reuses the deepest one rather than growing further.
fn indent_class(depth: u8) -> &'static str {
    match depth {
        0 => "",
        1 => "pl-8",
        2 => "pl-12",
        _ => "pl-16",
    }
}

/// Recursively renders `parent_item_id`'s full descendant subtree as ready-to-insert `<li>`
/// markup, each descendant's own row in turn carrying its own nested `children_html` — the
/// flat Tasks list's in-place "expand to view sub-items" feature (see `Row::children_html`'s
/// doc comment). Everything is fetched and rendered eagerly, up front — one query per
/// `has_children` node — so the browser's expand/collapse toggle never round-trips to the
/// server; the cost is paid on every list load regardless of whether a branch is ever expanded.
#[async_recursion]
async fn render_expandable_children(
    repo: &Arc<dyn ItemRepo>,
    parent_item_id: &str,
    project_id: &str,
    names: &HashMap<String, String>,
    show_complete: bool,
    tz: i32,
    skip_urls: &HashMap<String, String>,
    team_id: Option<&str>,
    depth: u8,
) -> Result<String, ItemError> {
    let children =
        list_project_items_unchecked(repo, project_id, Some(parent_item_id.to_string())).await?;
    let visible: Vec<&Item> = children
        .iter()
        .filter(|i| show_complete || !i.complete)
        .collect();
    let mut html = String::new();
    for i in &visible {
        let mut row = ProjectTaskRow::from_item(
            i,
            project_id,
            names,
            &visible,
            tz,
            skip_urls.get(&i.id).cloned(),
            team_id.is_some(),
            show_complete,
            None,
            None,
        );
        row.indent_class = indent_class(depth);
        if i.has_children {
            row.children_html = Some(
                render_expandable_children(
                    repo,
                    &i.id,
                    project_id,
                    names,
                    show_complete,
                    tz,
                    skip_urls,
                    team_id,
                    depth + 1,
                )
                .await?,
            );
        }
        html.push_str(&row.render()?);
    }
    Ok(html)
}

pub(crate) fn render_rows(
    items: &[Item],
    project_id: &str,
    names: &HashMap<String, String>,
    show_complete: bool,
    tz: i32,
    skip_urls: &HashMap<String, String>,
    team_id: Option<&str>,
) -> Result<Vec<String>, ItemError> {
    let visible: Vec<&Item> = items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .collect();
    visible
        .iter()
        .map(|i| {
            ProjectTaskRow::from_item(
                i,
                project_id,
                names,
                &visible,
                tz,
                skip_urls.get(&i.id).cloned(),
                team_id.is_some(),
                show_complete,
                None,
                None,
            )
            .render()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// Stage 10 gap 2: the flat Tasks list's own version of `render_rows`, merging in each
/// Task-typed series' single current virtual occurrence (if any) alongside real items —
/// mirrors `project_calendar::render_rows`'s exact merge pattern (render each kind to
/// `(timestamp, html)` pairs, concatenate, sort by timestamp, discard the timestamp). Kept
/// separate from `render_rows` rather than adding a parameter to it, since `render_rows` has
/// three other call sites in this module (children/subordinate task lists) where virtual
/// occurrences don't apply.
///
/// `just_completed_item_id` (added for the virtual-occurrence confirm-then-fade-away follow-up,
/// see `handlers::complete_project_item_series_occurrence_form`) forces that one materialized
/// item's row to stay visible even when `filters` would otherwise exclude it (typically because
/// it just became complete and `filters.show_complete` is off) — without the force-include, the
/// row would simply vanish on this fresh render instead of getting a moment to show its
/// "Completed" badge before `Row`'s own `data-dismiss-after` JS removes it client-side.
/// `in_list_view` is threaded onto `ProjectTaskVirtualRow` so its checkbox/Skip/Unskip only
/// target `#items-list` (see `handlers::list_task_rows_for_project`) when this is really the
/// flat list's own render, not a subordinate/children list render, which has no `#items-list`
/// in its DOM.
///
/// Stage 2 of `docs/list-filtering-plan.md`: `filters`/`requester_user_id` replace the old
/// `show_complete: bool` — every dimension in `ListFilters::matches` now gates visibility, not
/// just completion. `filters.recurring == false` additionally drops every virtual occurrence
/// outright (a virtual row is always a series occurrence, so "hide recurring" means "show none
/// of them") — materialized items whose `series_id` is set are excluded the same way by
/// `matches` itself, so no separate handling is needed for those.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn render_rows_with_virtual(
    repo: &Arc<dyn ItemRepo>,
    items: &[Item],
    virtual_occurrences: &[ProjectOccurrence],
    project_id: &str,
    names: &HashMap<String, String>,
    filters: &ListFilters,
    requester_user_id: &str,
    tz: i32,
    skip_urls: &HashMap<String, String>,
    team_id: Option<&str>,
    just_completed_item_id: Option<&str>,
    in_list_view: bool,
) -> Result<Vec<String>, ItemError> {
    let now = Utc::now();
    let is_team_project = team_id.is_some();
    let visible: Vec<&Item> = items
        .iter()
        .filter(|i| {
            filters.matches(i, requester_user_id, is_team_project, now)
                || Some(i.id.as_str()) == just_completed_item_id
        })
        .collect();
    let mut entries: Vec<(i64, String)> = Vec::with_capacity(visible.len());
    for i in &visible {
        let just_completed = Some(i.id.as_str()) == just_completed_item_id;
        let confirmation = just_completed.then(|| "Completed".to_string());
        let dismiss_after_ms = (just_completed && !filters.show_complete).then_some(1800u32);
        let mut row = ProjectTaskRow::from_item(
            i,
            project_id,
            names,
            &visible,
            tz,
            skip_urls.get(&i.id).cloned(),
            is_team_project,
            filters.show_complete,
            confirmation,
            dismiss_after_ms,
        );
        // In-place expansion (see `Row::children_html`'s doc comment) — the flat list is the
        // one screen that opts a `ProjectTaskRow` into this, eagerly inlining the whole
        // subtree so the browser's toggle never has to fetch. Leaf items (no children) are
        // left at `ProjectTaskRow::from_item`'s own default (`None`), keeping their original
        // name-click-opens-detail behavior.
        if i.has_children {
            row.children_html = Some(
                render_expandable_children(
                    repo,
                    &i.id,
                    project_id,
                    names,
                    filters.show_complete,
                    tz,
                    skip_urls,
                    team_id,
                    1,
                )
                .await?,
            );
        }
        entries.push((sort_key(i), row.render()?));
    }
    if filters.recurring {
        for occ in virtual_occurrences {
            entries.push((
                occ.occurrence_date.timestamp(),
                ProjectTaskVirtualRow::from_occurrence(occ, project_id, tz, filters, in_list_view)
                    .render()?,
            ));
        }
    }
    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

/// Renders exactly what `#items-list` (`templates/project_tasks/list_page.html`) shows for
/// `project_id` — the same items + current-virtual-occurrence query `handlers::
/// project_tasks_page` builds the initial page load from, factored out so the in-place
/// checkbox/Skip/Unskip handlers (`handlers::complete_project_item_series_occurrence_form` and
/// the `view=tasks-list` branch of `project_item_series::handlers::
/// skip_project_item_series_occurrence_form`/`unskip_project_item_series_occurrence_form`) can
/// rebuild the whole list in place after their mutation, rather than doing a full `HX-Redirect`
/// page reload. A whole-list rebuild (not a single-row swap) is deliberate: completing or
/// skipping a series' current occurrence can advance its cursor to a new current occurrence,
/// which needs to actually appear in the list — a single `<li>` outerHTML swap could never do
/// that. See `render_rows_with_virtual`'s `just_completed_item_id` for how the completing row
/// still gets its own confirm-then-fade-away treatment despite the full rebuild.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn list_task_rows_for_project(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    users: &Arc<dyn UserRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    project_id: &str,
    team_id: Option<&str>,
    requester_user_id: &str,
    filters: &ListFilters,
    tz: i32,
    just_completed_item_id: Option<&str>,
) -> Result<Vec<String>, ItemError> {
    let items = list_project_tasks(repo, project_id).await?;
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    // See `handlers::project_tasks_page`'s identical query/filter — kept in sync deliberately.
    // Skipped entirely when `filters.recurring` is off: `render_rows_with_virtual` would drop
    // every entry from this anyway (a virtual row is always a series occurrence), so there's no
    // reason to pay for the query.
    let virtual_occurrences: Vec<_> = if filters.recurring {
        item_series_service::list_occurrence_states_for_project(
            series,
            users,
            project_id,
            Utc::now(),
            Utc::now(),
            tz,
        )
        .await?
        .into_iter()
        .filter(|occ| occ.item_type == ItemKind::Task && occ.is_current)
        .filter(|occ| {
            !matches!(
                occ.state,
                item_series_service::OccurrenceState::Materialized { .. }
            )
        })
        .collect()
    } else {
        Vec::new()
    };
    let mut skip_urls: HashMap<String, String> = HashMap::new();
    for item in &items {
        if let Some(url) = item_series_service::skip_url_for_item(series, item, project_id).await? {
            skip_urls.insert(item.id.clone(), url);
        }
    }
    render_rows_with_virtual(
        repo,
        &items,
        &virtual_occurrences,
        project_id,
        &names,
        filters,
        requester_user_id,
        tz,
        &skip_urls,
        team_id,
        just_completed_item_id,
        true,
    )
    .await
}

/// The `#items-list` placeholder markup (`templates/project_tasks/list_page.html`'s own
/// `rows.is_empty()` branch) — duplicated here rather than shared via Askama, since this is
/// only ever swapped in as raw `innerHTML` by `list_task_rows_for_project`'s callers, never
/// rendered through a `Template` struct of its own.
pub(crate) fn items_list_inner_html(rows: &[String]) -> String {
    if rows.is_empty() {
        "<li class=\"py-3 text-sm text-gray-500 dark:text-gray-400\">No tasks yet.</li>".to_string()
    } else {
        rows.concat()
    }
}

/// `list_project_items_unchecked` already scopes to top-level, non-Template items — this narrows
/// further to `Task` and sorts by due date (undated tasks last), mirroring
/// `tasks::list_tasks`/`team_tasks::list_team_tasks`.
fn sort_key(item: &Item) -> i64 {
    item.due_date().map(|d| d.timestamp()).unwrap_or(i64::MAX)
}

pub(crate) async fn list_project_tasks(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
) -> Result<Vec<Item>, ItemError> {
    let mut items = list_project_items_unchecked(repo, project_id, None).await?;
    items.retain(|i| i.kind() == ItemKind::Task);
    items.sort_by_key(sort_key);
    Ok(items)
}

/// The full sibling group (including the item itself) a given item belongs to — see
/// `tasks::sibling_group`'s identical rationale.
pub(crate) async fn sibling_group(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    parent_item_id: Option<&str>,
) -> Result<Vec<Item>, ItemError> {
    match parent_item_id {
        Some(pid) => list_project_items_unchecked(repo, project_id, Some(pid.to_string())).await,
        None => list_project_tasks(repo, project_id).await,
    }
}

pub(crate) async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    team_id: Option<&str>,
    requester_user_id: &str,
    parent_item_id: Option<&str>,
    show_complete: bool,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            list_project_items_unchecked(repo, project_id, Some(parent_id.to_string())).await?,
            "No sub-items yet.",
        )
    } else {
        (list_project_tasks(repo, project_id).await?, "No tasks yet.")
    };
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let rows = render_rows(
        &items,
        project_id,
        &names,
        parent_item_id.is_some() || show_complete,
        tz,
        &HashMap::new(),
        team_id,
    )?;
    render(ProjectTaskRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
}
