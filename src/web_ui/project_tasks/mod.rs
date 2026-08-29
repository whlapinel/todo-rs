pub mod handlers;
pub mod templates;

use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::{self as item_series_service, ProjectOccurrence};
use crate::service::project_items::list_project_items_unchecked;
use crate::service::teams as team_service;
use crate::storage::sqlite::{ItemDependencyRepo, ItemRepo, ItemSeriesRepo, TeamRepo, UserRepo};
use crate::web_ui::list_filters::{ListFilterQuery, ListFilters};
use crate::web_ui::project_tasks::templates::{
    ProjectTaskRow, ProjectTaskRowsFragmentTemplate, ProjectTaskSelectRow, ProjectTaskVirtualRow,
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
    /// 1 (highest) through 4 (lowest); empty clears it. Task-only, but — unlike
    /// `points`/`assigned_to_user_id` — always rendered, personal or team project,
    /// admin or not. See root CLAUDE.md's Priority section.
    priority: Option<String>,
    /// See `tasks::TaskForm`'s identical field for the redirect-vs-in-place-fragment
    /// rationale.
    redirect: Option<String>,
    /// "Depends on" (docs/issues_and_features.md) — a comma-joined string of sibling item
    /// ids, kept in sync with an unnamed checkbox group by the edit form's own `<script>`,
    /// same axum-0.6-Form-can't-deserialize-repeated-keys workaround as
    /// `project_item_series`'s `rotationUserIds` (see that module's doc comment). Only
    /// rendered on the dedicated edit page's form — every other `ProjectTaskForm` POST
    /// (reschedule dialog, quick assign, checkbox toggle, series occurrence) simply never
    /// includes this field, which `update_params_from_form` below treats as "leave
    /// dependencies unchanged," not "clear them."
    depends_on_item_ids: Option<String>,
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

/// The web UI's offset input is "days before the due date" — always non-negative from the
/// user's perspective — while stored `due_offset_days` keeps the negative-before/positive-after
/// convention described in CLAUDE.md's Recurrence section. Parsing as `u32` rejects a negative
/// input the same way an unparseable one is already silently dropped elsewhere in this form.
pub(crate) fn parse_days_before_due(s: &str) -> Option<i32> {
    s.parse::<u32>().ok().map(|d| -(d as i32))
}

fn overlay_days_before_due(form_value: &Option<String>, current: Option<i32>) -> Option<i32> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => parse_days_before_due(s.trim()).or(current),
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
            .and_then(parse_days_before_due),
        assigned_to_user_id: non_empty(&form.assigned_to_user_id),
        points: form
            .points
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        priority: form
            .priority
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
        due_offset_days: overlay_days_before_due(&form.due_offset_days, current.due_offset_days()),
        assigned_to_user_id: overlay_str(&form.assigned_to_user_id, current.assigned_to_user_id()),
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        // No points input renders on a non-admin's/personal-project's form — `overlay_i32`
        // falls back to `current.points` when absent, mirroring `team_tasks.rs`'s identical
        // comment: a plain edit here can't silently wipe it, and the service layer's own
        // admin gate is what actually decides whether a *changed* value is honored.
        points: overlay_i32(&form.points, current.points()),
        priority: overlay_i32(&form.priority, current.priority()),
        event_type: current.event_type(),
        depends_on_item_ids: form.depends_on_item_ids.as_deref().map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        }),
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

/// (id, name) of `item`'s still-incomplete "depends on" links, for the row's "Blocked by ..."
/// badge (`components/row.html`) — looks each dependency id up in `siblings` (dependencies are
/// always same-parent siblings, see `service::item_dependencies`, so every dependency of an item
/// in `siblings` is itself in `siblings`) and keeps only the ones not yet complete. `dep_map`
/// comes from a single batched `ItemDependencyRepo::list_for_items` call across the whole sibling
/// group, so this is just an in-memory lookup — no per-row query. Ids are carried alongside names
/// so `render_blocked_by` can link each name to its own detail page.
fn blocked_by_names_for(
    item: &Item,
    siblings: &[&Item],
    dep_map: &HashMap<String, Vec<String>>,
) -> Vec<(String, String)> {
    dep_map
        .get(&item.id)
        .into_iter()
        .flatten()
        .filter_map(|dep_id| siblings.iter().find(|s| &s.id == dep_id))
        .filter(|dep| !dep.complete)
        .map(|dep| (dep.id.clone(), dep.name.clone()))
        .collect()
}

/// Builds the three `Row` fields the "Blocked by ..." badge needs from `blocked_by_names_for`'s
/// (id, name) pairs: the plain-text names (`Row::blocked_by_names`, used for the emptiness check
/// and, joined, for the `title`-attribute label) and the pre-rendered anchor markup
/// (`Row::blocked_by_links_html`, each name linking to its own detail page) shown in the badge's
/// visible `lg:` label — see `components::row::BlockedByNames`'s doc comment for why the anchors
/// are rendered through a dedicated Askama template rather than inline in `row.html`.
fn render_blocked_by(
    project_id: &str,
    blocked_by: Vec<(String, String)>,
) -> Result<(Vec<String>, String, String), ItemError> {
    let names: Vec<String> = blocked_by.iter().map(|(_, name)| name.clone()).collect();
    let label = names.join(", ");
    let links_html = crate::web_ui::components::row::BlockedByNames {
        links: blocked_by
            .into_iter()
            .map(|(id, name)| crate::web_ui::components::row::BlockedByLink {
                name,
                url: format!("/web/projects/{project_id}/tasks/{id}"),
            })
            .collect(),
    }
    .render()?;
    Ok((names, label, links_html))
}

/// Recursively renders `parent_item_id`'s full descendant subtree as ready-to-insert `<li>`
/// markup, each descendant's own row in turn carrying its own nested `children_html` — the
/// in-place "expand to view sub-items" feature (see `Row::children_html`'s doc comment).
/// Everything is fetched and rendered eagerly, up front — one query per `has_children` node —
/// so the browser's expand/collapse toggle never round-trips to the server; the cost is paid on
/// every list load regardless of whether a branch is ever expanded. `pub(crate)` (not just this
/// module's own flat Tasks list) since the calendar screens (`project_calendar`/`main_calendar`)
/// reuse it unchanged rather than duplicating it — see their own `calendar_row` doc comments.
#[async_recursion]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn render_expandable_children(
    repo: &Arc<dyn ItemRepo>,
    parent_item_id: &str,
    project_id: &str,
    names: &HashMap<String, String>,
    show_complete: bool,
    tz: i32,
    skip_urls: &HashMap<String, String>,
    is_team_project: bool,
    depth: u8,
    // `None` only for the calendar/cross-project screens reusing this function (see this fn's
    // own doc comment) — the "Blocked by ..." badge is still project_tasks-only, and `None`
    // skips the extra batched dependency query entirely rather than paying for it just to
    // render nothing. Every project_tasks-owned caller (the flat list, and — via
    // `render_sibling_rows` below — the item detail page's Sub-items/Linked-tasks panels)
    // always passes `Some`.
    item_dependencies: Option<&Arc<dyn ItemDependencyRepo>>,
) -> Result<String, ItemError> {
    let children =
        list_project_items_unchecked(repo, project_id, Some(parent_item_id.to_string())).await?;
    let visible: Vec<&Item> = children
        .iter()
        .filter(|i| show_complete || !i.complete)
        .collect();
    let dep_map = match item_dependencies {
        Some(deps) => {
            deps.list_for_items(&visible.iter().map(|i| i.id.clone()).collect::<Vec<_>>())
                .await?
        }
        None => HashMap::new(),
    };
    let mut html = String::new();
    for i in &visible {
        let mut row = ProjectTaskRow::from_item(
            i,
            project_id,
            names,
            &visible,
            tz,
            skip_urls.get(&i.id).cloned(),
            is_team_project,
            show_complete,
            None,
            None,
        );
        row.indent_class = indent_class(depth);
        let (blocked_by_names, blocked_by_label, blocked_by_links_html) =
            render_blocked_by(project_id, blocked_by_names_for(i, &visible, &dep_map))?;
        row.blocked_by_names = blocked_by_names;
        row.blocked_by_label = blocked_by_label;
        row.blocked_by_links_html = blocked_by_links_html;
        row.expanded_row = row.expanded_row || !row.blocked_by_names.is_empty();
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
                    is_team_project,
                    depth + 1,
                    item_dependencies,
                )
                .await?,
            );
        }
        html.push_str(&row.render()?);
    }
    Ok(html)
}

/// Builds pre-rendered `Row` markup for a flat, non-recursive sibling group with no virtual
/// occurrences involved — the shape shared by the item detail page's Sub-items panel
/// (`handlers::render_children_fragment`, both its initial load and its "New sub-item"
/// create-refresh via `render_scope_fragment` below) and Events' Linked-tasks panel
/// (`handlers::render_source_event_fragment`). Introduced so those three call sites compute the
/// "Blocked by ..." badge (one batched `ItemDependencyRepo::list_for_items` query across
/// `visible`) and eagerly inline `children_html` for any row that itself `has_children` through
/// one shared implementation, rather than each hand-rolling its own copy of
/// `render_expandable_children`'s per-row body and risking exactly the kind of silent omission
/// that left them without the badge in the first place.
pub(crate) async fn render_sibling_rows(
    repo: &Arc<dyn ItemRepo>,
    visible: &[&Item],
    project_id: &str,
    names: &HashMap<String, String>,
    tz: i32,
    is_team_project: bool,
    show_complete: bool,
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
) -> Result<Vec<String>, ItemError> {
    let dep_map = item_dependencies
        .list_for_items(&visible.iter().map(|i| i.id.clone()).collect::<Vec<_>>())
        .await?;
    let mut rows = Vec::with_capacity(visible.len());
    for i in visible {
        let mut row = ProjectTaskRow::from_item(
            i,
            project_id,
            names,
            visible,
            tz,
            None,
            is_team_project,
            show_complete,
            None,
            None,
        );
        let (blocked_by_names, blocked_by_label, blocked_by_links_html) =
            render_blocked_by(project_id, blocked_by_names_for(i, visible, &dep_map))?;
        row.blocked_by_names = blocked_by_names;
        row.blocked_by_label = blocked_by_label;
        row.blocked_by_links_html = blocked_by_links_html;
        row.expanded_row = row.expanded_row || !row.blocked_by_names.is_empty();
        if i.has_children {
            row.children_html = Some(
                render_expandable_children(
                    repo,
                    &i.id,
                    project_id,
                    names,
                    show_complete,
                    tz,
                    &HashMap::new(),
                    is_team_project,
                    1,
                    Some(item_dependencies),
                )
                .await?,
            );
        }
        rows.push(row.render()?);
    }
    Ok(rows)
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
/// `matches` itself, so no separate handling is needed for those. `virtual_occurrences` itself
/// is expected to already be filtered through `ListFilters::matches_occurrence` by the caller
/// (`list_task_rows_for_project`) — this function doesn't re-filter it, same as it doesn't
/// re-filter `items` against anything beyond what's already true of `visible` below.
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
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
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
    // Top-level items are always siblings of each other (all share `parent_item_id: None`),
    // so one batched query covers every row's "Blocked by ..." badge — see
    // `blocked_by_names_for`'s doc comment.
    let dep_map = item_dependencies
        .list_for_items(&visible.iter().map(|i| i.id.clone()).collect::<Vec<_>>())
        .await?;
    let mut entries: Vec<((i32, i64), String)> = Vec::with_capacity(visible.len());
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
        let (blocked_by_names, blocked_by_label, blocked_by_links_html) =
            render_blocked_by(project_id, blocked_by_names_for(i, &visible, &dep_map))?;
        row.blocked_by_names = blocked_by_names;
        row.blocked_by_label = blocked_by_label;
        row.blocked_by_links_html = blocked_by_links_html;
        row.expanded_row = row.expanded_row || !row.blocked_by_names.is_empty();
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
                    team_id.is_some(),
                    1,
                    Some(item_dependencies),
                )
                .await?,
            );
        }
        entries.push((sort_key(i), row.render()?));
    }
    if filters.recurring {
        for occ in virtual_occurrences {
            entries.push((
                (priority_rank(occ.priority), occ.occurrence_date.timestamp()),
                ProjectTaskVirtualRow::from_occurrence(occ, project_id, tz, filters, in_list_view)
                    .render()?,
            ));
        }
    }
    entries.sort_by_key(|(key, _)| *key);
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
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
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
    let now = Utc::now();
    let is_team_project = team_id.is_some();
    let virtual_occurrences: Vec<_> = if filters.recurring {
        item_series_service::list_occurrence_states_for_project(
            series, users, project_id, now, now, tz,
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
        // See `render_rows_with_virtual`'s doc comment: this is `filters.matches`' own
        // occurrence-shaped counterpart — without it, a filter that would exclude a real item
        // (e.g. "assigned to Bob") left its series' current occurrence showing regardless.
        .filter(|occ| filters.matches_occurrence(occ, requester_user_id, is_team_project, now))
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
        item_dependencies,
    )
    .await
}

/// A select-mode row (docs/issues_and_features.md's "Multi-select" item) — see
/// `templates::ProjectTaskSelectRow`'s doc comment for why this is a wholly separate, minimal
/// rendering rather than another `ProjectTaskRow`/`Row` variant. Deliberately top-level-only —
/// no sub-items are rendered or fetched here at all: showing a sub-item's row nested inside its
/// parent's meant highlighting the parent (the only element a click can actually toggle) looked
/// like it highlighted the child too, since the parent `<li>`'s own background paints behind
/// its nested content. Selecting a task's own sub-items is meant to happen from a select mode
/// scoped to that one task's page instead (not yet built) — this top-level list only ever
/// selects top-level tasks.
fn render_select_row(item: &Item, tz: i32) -> Result<String, ItemError> {
    ProjectTaskSelectRow {
        id: item.id.clone(),
        name: item.name.clone(),
        complete: item.complete,
        due_date: item.due_date().map(|d| {
            crate::web_ui::format_display_date(crate::web_ui::to_local(d, tz), item.has_due_time())
        }),
        overdue: item.is_overdue(Utc::now()),
    }
    .render()
    .map_err(ItemError::from)
}

/// Top-level select-mode rows for `items` (the project's Tasks — same `filters` the normal list
/// view applies, so a user can still narrow down before selecting; virtual/series occurrences
/// are excluded entirely, since they have no real item id to select yet).
pub(crate) fn render_select_rows(
    items: &[Item],
    filters: &ListFilters,
    requester_user_id: &str,
    is_team_project: bool,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    let now = Utc::now();
    items
        .iter()
        .filter(|item| filters.matches(item, requester_user_id, is_team_project, now))
        .map(|item| render_select_row(item, tz))
        .collect()
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

/// 1 sorts first (highest priority), 4 next, unset last — matches
/// `storage::sqlite::items`'s own `COALESCE(priority, 5)` SQL ordering (root CLAUDE.md's
/// Priority section).
fn priority_rank(priority: Option<i32>) -> i32 {
    priority.unwrap_or(5)
}

/// `list_project_items_unchecked` already scopes to top-level, non-Template items — this narrows
/// further to `Task` and sorts by priority first, then due date (undated tasks last), mirroring
/// `tasks::list_tasks`/`team_tasks::list_team_tasks`'s original due-date-only precedent.
fn sort_key(item: &Item) -> (i32, i64) {
    (
        priority_rank(item.priority()),
        item.due_date().map(|d| d.timestamp()).unwrap_or(i64::MAX),
    )
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
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
) -> Result<Html<String>, ItemError> {
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    // The `Some(parent_id)` branch is this same Sub-items panel's own create-refresh (the
    // detail page's "New sub-item"/"Add multiple at once" forms both target `#children-list`
    // via this function) — it needs `render_sibling_rows`' badge/children_html treatment for
    // the exact reason `render_children_fragment`'s initial load does, or the badge would
    // flicker away the moment a sub-item is added. The `None` (flat top-level list) branch is a
    // different screen's own create-refresh and keeps its existing `render_rows` behavior.
    let (rows, empty_message) = if let Some(parent_id) = parent_item_id {
        let items =
            list_project_items_unchecked(repo, project_id, Some(parent_id.to_string())).await?;
        let visible: Vec<&Item> = items.iter().collect();
        (
            render_sibling_rows(
                repo,
                &visible,
                project_id,
                &names,
                tz,
                team_id.is_some(),
                true,
                item_dependencies,
            )
            .await?,
            "No sub-items yet.",
        )
    } else {
        let items = list_project_tasks(repo, project_id).await?;
        (
            render_rows(
                &items,
                project_id,
                &names,
                show_complete,
                tz,
                &HashMap::new(),
                team_id,
            )?,
            "No tasks yet.",
        )
    };
    render(ProjectTaskRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::ItemType;

    fn task(id: &str, name: &str, complete: bool) -> Item {
        Item {
            id: id.to_string(),
            name: name.to_string(),
            complete,
            ..Item::default()
        }
    }

    #[test]
    fn blocked_by_names_for_includes_only_incomplete_dependencies() {
        let a = task("a", "Design", false);
        let b = task("b", "Build", false);
        let c = task("c", "Ship", false);
        let siblings: Vec<&Item> = vec![&a, &b, &c];
        let mut dep_map = HashMap::new();
        dep_map.insert("c".to_string(), vec!["a".to_string(), "b".to_string()]);

        assert_eq!(
            blocked_by_names_for(&c, &siblings, &dep_map),
            vec![
                ("a".to_string(), "Design".to_string()),
                ("b".to_string(), "Build".to_string())
            ]
        );
    }

    #[test]
    fn blocked_by_names_for_omits_a_completed_dependency() {
        let a = task("a", "Design", true);
        let b = task("b", "Ship", false);
        let siblings: Vec<&Item> = vec![&a, &b];
        let mut dep_map = HashMap::new();
        dep_map.insert("b".to_string(), vec!["a".to_string()]);

        assert!(blocked_by_names_for(&b, &siblings, &dep_map).is_empty());
    }

    #[test]
    fn blocked_by_names_for_is_empty_with_no_dependency_edges() {
        let a = task("a", "Solo", false);
        let siblings: Vec<&Item> = vec![&a];
        assert!(blocked_by_names_for(&a, &siblings, &HashMap::new()).is_empty());
    }

    fn set_priority(item: &mut Item, priority: i32) {
        if let ItemType::Task { priority: p, .. } = &mut item.item_type {
            *p = Some(priority);
        } else {
            panic!("test item must be a Task");
        }
    }

    fn set_due_date(item: &mut Item, secs: i64) {
        item.item_type
            .schedule_mut()
            .expect("has schedule")
            .due_date = DateTime::from_timestamp(secs, 0);
    }

    /// Priority sorts first (1 highest ... 4 lowest, unset last); due date only breaks
    /// ties within the same priority rank. See root CLAUDE.md's Priority section.
    #[test]
    fn sort_key_orders_by_priority_then_due_date() {
        let mut low_priority_early_due = task("a", "A", false);
        set_priority(&mut low_priority_early_due, 4);
        set_due_date(&mut low_priority_early_due, 1_000);

        let mut high_priority_late_due = task("b", "B", false);
        set_priority(&mut high_priority_late_due, 1);
        set_due_date(&mut high_priority_late_due, 9_000);

        let mut no_priority = task("c", "C", false);
        set_due_date(&mut no_priority, 500);

        let mut same_priority_earlier_due = task("d", "D", false);
        set_priority(&mut same_priority_earlier_due, 1);
        set_due_date(&mut same_priority_earlier_due, 2_000);

        let mut items = vec![
            low_priority_early_due,
            high_priority_late_due,
            no_priority,
            same_priority_earlier_due,
        ];
        items.sort_by_key(sort_key);

        assert_eq!(
            items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["d", "b", "a", "c"]
        );
    }
}
