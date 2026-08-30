use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::attachment::Attachment;
use crate::domain::comment::Comment;
use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::service::item_series::ProjectOccurrence;
use crate::storage::sqlite::ItemRepo;
use crate::web_ui::components::row::Row;
use crate::web_ui::{format_display_date, reminder_labels, to_local};
use askama::Template;
use chrono::Utc;

// ---- templates --------------------------------------------------------------

pub fn offset_label_for(item: &Item) -> Option<String> {
    if !item.is_offset_driven() {
        return None;
    }
    match item.due_offset_days() {
        Some(0) => Some("on due date".to_string()),
        Some(n) if n > 0 => Some(format!("+{n}d")),
        Some(n) => Some(format!("{n}d")),
        None => None,
    }
}

/// The web UI presents this as "days before the due date" — non-negative from the user's
/// perspective — while `due_offset_days` itself keeps the negative-before/positive-after
/// convention described in CLAUDE.md's Recurrence section. See `parse_days_before_due` for the
/// inverse (form input -> stored value).
pub fn format_offset_input(due_offset_days: Option<i32>) -> String {
    due_offset_days
        .map(|d| (-d).to_string())
        .unwrap_or_default()
}

pub fn format_points_input(points: Option<i32>) -> String {
    points.map(|p| p.to_string()).unwrap_or_default()
}

/// Read-only display label for a Task's `priority` (row detail line, dialog, full page) — see
/// root CLAUDE.md's Priority section. `None` when unset, matching `offset_label_for`'s
/// "absent field renders nothing" convention.
pub fn priority_label_for(priority: Option<i32>) -> Option<String> {
    priority.map(|p| format!("P{p}"))
}

/// `Row`'s first real caller — see `docs/project-abstraction-plan.md` stage B5a. Builds a
/// generic `components::row::Row` rather than a Task-specific template of its own.
pub struct ProjectTaskRow;

impl ProjectTaskRow {
    #[allow(clippy::too_many_arguments)]
    pub fn from_item(
        item: &Item,
        project_id: &str,
        names: &HashMap<String, String>,
        siblings: &[&Item],
        tz: i32,
        skip_url: Option<String>,
        is_team_project: bool,
        show_complete: bool,
        confirmation: Option<String>,
        dismiss_after_ms: Option<u32>,
        // `None` when `item.series_id` is unset; `Some(is_current)` otherwise — see
        // `Row::series_current`'s doc comment. Only `render_rows_with_virtual` (the flat Tasks
        // list's own merge, which already runs the query this comes from) passes a real value;
        // every other caller of `from_item` passes `None`, leaving that row's series badge
        // absent until it too threads a `current_flags` map through.
        series_current: Option<bool>,
    ) -> Row {
        let offset_label = offset_label_for(item);
        let assignee_name = item
            .assigned_to_user_id()
            .map(|id| names.get(&id).cloned().unwrap_or(id));
        let priority_label = priority_label_for(item.priority());
        Row {
            id: item.id.clone(),
            item_url: format!("/web/projects/{project_id}/tasks/{}", item.id),
            name: item.name.clone(),
            complete: item.complete(),
            due_date: item
                .due_date()
                .map(|d| format_display_date(to_local(d, tz), item.has_due_time())),
            overdue: item.is_overdue(Utc::now()),
            scheduled_date: item
                .scheduled_date()
                .map(|d| format_display_date(to_local(d, tz), item.has_scheduled_time())),
            scheduled_end_date: item
                .scheduled_end_date()
                .map(|d| format_display_date(to_local(d, tz), item.has_end_time())),
            event_type: item.event_type(),
            expanded_row: item.due_date().is_some()
                || item.scheduled_date().is_some()
                || item.due_offset_days().is_some()
                || offset_label.is_some()
                || assignee_name.is_some()
                || priority_label.is_some(),
            has_children: item.has_children,
            offset_label,
            assignee_name,
            priority_label,
            series_current,
            complete_url: Some(format!("/web/projects/{project_id}/tasks/{}", item.id)),
            edit_url: (!item.complete())
                .then(|| format!("/web/projects/{project_id}/tasks/{}/edit", item.id)),
            duplicate_url: Some(format!(
                "/web/projects/{project_id}/tasks/{}/duplicate",
                item.id
            )),
            add_child_url: Some(format!(
                "/web/projects/{project_id}/tasks/{}/add-child",
                item.id
            )),
            save_as_template_url: Some(format!(
                "/web/projects/{project_id}/tasks/{}/save-as-template",
                item.id
            )),
            // Events-only (see `Row::add_linked_task_url`'s doc comment) — a Task uses
            // `add_child_url` instead.
            add_linked_task_url: None,
            move_url: (item.source_event_id().is_none()
                && (item.parent_item_id().is_some() || siblings.iter().any(|s| s.id != item.id)))
            .then(|| format!("/web/projects/{project_id}/tasks/{}/move", item.id)),
            reschedule_url: Some(format!(
                "/web/projects/{project_id}/tasks/{}/reschedule",
                item.id
            )),
            assign_url: is_team_project
                .then(|| format!("/web/projects/{project_id}/tasks/{}/assign", item.id)),
            skip_url,
            toggle_complete_json: (!item.complete()).to_string(),
            show_complete,
            confirmation,
            dismiss_after_ms,
            // Only an Event can ever be Google-Calendar-imported (see CLAUDE.md's Points/
            // Recurrence sections — imported items are always `ItemType::Event`).
            is_imported: false,
            // Calendar-only fields — see `Row`'s doc comments. The calendar screens build a
            // `Row` via this same `from_item` and then override these themselves.
            type_badge: None,
            parent_name: None,
            project_name: None,
            // Stage 1 of docs/dialog-item-forms-plan.md — project_tasks is the proof-of-concept
            // screen whose detail/edit/new pages were converted into dialog fragments.
            detail_via_dialog: true,
            // The in-place expand feature is opt-in per render call, not per item — the flat
            // Tasks list's own row-building (`render_rows_with_virtual`/`indent_class`/
            // `render_expandable_children` in `project_tasks/mod.rs`) overrides these two on top
            // of this base row when it wants a given row expandable, as does the item detail
            // page's own Sub-items/linked-tasks panels (`handlers::render_children_fragment`/
            // `render_source_event_fragment`) for any child that itself `has_children`; every
            // other caller of `from_item` (calendar screens, save-as-template, etc.) leaves
            // these at this default and keeps the plain decorative `has_children` arrow.
            children_html: None,
            indent_class: "",
            // Set by `blocked_by_names_for`-aware callers only (the flat Tasks list and its
            // in-place-expanded children — see `project_tasks::blocked_by_names_for`'s doc
            // comment); every other caller of `from_item` leaves this empty.
            blocked_by_names: Vec::new(),
            blocked_by_label: String::new(),
            blocked_by_links_html: String::new(),
        }
    }
}

/// A select-mode row (docs/issues_and_features.md's "Multi-select" item) — rendered only when
/// the Tasks list is loaded with `?select=1` (`handlers::project_tasks_page`,
/// `project_tasks::render_select_row`), completely independent of `Row`/`ProjectTaskRow` above.
/// Deliberately minimal: a single native `<label>` wraps the checkbox and the name, so "click
/// anywhere on the row toggles selection" is a free native browser behavior rather than
/// something JS has to reimplement — see base.html's `toggleBatchSelection` doc comment for
/// why an earlier version of this (disabling the normal row's own checkbox/menu/navigation in
/// place, via CSS `pointer-events` + a click-delegation listener) didn't work reliably.
///
/// Top-level tasks only — no sub-items, no `indent_class`/`children_html` (see
/// `project_tasks::render_select_row`'s doc comment for why nesting a sub-item's row inside its
/// parent's made selecting the parent look like it selected the child too).
#[derive(Template)]
#[template(path = "project_tasks/select_row.html")]
pub struct ProjectTaskSelectRow {
    pub id: String,
    pub name: String,
    pub complete: bool,
    pub due_date: Option<String>,
    pub overdue: bool,
}

/// A lightweight date/schedule-only editor, opened from a task row's calendar-icon button —
/// see `docs/archived/archived_issues_and_features.md`'s "quick reschedule" entry. Deliberately reuses the same PUT
/// `/web/projects/:project_id/tasks/:item_id` endpoint (`handlers::update_project_task_form`)
/// the full edit form already saves to, rather than introducing a second save path: the field
/// names below (`dueDate`/`dueTime`/`scheduledDate`/... via `macros::due_date_fields`/
/// `scheduled_fields`) match `ProjectTaskForm` exactly, so every field this dialog omits
/// (name, assignment, points, ...) round-trips from `current` unchanged via that handler's
/// existing overlay helpers — no separate validation/side-effect logic to duplicate.
#[derive(Template)]
#[template(path = "components/reschedule_dialog.html")]
pub struct RescheduleDialog {
    pub item_id: String,
    pub post_reschedule_url: String,
    pub scheduled_start_date: String,
    pub scheduled_start_time: String,
    pub scheduled_end_date: String,
    pub scheduled_end_time: String,
    pub due_date: String,
    pub due_time: String,
}

impl RescheduleDialog {
    /// `view`: `Some("project-calendar"/"main-calendar")` when this dialog was opened from a
    /// calendar row (see `RowViewQuery`) — carried onto `post_reschedule_url` so the save PUT
    /// tells `update_project_task_form` to re-render via that screen's `calendar_row` overlay.
    pub fn from_task(task: &Item, project_id: &str, tz: i32, view: Option<&str>) -> Self {
        let local_due_date = task.due_date().map(|d| to_local(d, tz));
        let local_scheduled_date = task.scheduled_date().map(|d| to_local(d, tz));
        let local_scheduled_end_date = task.scheduled_end_date().map(|d| to_local(d, tz));
        let post_reschedule_url = format!("/web/projects/{project_id}/tasks/{}", task.id);
        let post_reschedule_url = match view {
            Some(v) => format!("{post_reschedule_url}?view={v}"),
            None => post_reschedule_url,
        };
        RescheduleDialog {
            item_id: task.id.clone(),
            post_reschedule_url,
            due_date: local_due_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            due_time: if task.has_due_time() {
                local_due_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            scheduled_start_date: local_scheduled_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_start_time: if task.has_scheduled_time() {
                local_scheduled_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            scheduled_end_date: local_scheduled_end_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_end_time: if task.has_end_time() {
                local_scheduled_end_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
        }
    }
}

/// The offset-driven counterpart to `RescheduleDialog` — opened instead of it (see
/// `handlers::get_reschedule_task`) when the task `is_offset_driven()` (a sub-item with a
/// `parent_item_id`, or a task linked to an Event via `source_event_id`): such a task's own
/// `due_date` is never editable directly, only via its days-before-due offset (see CLAUDE.md's
/// Recurrence section), so the due-date fields are omitted here (read-only display lives on the
/// full edit form instead). `scheduled_date`/`scheduled_end_date` are unaffected by the offset —
/// `due_offset_days` only ever recomputes `due_date` (`Item::deadline_from_offset`), never the
/// scheduled window — so they're independently settable here exactly like `RescheduleDialog`'s.
/// Reuses the same PUT this screen's other dialogs do — `update_params_from_form`'s overlay
/// helpers leave every field this dialog omits (name, assignment, points, ...) unchanged, and
/// `service::items::update_item` recomputes `due_date` from the new offset itself
/// (`resolve_offset_anchor` + `deadline_from_offset`), so there's no separate save path to
/// duplicate here either.
#[derive(Template)]
#[template(path = "components/offset_reschedule_dialog.html")]
pub struct OffsetRescheduleDialog {
    pub item_id: String,
    pub post_reschedule_url: String,
    pub due_offset_days_input: String,
    /// See `ProjectTaskDetailFields::anchor_date_input`'s identical doc comment — the parent's
    /// (or linked event's) own due/scheduled date, not this item's own, so the JS preview in
    /// `macros::due_offset_days_field` works even before this item has ever had an offset set.
    pub anchor_date_input: String,
    pub scheduled_start_date: String,
    pub scheduled_start_time: String,
    pub scheduled_end_date: String,
    pub scheduled_end_time: String,
}

impl OffsetRescheduleDialog {
    /// `anchor_date`: this task's resolved offset anchor (`service::items::resolve_offset_anchor`)
    /// — an async repo lookup the caller must already have made, mirroring
    /// `ProjectTaskDetailFields::from_item`'s identical `anchor_date` parameter. `view`: see
    /// `RescheduleDialog::from_task`'s identical rationale.
    pub fn from_task(
        task: &Item,
        project_id: &str,
        tz: i32,
        anchor_date: Option<chrono::DateTime<Utc>>,
        view: Option<&str>,
    ) -> Self {
        let local_scheduled_date = task.scheduled_date().map(|d| to_local(d, tz));
        let local_scheduled_end_date = task.scheduled_end_date().map(|d| to_local(d, tz));
        let post_reschedule_url = format!("/web/projects/{project_id}/tasks/{}", task.id);
        let post_reschedule_url = match view {
            Some(v) => format!("{post_reschedule_url}?view={v}"),
            None => post_reschedule_url,
        };
        OffsetRescheduleDialog {
            item_id: task.id.clone(),
            post_reschedule_url,
            due_offset_days_input: format_offset_input(task.due_offset_days()),
            anchor_date_input: anchor_date
                .map(|d| to_local(d, tz).format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_start_date: local_scheduled_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_start_time: if task.has_scheduled_time() {
                local_scheduled_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            scheduled_end_date: local_scheduled_end_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_end_time: if task.has_end_time() {
                local_scheduled_end_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
        }
    }
}

/// A lightweight assignee-only editor, opened from a task row's person-icon button — mirrors
/// `RescheduleDialog` exactly (see its doc comment for the rationale): reuses the same PUT
/// `/web/projects/:project_id/tasks/:item_id` endpoint (`handlers::update_project_task_form`),
/// so every field this dialog omits round-trips from `current` unchanged via that handler's
/// existing overlay helpers. Only ever built for a team-backed project — `assign_url` (see
/// `components::row::Row`) is `None` on a personal project, so this dialog's route is never
/// reached without one.
#[derive(Template)]
#[template(path = "components/quick_assign_dialog.html")]
pub struct QuickAssignDialog {
    pub item_id: String,
    pub post_assign_url: String,
    pub assignee_options: Vec<(String, String)>,
    pub assigned_to_user_id: Option<String>,
}

impl QuickAssignDialog {
    /// `view`: see `RescheduleDialog::from_task`'s identical rationale.
    pub fn from_task(
        task: &Item,
        project_id: &str,
        assignee_options: Vec<(String, String)>,
        view: Option<&str>,
    ) -> Self {
        let post_assign_url = format!("/web/projects/{project_id}/tasks/{}", task.id);
        let post_assign_url = match view {
            Some(v) => format!("{post_assign_url}?view={v}"),
            None => post_assign_url,
        };
        QuickAssignDialog {
            item_id: task.id.clone(),
            post_assign_url,
            assignee_options,
            assigned_to_user_id: task.assigned_to_user_id(),
        }
    }
}

/// Opened from a task row's "Add sub-item" row-action — a bare name-only create form, posting
/// straight to the same POST `/web/projects/:project_id/tasks` endpoint the standalone "+ New
/// Task" page and this item's own detail-page "New sub-item" form already use
/// (`create_project_task_form`), with `parentItemId` pre-filled to this row's own id and
/// `redirect=1` so a successful save is a full `HX-Redirect` — targeting `return_to` (whatever
/// page the dialog was actually opened from — the Tasks list, or a task's own detail page, if
/// this row was one of its Sub-items) rather than unconditionally the Tasks list, fixing
/// docs/issues_and_features.md's "adding a sub-item redirects to the main Tasks list instead of
/// back to the item's own full/detail page" item. `Row::children_html`'s doc comment explains
/// why this always has to be a full redirect rather than a narrower in-place swap: a leaf row
/// has no `#item-{id}-children` container to swap into.
#[derive(Template)]
#[template(path = "components/add_child_dialog.html")]
pub struct AddChildDialog {
    pub parent_item_id: String,
    pub parent_name: String,
    pub post_create_url: String,
    /// Migrated from the retired full detail page's own "Add multiple at once" section — see
    /// `handlers::create_project_tasks_batch`, which already accepted `parentItemId`/`redirect`
    /// (the detail page posted to it too, just with a narrower `#children-list` target instead
    /// of this dialog's full-redirect close).
    pub post_batch_url: String,
    /// Where the create/batch-create forms' `redirect=1` should send the client back to —
    /// seeded from the `HX-Current-URL` header on the dialog's own GET (`handlers::
    /// get_add_child_task`), so it reflects whatever page was actually open underneath when
    /// "Add sub-item" was clicked. Falls back to this project's Tasks list when that header is
    /// absent (a non-htmx request) or fails `sanitize_return_to`'s same-project check.
    pub return_to: String,
}

impl AddChildDialog {
    pub fn new(parent: &Item, project_id: &str, current_url: Option<&str>) -> Self {
        let return_to = super::sanitize_return_to(current_url, project_id)
            .unwrap_or_else(|| format!("/web/projects/{project_id}/tasks"));
        AddChildDialog {
            parent_item_id: parent.id.clone(),
            parent_name: parent.name.clone(),
            post_create_url: format!("/web/projects/{project_id}/tasks"),
            post_batch_url: format!("/web/projects/{project_id}/tasks/batch"),
            return_to,
        }
    }
}

/// Sentinel `target` value meaning "promote" — reparent onto the item's own grandparent — as
/// opposed to every other `<option>` value in `MoveDialog`, which is a sibling's own id meaning
/// "subordinate under this sibling". Never collides with a real item id.
pub const MOVE_TARGET_PARENT: &str = "up";

/// Opened from a task row's "Move" row-action (or the detail view's own "Move" button) — unifies
/// what used to be two separate actions ("promote to sibling of parent" and "subordinate to
/// sibling") into a single picker: the item's current parent (if any, listed first and marked
/// "(parent)") plus every current sibling. Picking the parent entry promotes; picking a sibling
/// subordinates the item under it. See `handlers::move_project_task_form`'s dispatch on
/// `MOVE_TARGET_PARENT` and `CLAUDE.md`'s reparent-actions note.
#[derive(Template)]
#[template(path = "components/move_dialog.html")]
pub struct MoveDialog {
    pub item_name: String,
    pub post_move_url: String,
    /// (target value, label, is_parent) in display order — the parent entry (if any) always
    /// first. `target value` is `MOVE_TARGET_PARENT` for the parent entry, a sibling's own id
    /// otherwise.
    pub options: Vec<(String, String, bool)>,
}

impl MoveDialog {
    pub fn new(item: &Item, parent: Option<&Item>, siblings: &[Item], project_id: &str) -> Self {
        let mut options: Vec<(String, String, bool)> = Vec::new();
        if let Some(parent) = parent {
            options.push((MOVE_TARGET_PARENT.to_string(), parent.name.clone(), true));
        }
        options.extend(
            siblings
                .iter()
                .filter(|s| s.id != item.id)
                .map(|s| (s.id.clone(), s.name.clone(), false)),
        );
        MoveDialog {
            item_name: item.name.clone(),
            post_move_url: format!("/web/projects/{project_id}/tasks/{}/move", item.id),
            options,
        }
    }
}

/// Stage 10 gap 2: a Task series' `current_occurrence_date`, rendered as a distinct virtual
/// row in the flat `/tasks` list — mirrors `project_calendar::ProjectCalendarVirtualRow` and
/// the Tasks calendar's `CalendarVirtualTaskEntry`, minus the type badge (this list is
/// Task-only already). Every row built from this struct is current by construction (callers
/// filter for `is_current` before constructing one), so the template has no "Planned" branch.
#[derive(Template)]
#[template(path = "project_tasks/virtual_row.html")]
pub struct ProjectTaskVirtualRow {
    pub series_id: String,
    pub occurrence_ts: i64,
    /// `"{series_id}-{occurrence_ts}"` — this row's own id for `openRowActionsMenu()`'s
    /// `#row-actions-menu-{row_id}` lookup (see `components/row.html`'s identical convention,
    /// keyed there off a real item id instead). Kept as a separate precomputed field rather than
    /// formatting it inline in the template, matching `list_query`'s own precomputed-in-Rust
    /// convention below.
    pub row_id: String,
    pub name: String,
    pub date_label: String,
    /// Mirrors `ProjectTaskRow`'s 💀-vs-📅 date icon choice — see
    /// `ProjectOccurrence::is_due_date_basis`'s doc comment.
    pub is_due_date_basis: bool,
    /// Same meaning as `ProjectTaskRow::overdue` — `false` for a scheduled-date-basis series
    /// (scheduled dates are never styled overdue, matching a materialized row).
    pub overdue: bool,
    pub materialize_url: String,
    pub skip_url: String,
    pub complete_url: String,
    pub is_current: bool,
    /// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — `true` when this
    /// occurrence has been explicitly skipped (`OccurrenceState::Skipped`), in which case the
    /// template shows a struck-through name + "Skipped" label + Unskip button instead of the
    /// materialize link/Skip button.
    pub is_skipped: bool,
    pub unskip_url: String,
    /// `GET`s the materialize-on-save edit form directly, skipping the Details detour — see
    /// `ProjectOccurrence::edit_url`'s doc comment. Rendered only when `!is_skipped`.
    pub edit_url: String,
    /// `GET`s the materialize-on-save "Add sub-item" dialog — see
    /// `ProjectOccurrence::add_task_child_url`'s doc comment. Rendered only when `!is_skipped`.
    pub add_child_url: String,
    /// Populated the same way `ProjectTaskRow::assignee_name` is — previously dropped despite
    /// `ProjectOccurrence` already carrying it, so a virtual occurrence's row silently showed no
    /// assignee until materialized. See docs/issues_and_features.md's "occurrences don't show
    /// the assignee unless it's materialized" item.
    pub assignee_name: Option<String>,
    /// See `Row::priority_label`'s identical doc comment — sourced from `ProjectOccurrence::
    /// priority` (the series' own priority) rather than a materialized item's, since this row is
    /// only ever built for a still-virtual/skipped occurrence.
    pub priority_label: Option<String>,
    /// True only when this row is being rendered for the flat Tasks list (`handlers::
    /// project_tasks_page`/`list_task_rows_for_project`), never the calendar day panel
    /// (`day_list_rows`) — gates whether the checkbox/Skip/Unskip carry `hx-target="#items-list"`
    /// at all. The calendar day panel has no such container in its DOM, so those same buttons
    /// there fall back to the pre-existing default (whole-page) htmx behavior rather than
    /// silently targeting an id that doesn't exist. See the archived "extend confirm-then-fade
    /// to virtual occurrences" entry (2026-08-21) for why only the flat list got this treatment.
    pub in_list_view: bool,
}

impl ProjectTaskVirtualRow {
    pub fn from_occurrence(
        occ: &ProjectOccurrence,
        project_id: &str,
        tz: i32,
        filters: &crate::web_ui::list_filters::ListFilters,
        in_list_view: bool,
    ) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        // Baked into the URL itself (rather than relying on `hx-vals`, which `Row`'s checkbox
        // uses) so a single query-string suffix covers the checkbox and both Skip/Unskip
        // buttons identically — see `list_task_rows_for_project`'s callers (and
        // `OccurrenceRowActionQuery` in both `handlers.rs` and
        // `project_item_series::handlers`), which read these same params back off the request.
        // Stage 2 of docs/list-filtering-plan.md: carries the full active filter set, not just
        // `showComplete`.
        let list_query = if in_list_view {
            let suffix = filters.query_string();
            if suffix.is_empty() {
                "?view=tasks-list".to_string()
            } else {
                format!("?view=tasks-list&{suffix}")
            }
        } else {
            String::new()
        };
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            row_id: format!("{}-{}", occ.series_id, occ.occurrence_date.timestamp()),
            name: occ.series_name.clone(),
            date_label: format_display_date(local, true),
            is_due_date_basis: occ.is_due_date_basis,
            overdue: occ.is_due_date_basis && occ.occurrence_date < Utc::now(),
            materialize_url: occ.materialize_url(project_id),
            skip_url: format!("{}{list_query}", occ.skip_url(project_id)),
            complete_url: format!("{}{list_query}", occ.complete_url(project_id)),
            is_current: occ.is_current,
            is_skipped: occ.is_skipped(),
            unskip_url: format!("{}{list_query}", occ.unskip_url(project_id)),
            edit_url: occ.edit_url(project_id),
            add_child_url: occ.add_task_child_url(project_id),
            assignee_name: occ.assigned_to_user_name.clone(),
            priority_label: priority_label_for(occ.priority),
            in_list_view,
        }
    }
}

#[derive(Template)]
#[template(path = "project_tasks/detail_fields.html")]
pub struct ProjectTaskDetailFields {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub complete: bool,
    pub is_top_level: bool,
    /// See `tasks::TaskDetailFields`'s identical field.
    pub is_offset_driven: bool,
    pub due_date_input: String,
    pub due_time_input: String,
    pub scheduled_date_input: String,
    pub scheduled_time_input: String,
    pub scheduled_end_date_input: String,
    pub scheduled_end_time_input: String,
    pub due_offset_days_input: String,
    /// This item's true offset anchor — the parent's (or linked event's) own due/scheduled
    /// date, resolved via `service::items::resolve_offset_anchor` — formatted `YYYY-MM-DD` in
    /// the viewer's local timezone, or empty if the item isn't offset-driven or the anchor has
    /// no date of its own. Fed to `macros::due_offset_days_field`'s JS preview instead of this
    /// item's own (possibly not-yet-computed) `due_date_input`, so the preview works even when
    /// no offset has been set yet — see docs/issues_and_features.md's "Helper js on edit task
    /// for sub-items" entry.
    pub anchor_date_input: String,
    /// True for a team-backed project — gates the "Assign to"/points markup, which never
    /// renders at all on a personal project (not just hidden — see
    /// `docs/project-abstraction-plan.md` stage B5a's note on why points stays
    /// team-membership-gated rather than a new `ProjectRepo`-native concept).
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub assigned_to_user_id: Option<String>,
    /// Gates the admin-only `points` input — see `macros::points_field` and
    /// `service::teams::require_team_admin`, whose result populates this at render time.
    pub is_team_admin: bool,
    pub points_input: String,
    /// 1–4, or empty for "no priority" — unlike `points_input`, always rendered
    /// regardless of `is_team_admin`/`is_team_project`. See `macros::priority_field`.
    pub priority_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    pub just_saved: bool,
    /// True only when this fragment was reached via the item's own full detail page's Edit
    /// link (`?redirect=1` on `GET .../edit`, see `project_task_edit_page`) rather than a list
    /// row's "⋮ Edit"/detail-dialog Edit button. There's no `#item-{id}` row to target/select
    /// on a full detail page (only `#item-{id}-view` inside it) and the whole page needs its
    /// header/view refreshed on save, not just a row — see `detail_fields.html`'s own comment.
    /// Always `false` for the post-save row+fields+view fragment (that branch is only reached
    /// when `redirect` was absent, i.e. the list-row case).
    pub via_full_page: bool,
    /// "Depends on" (docs/issues_and_features.md) — every sibling Task this item could depend
    /// on (same project, same `parent_item_id`, excluding itself), for the checkbox picker.
    /// Empty for a Simple/Event/Template item — `require_task` already keeps this screen
    /// Task-only, but `depends_on_options` staying empty is also just correct on its own: a
    /// non-Task item is never a valid dependency target anyway (see
    /// `service::item_dependencies::set_item_dependencies`).
    pub depends_on_options: Vec<(String, String)>,
    /// The ids currently selected among `depends_on_options`, for pre-checking the picker.
    pub depends_on_item_ids: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
impl ProjectTaskDetailFields {
    pub fn from_item(
        item: &Item,
        project_id: &str,
        is_team_project: bool,
        assignee_options: Vec<(String, String)>,
        is_team_admin: bool,
        tz: i32,
        just_saved: bool,
        via_full_page: bool,
        depends_on_options: Vec<(String, String)>,
        depends_on_item_ids: Vec<String>,
        anchor_date: Option<chrono::DateTime<Utc>>,
    ) -> Self {
        let local_due_date = item.due_date().map(|d| to_local(d, tz));
        let due_date_input = local_due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time() {
            local_due_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_date = item.scheduled_date().map(|d| to_local(d, tz));
        let scheduled_date_input = local_scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_time_input = if item.has_scheduled_time() {
            local_scheduled_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_end_date = item.scheduled_end_date().map(|d| to_local(d, tz));
        let scheduled_end_date_input = local_scheduled_end_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_end_time_input = if item.has_end_time() {
            local_scheduled_end_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            complete: item.complete(),
            is_top_level: item.parent_item_id().is_none(),
            is_offset_driven: item.is_offset_driven(),
            due_date_input,
            due_time_input,
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            due_offset_days_input: format_offset_input(item.due_offset_days()),
            anchor_date_input: anchor_date
                .map(|d| to_local(d, tz).format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            is_team_project,
            assignee_options,
            assigned_to_user_id: item.assigned_to_user_id(),
            is_team_admin,
            points_input: format_points_input(item.points()),
            priority_input: format_points_input(item.priority()),
            just_saved,
            via_full_page,
            depends_on_options,
            depends_on_item_ids,
        }
    }
}

/// One rendered comment row on a task's read-only detail view — "Add comments for tasks"
/// (docs/issues_and_features.md). `author_name` is resolved by the caller (the same `names`
/// map `assignee_name` already uses for a team project; a personal project has only its one
/// owner, so the caller resolves that single name via `UserRepo` instead) rather than carried
/// on the wire, mirroring how `assignedToUserId` is resolved to a display name here already.
/// `attachments` (root CLAUDE.md's Attachments section) is this comment's own files — a file
/// always belongs to a comment now, so `body` may be empty when `attachments` isn't (the
/// file *is* the content; text becomes an optional caption) — see
/// `service::comments::create_comment_with_attachment`.
pub struct CommentLine {
    pub id: String,
    pub author_name: String,
    pub created_at: String,
    pub body: String,
    pub attachments: Vec<AttachmentLine>,
    /// Author-only edit/delete affordances (`service::comments::update_comment`/
    /// `delete_comment`) are only ever shown to the comment's own author.
    pub is_own: bool,
    /// Set when this is the one comment `edit_project_task_comment_form` (the GET
    /// `.../comments/{comment_id}/edit` route) is rendering in edit mode — see
    /// `detail_view.html`'s branch on this field. `false` for every other comment on the
    /// same detail view, and for a plain (non-editing) render of this one too.
    pub editing: bool,
}

/// Builds display rows for a task's comments, oldest first (matches `CommentRepo::list_for_item`'s
/// own order) — see `CommentLine`. `attachments` is every attachment on the item (not
/// pre-grouped — a single `AttachmentRepo::list_for_item` query, same as `comments` itself),
/// grouped here by `comment_id` so each comment only renders its own. `requester_user_id`
/// drives `CommentLine::is_own`; `editing_comment_id` (from the edit-toggle route) marks at
/// most one comment as `editing`.
pub fn comment_lines(
    comments: &[Comment],
    attachments: &[Attachment],
    names: &HashMap<String, String>,
    project_id: &str,
    item_id: &str,
    tz: i32,
    requester_user_id: &str,
    editing_comment_id: Option<&str>,
) -> Vec<CommentLine> {
    let mut by_comment: HashMap<&str, Vec<&Attachment>> = HashMap::new();
    for a in attachments {
        by_comment.entry(a.comment_id.as_str()).or_default().push(a);
    }
    comments
        .iter()
        .map(|c| {
            let own_attachments: Vec<Attachment> = by_comment
                .get(c.id.as_str())
                .into_iter()
                .flatten()
                .map(|a| (*a).clone())
                .collect();
            CommentLine {
                id: c.id.clone(),
                author_name: names
                    .get(&c.author_user_id)
                    .cloned()
                    .unwrap_or_else(|| c.author_user_id.clone()),
                created_at: format_display_date(to_local(c.created_at, tz), true),
                body: c.body.clone(),
                attachments: attachment_lines(&own_attachments, project_id, item_id),
                is_own: c.author_user_id == requester_user_id,
                editing: editing_comment_id == Some(c.id.as_str()),
            }
        })
        .collect()
}

/// One rendered attachment row on a task's read-only detail view — root CLAUDE.md's
/// Attachments section. No author/timestamp fields here — the enclosing `CommentLine`
/// already shows both for the whole comment, and every attachment on this codebase's
/// model belongs to exactly one comment (see `Attachment::comment_id`), so repeating them
/// per-file would be redundant. `download_url` points at the raw-bytes download route,
/// which the template must render with `hx-boost="false"` so htmx doesn't intercept it
/// as a boosted navigation (that would try to swap the file's bytes into `#page`).
/// `is_image` (a plain `content_type` prefix check) is what lets the template render an
/// inline `<img>` thumbnail instead of a bare download link — the download route itself
/// serves an image with `Content-Disposition: inline` for exactly the same reason (see
/// `download_project_task_attachment`), so the `<img src>` actually renders instead of
/// triggering a save-file prompt.
pub struct AttachmentLine {
    pub filename: String,
    pub size_display: String,
    pub download_url: String,
    pub is_image: bool,
}

/// "128 B" / "4.3 KB" / "2.1 MB" — binary (1024-based) units, one decimal place above
/// the smallest unit. Good enough for a byte count nobody needs precise to the byte.
fn format_size_bytes(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Builds display rows for a task's attachments, newest first (matches
/// `AttachmentRepo::list_for_item`'s own order) — see `AttachmentLine`.
pub fn attachment_lines(
    attachments: &[Attachment],
    project_id: &str,
    item_id: &str,
) -> Vec<AttachmentLine> {
    attachments
        .iter()
        .map(|a| AttachmentLine {
            filename: a.filename.clone(),
            size_display: format_size_bytes(a.size_bytes),
            is_image: a.content_type.starts_with("image/"),
            download_url: format!(
                "/web/projects/{project_id}/tasks/{item_id}/attachments/{}",
                a.id
            ),
        })
        .collect()
}

/// Read-only counterpart to `ProjectTaskDetailFields` — see `items.rs`'s `DetailView` for the
/// row-editing convention this mirrors (complete-toggle lives here too).
#[derive(Template)]
#[template(path = "project_tasks/detail_view.html")]
pub struct ProjectTaskDetailView {
    pub id: String,
    pub project_id: String,
    pub description: Option<String>,
    pub complete: bool,
    pub toggle_complete_json: String,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub scheduled_date: Option<String>,
    pub scheduled_end_date: Option<String>,
    pub is_offset_driven: bool,
    pub offset_label: Option<String>,
    /// See `Row::priority_label`'s identical doc comment.
    pub priority_label: Option<String>,
    pub is_team_project: bool,
    pub assignee_name: Option<String>,
    /// `Some((parent_name, parent_url))` when this item has a `parent_item_id` — closes
    /// `docs/issues_and_features.md`'s "Put link to parent in child task detail view".
    /// `parent_url` routes to `/tasks/`, `/events/`, or `/simple-lists/` depending on the
    /// parent's own `ItemKind` — see `resolve_parent_link`. `None` for a top-level item, or
    /// (gracefully, not an error) if the parent has since been deleted.
    pub parent_link: Option<(String, String)>,
    /// See `tasks::TaskDetailView`'s identical field.
    pub linked_event: Option<(String, String)>,
    /// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — `Some((series_name,
    /// edit_url))` when this item was materialized from a series (`item.series_id.is_some()`),
    /// closing `docs/archived/archived_issues_and_features.md`'s ranked item 2(a) ("no link from a materialized item's detail
    /// page back to its series"). `None` for every item never materialized from a series — the
    /// overwhelmingly common case.
    pub series_link: Option<(String, String)>,
    /// "Depends on" (docs/issues_and_features.md) — `(name, url)` per dependency, resolved by
    /// the caller (`service::item_dependencies::list_item_dependencies`). Empty if this item
    /// has none.
    pub depends_on: Vec<(String, String)>,
    /// Read-only reminders view (Stage D of `docs/reminders-in-app-notifications-plan.md`) —
    /// pre-formatted labels (`reminder_labels`) for this item's `ReminderRepo::list_for_item`
    /// rows. Empty for an item with no due/scheduled dates, or on a personal project item
    /// whose caller didn't resolve them. No mutation UI yet — see that plan doc's scope note.
    pub reminders: Vec<String>,
    /// "Add comments for tasks" (docs/issues_and_features.md) — oldest first (matches
    /// `CommentRepo::list_for_item`), see `comment_lines`; `detail_view.html` displays them
    /// newest-first via `.iter().rev()`, so this field's own order stays the stable
    /// oldest-first contract every other reader of it relies on. Task-only: this view struct
    /// is never built for an Event/SimpleList/Template item (`require_task` already guards
    /// every caller of `from_item`).
    pub comments: Vec<CommentLine>,
    /// Whether the "Show comments (N)" disclosure starts open. `true` for every render that's
    /// a direct result of a comment/attachment action (post/edit-toggle/save/cancel/delete —
    /// `render_project_task_comments_view`'s callers, plus `create_project_task_comment_form`/
    /// `delete_project_task_attachment_form`), so the panel doesn't collapse back on the user
    /// mid-interaction; `false` for a plain page load or an unrelated field edit
    /// (`project_task_detail_page`, `update_project_task_form`), matching the "only show
    /// comments when the user asks" request this was added for.
    pub comments_expanded: bool,
}

impl ProjectTaskDetailView {
    #[allow(clippy::too_many_arguments)]
    pub fn from_item(
        item: &Item,
        project_id: &str,
        is_team_project: bool,
        names: &HashMap<String, String>,
        tz: i32,
        parent_link: Option<(String, String)>,
        linked_event: Option<(String, String)>,
        series_link: Option<(String, String)>,
        depends_on: Vec<(String, String)>,
        reminders: Vec<crate::domain::reminder::Reminder>,
        comments: Vec<Comment>,
        attachments: Vec<Attachment>,
        requester_user_id: &str,
        editing_comment_id: Option<&str>,
        comments_expanded: bool,
    ) -> Self {
        let due_date = item
            .due_date()
            .map(|d| format_display_date(to_local(d, tz), item.has_due_time()));
        let scheduled_date = item
            .scheduled_date()
            .map(|d| format_display_date(to_local(d, tz), item.has_scheduled_time()));
        let scheduled_end_date = item
            .scheduled_end_date()
            .map(|d| format_display_date(to_local(d, tz), item.has_end_time()));
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            description: item.description.clone(),
            complete: item.complete(),
            toggle_complete_json: (!item.complete()).to_string(),
            due_date,
            overdue: item.is_overdue(Utc::now()),
            scheduled_date,
            scheduled_end_date,
            is_offset_driven: item.is_offset_driven(),
            offset_label: offset_label_for(item),
            priority_label: priority_label_for(item.priority()),
            is_team_project,
            assignee_name: item
                .assigned_to_user_id()
                .map(|id| names.get(&id).cloned().unwrap_or(id)),
            parent_link,
            linked_event,
            series_link,
            depends_on,
            reminders: reminder_labels(&reminders, tz),
            // `attachments` is every attachment on the item, ungrouped — `comment_lines`
            // groups them by `comment_id` per comment (a file always belongs to a
            // comment now, see root CLAUDE.md's Attachments section).
            comments: comment_lines(
                &comments,
                &attachments,
                names,
                project_id,
                &item.id,
                tz,
                requester_user_id,
                editing_comment_id,
            ),
            comments_expanded,
        }
    }
}

/// Stage C of `docs/unify-virtual-materialized-occurrences-plan.md` — the read-only view for
/// a still-virtual or skipped series occurrence, rendered by the new
/// `GET /projects/:project_id/series/:series_id/occurrences/:occurrence_ts` route (no side
/// effect — this never materializes). Visually mirrors `ProjectTaskDetailView`, but every
/// mutating affordance (checkbox, Skip/Unskip) points at a series/occurrence-scoped route
/// that materializes the occurrence as an internal step of the write, rather than at
/// `/tasks/{id}` (which doesn't exist yet). Once materialized, the detail *page* route
/// redirects to the item's real `/tasks/{id}` page instead of rendering this — so this
/// struct is only ever built for a `Virtual` or `Skipped` occurrence, never a materialized
/// one; there is no `is_top_level`/offset/linked-event handling here because a series
/// occurrence is always top-level and never offset-driven.
#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_view.html")]
pub struct ProjectTaskSeriesOccurrenceView {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub description: Option<String>,
    pub is_skipped: bool,
    pub is_current: bool,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub scheduled_date: Option<String>,
    /// See `Row::priority_label`'s identical doc comment — sourced from the series' own
    /// `priority` (settable only on a `Task`-typed series — see root CLAUDE.md's Priority
    /// section), since this view is only ever built for a still-virtual/skipped occurrence.
    pub priority_label: Option<String>,
    pub is_team_project: bool,
    pub assignee_name: Option<String>,
    pub update_url: String,
    pub skip_url: String,
    pub unskip_url: String,
    /// Ranked issue "Should link to series from series virtual occurrence details page the
    /// same way we link from a materialized occurrence (item) details page" — mirrors
    /// `ProjectTaskDetailView::series_link`, but unlike that field this is never `None`: a
    /// virtual/skipped occurrence always belongs to the series it was rendered from.
    pub series_link: (String, String),
}

impl ProjectTaskSeriesOccurrenceView {
    #[allow(clippy::too_many_arguments)]
    pub fn from_series(
        series: &crate::domain::item_series::ItemSeries,
        occurrence_date: chrono::DateTime<Utc>,
        project_id: &str,
        is_team_project: bool,
        names: &HashMap<String, String>,
        // Stage 4 of docs/assignment-rotation-plan.md — this occurrence's own resolved
        // assignee (`item_series_service::resolve_occurrence_assignee`), not
        // `series.assigned_to_user_id` directly, which is always `None` for a rotating
        // series.
        resolved_assignee_id: Option<String>,
        is_skipped: bool,
        is_current: bool,
        tz: i32,
    ) -> Self {
        let occurrence_ts = occurrence_date.timestamp();
        let local = to_local(occurrence_date, tz);
        let is_due_date_basis = crate::service::item_series::is_due_date_basis(series);
        let due_date = is_due_date_basis.then(|| format_display_date(local, true));
        let scheduled_date = (!is_due_date_basis).then(|| format_display_date(local, true));
        Self {
            series_id: series.id.clone(),
            occurrence_ts,
            description: series.description.clone(),
            is_skipped,
            is_current,
            overdue: is_due_date_basis && occurrence_date < Utc::now(),
            due_date,
            scheduled_date,
            priority_label: priority_label_for(series.priority),
            is_team_project,
            assignee_name: resolved_assignee_id
                .as_ref()
                .map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone())),
            update_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/task",
                series.id,
            ),
            skip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/skip",
                series.id,
            ),
            unskip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/unskip",
                series.id,
            ),
            series_link: (
                series.name.clone(),
                format!("/web/projects/{project_id}/series/{}/edit", series.id),
            ),
        }
    }
}

/// Stage C counterpart to `ProjectTaskSeriesOccurrenceView` — the edit form for a still-
/// virtual series occurrence, prefilled from `ItemSeries` + `occurrence_date` rather than a
/// real `Item` (none exists yet). Submitting it (`Save`) materializes the occurrence and
/// applies the edit in one step — see
/// `project_tasks::handlers::update_project_task_series_occurrence_form`.
#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_fields.html")]
pub struct ProjectTaskSeriesOccurrenceFields {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub name: String,
    pub description: String,
    pub due_date_input: String,
    pub due_time_input: String,
    pub scheduled_date_input: String,
    pub scheduled_time_input: String,
    pub scheduled_end_date_input: String,
    pub scheduled_end_time_input: String,
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub assigned_to_user_id: Option<String>,
    pub is_team_admin: bool,
    pub points_input: String,
    pub priority_input: String,
    pub update_url: String,
}

impl ProjectTaskSeriesOccurrenceFields {
    #[allow(clippy::too_many_arguments)]
    pub fn from_series(
        series: &crate::domain::item_series::ItemSeries,
        occurrence_date: chrono::DateTime<Utc>,
        project_id: &str,
        is_team_project: bool,
        assignee_options: Vec<(String, String)>,
        // Stage 4 of docs/assignment-rotation-plan.md — same resolved-not-raw rationale
        // as `ProjectTaskSeriesOccurrenceView::from_series`.
        resolved_assignee_id: Option<String>,
        is_team_admin: bool,
        tz: i32,
    ) -> Self {
        let occurrence_ts = occurrence_date.timestamp();
        let local = to_local(occurrence_date, tz);
        let date_input = local.format("%Y-%m-%d").to_string();
        let time_input = local.format("%H:%M").to_string();
        let is_due_date_basis = crate::service::item_series::is_due_date_basis(series);
        Self {
            series_id: series.id.clone(),
            occurrence_ts,
            name: series.name.clone(),
            description: series.description.clone().unwrap_or_default(),
            due_date_input: if is_due_date_basis {
                date_input.clone()
            } else {
                String::new()
            },
            due_time_input: if is_due_date_basis {
                time_input.clone()
            } else {
                String::new()
            },
            scheduled_date_input: if is_due_date_basis {
                String::new()
            } else {
                date_input
            },
            scheduled_time_input: if is_due_date_basis {
                String::new()
            } else {
                time_input
            },
            scheduled_end_date_input: String::new(),
            scheduled_end_time_input: String::new(),
            is_team_project,
            assignee_options,
            assigned_to_user_id: resolved_assignee_id,
            is_team_admin,
            points_input: format_points_input(series.points),
            priority_input: format_points_input(series.priority),
            update_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/task",
                series.id,
            ),
        }
    }
}

/// The "Add sub-item" dialog for a still-virtual/skipped Task series occurrence — mirrors
/// `AddChildDialog`, but posts to the occurrence-scoped `.../task-children` route
/// (`create_project_task_series_occurrence_child_form`, which materializes the occurrence
/// before creating the child) rather than the generic create-task route, and has no
/// `parentItemId` field since the occurrence path already identifies the parent. No batch-add
/// section — `create_project_task_series_occurrence_child_form` only ever accepts a single
/// `name`/`due_offset_days` pair, matching `ProjectTaskSeriesOccurrenceChildForm`'s shape.
#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_add_child_dialog.html")]
pub struct ProjectTaskSeriesOccurrenceAddChildDialog {
    pub parent_name: String,
    pub post_create_url: String,
}

/// Stage 2 of docs/dialog-item-forms-plan.md — the read-only dialog for a still-virtual/
/// skipped Task series occurrence, mirroring `ProjectTaskDetailDialog`.
#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_detail_dialog.html")]
pub struct ProjectTaskSeriesOccurrenceDetailDialog {
    pub name: String,
    pub is_skipped: bool,
    pub view: String,
    pub edit_url: String,
}

impl ProjectTaskSeriesOccurrenceDetailDialog {
    pub fn new(
        project_id: &str,
        series_id: &str,
        occurrence_ts: i64,
        name: &str,
        is_skipped: bool,
        view: String,
    ) -> Self {
        Self {
            name: name.to_string(),
            is_skipped,
            view,
            edit_url: format!(
                "/web/projects/{project_id}/series/{series_id}/occurrences/{occurrence_ts}/edit"
            ),
        }
    }
}

/// See docs/item-detail-full-page-retirement.md — this page is now just the read-only detail
/// dialog fragment plus the Decision-3 auto-open script. It previously also rendered a
/// materialize-via-"add sub-item" form directly on the page (dropped, not moved, since nothing
/// in the UI ever linked to this route in the first place — see that doc).
#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_detail_page.html")]
pub struct ProjectTaskSeriesOccurrenceDetailPageTemplate {
    pub name: String,
    pub dialog: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_tasks/series_occurrence_edit_page.html")]
pub struct ProjectTaskSeriesOccurrenceEditPageTemplate {
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}

/// Resolves the (name, detail-page URL) of the Event a task references via `sourceEventId`,
/// scoped to `project_id` — the project-scoped counterpart of `tasks::resolve_linked_event`/
/// `team_tasks::resolve_linked_event`. Links to the project-scoped Events screen directly
/// (`/web/projects/{project_id}/events/{id}`) now that stage B5b has built one — until this
/// stage it fell back to the event's *legacy* detail URL (`dashboard::detail_url`); the event
/// is guaranteed to already belong to `project_id` (fetched via `get_by_project` below), so
/// building the URL locally needs no extra lookup.
/// Resolves the (parent_name, parent_url) of this item's structural parent, if it has one —
/// closes `docs/issues_and_features.md`'s "Put link to parent in child task detail view".
/// The parent can be a Task or an Event (see `docs/CLAUDE.md`'s Events section: an
/// auto-triggered or manually-added task lives under its Event parent the same as any other
/// child), so the URL segment is picked from the parent's own `ItemKind` rather than assumed.
/// Mirrors `resolve_series_link`'s graceful-`NotFound` handling: a parent can be deleted
/// without cascading to its children (see `service::project_items::delete_project_item`), so a
/// missing parent here means "no link to show," not "this item doesn't exist."
pub async fn resolve_parent_link(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(parent_id) = item.parent_item_id() else {
        return Ok(None);
    };
    let parent = match repo.get_by_project(project_id, &parent_id).await {
        Ok(parent) => parent,
        Err(crate::storage::sqlite::RepoError::NotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let segment = match parent.item_type.kind() {
        ItemKind::Event => "events",
        ItemKind::Simple => "simple-lists",
        ItemKind::Task | ItemKind::Template => "tasks",
    };
    Ok(Some((
        parent.name.clone(),
        format!("/web/projects/{project_id}/{segment}/{}", parent.id),
    )))
}

pub async fn resolve_linked_event(
    repo: &Arc<dyn ItemRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(event_id) = item.source_event_id() else {
        return Ok(None);
    };
    let event = repo
        .get_by_project(project_id, &event_id)
        .await
        .map_err(ItemError::from)?;
    Ok(Some((
        event.name.clone(),
        format!("/web/projects/{project_id}/events/{}", event.id),
    )))
}

/// Resolves the (series_name, edit-page URL) of the `ItemSeries` this item was materialized
/// from, via `item.series_id` — see `ProjectTaskDetailView::series_link`'s doc comment.
/// Links to the series' edit page (`/web/projects/{project_id}/series/{series_id}/edit`) —
/// there's no dedicated series *detail* page (only the list and edit screens), and the edit
/// page already shows every field a "view" would.
pub async fn resolve_series_link(
    series: &Arc<dyn crate::storage::sqlite::ItemSeriesRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, ItemError> {
    let Some(series_id) = item.series_id() else {
        return Ok(None);
    };
    // `delete_series` orphans rather than cascades (see its doc comment) — a materialized
    // item can outlive its parent series, so a missing series here means "no link to show",
    // not "this item doesn't exist."
    let series = match series.get_series(&series_id).await {
        Ok(series) => series,
        Err(crate::storage::sqlite::RepoError::NotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some((
        series.name,
        format!("/web/projects/{project_id}/series/{series_id}/edit"),
    )))
}

/// Resolves `item`'s "depends on" set (docs/issues_and_features.md) into `(name, url)` links
/// for the read-only detail view — each dependency is guaranteed to be a same-project Task
/// (enforced at write time by `service::item_dependencies::set_item_dependencies`), so the URL
/// always points at this same screen's own `/tasks/{id}`.
pub async fn resolve_depends_on_links(
    repo: &Arc<dyn ItemRepo>,
    item_dependencies: &Arc<dyn crate::storage::sqlite::ItemDependencyRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Vec<(String, String)>, ItemError> {
    let deps = crate::service::item_dependencies::list_item_dependencies(
        item_dependencies,
        repo,
        project_id,
        &item.id,
    )
    .await?;
    Ok(deps
        .into_iter()
        .map(|d| {
            (
                d.name.clone(),
                format!("/web/projects/{project_id}/tasks/{}", d.id),
            )
        })
        .collect())
}

/// The edit form's dependency picker data: every eligible sibling (same project, same
/// `parent_item_id`, excluding `item` itself and anything not Task-typed — see
/// `set_item_dependencies`'s scoping) as `(id, name)` options, plus `item`'s currently
/// selected dependency ids for pre-checking them.
///
/// Completed siblings and series-occurrence siblings (`series_id.is_some()`) are excluded from
/// the offered options — depending on an already-done item is a no-op guard, and a series
/// occurrence's completion state resets on every cycle, which doesn't compose meaningfully with
/// this feature's one-shot "depends on" semantics. An already-*selected* dependency is always
/// kept in the options regardless of these two filters, even if it's since become complete or
/// turned out to be a series occurrence — otherwise it would silently vanish from the hidden
/// `dependsOnItemIds` field the moment the user toggled any other checkbox (the sync script in
/// `detail_fields.html` rebuilds that field purely from checkboxes present in the DOM), quietly
/// dropping a saved dependency the user never touched. This is UI-level only:
/// `set_item_dependencies` itself still permits either via the API/MCP.
pub async fn depends_on_picker_data(
    repo: &Arc<dyn ItemRepo>,
    item_dependencies: &Arc<dyn crate::storage::sqlite::ItemDependencyRepo>,
    project_id: &str,
    item: &Item,
) -> Result<(Vec<(String, String)>, Vec<String>), ItemError> {
    let siblings = crate::web_ui::project_tasks::sibling_group(
        repo,
        project_id,
        item.parent_item_id().as_deref(),
    )
    .await?;
    let selected = item_dependencies.list_for_item(&item.id).await?;
    let options = siblings
        .into_iter()
        .filter(|s| {
            s.id != item.id
                && s.kind() == ItemKind::Task
                && (selected.contains(&s.id) || (!s.complete() && s.series_id().is_none()))
        })
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    Ok((options, selected))
}

#[derive(Template)]
#[template(path = "project_tasks/rows_fragment.html")]
pub struct ProjectTaskRowsFragmentTemplate {
    pub rows: Vec<String>,
    pub empty_message: String,
}

#[derive(Template)]
#[template(path = "project_tasks/list_page.html")]
pub struct ProjectTasksListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub show_complete: bool,
    /// Stage 2 of docs/list-filtering-plan.md — the filter bar's remaining four controls.
    /// `is_team_project` gates whether the "Assigned to" control renders at all (assignment has
    /// no meaning on a personal project — same precedent as everywhere else this is checked).
    pub is_team_project: bool,
    /// Canonical value (`AssignedToFilter::as_value`) for the "Assigned to" `<select>`'s
    /// `selected` comparison: `"me"` | `"unassigned"` | `"all"` | a specific user id.
    pub assigned_to: String,
    pub assignee_options: Vec<(String, String)>,
    /// `DueDateFilter::as_value`: `"all"` | `"overdue"` | `"none"`.
    pub due_date: String,
    /// `ScheduleFilter::as_value`: `"all"` | `"past"` | `"none"`.
    pub schedule: String,
    /// `true` = show recurring items (the default).
    pub recurring: bool,
    /// `PriorityFilter::as_value`: `""` (any priority) | `"1"`-`"4"`.
    pub priority: String,
    /// Pre-encoded `ListFilters::query_string()` (empty at all-default filters) — used to build
    /// the "New task" button's URL so the filters carry into `new_project_task_page` (which
    /// re-embeds them as `NewProjectTaskPageTemplate::filters_query` for the batch form's own
    /// round trip). Replaces the old ad hoc `?showComplete=1` literal this button used to build
    /// by hand — see Stage 1's doc comment on `ListFilters::query_string`.
    pub filters_query: String,
    /// `Some("{n} pts")` on a team-backed project (the viewer's own balance — see
    /// `service::teams::member_points`), `None` on a personal project.
    pub points_label: Option<String>,
    /// `?select=1` (docs/issues_and_features.md's "Multi-select" item) — when set, `rows` holds
    /// `ProjectTaskSelectRow` markup instead of the normal `ProjectTaskRow`/virtual-occurrence
    /// mix (see `handlers::project_tasks_page`'s dispatch), and the header shows Cancel/Actions
    /// instead of Select.
    pub select_mode: bool,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_tasks/new_page.html")]
pub struct NewProjectTaskPageTemplate {
    pub project_id: String,
    pub show_complete: bool,
    pub is_team_project: bool,
    pub assignee_options: Vec<(String, String)>,
    pub blank_scheduled_date_input: String,
    pub blank_scheduled_time_input: String,
    pub blank_scheduled_end_date_input: String,
    pub blank_scheduled_end_time_input: String,
    pub blank_due_date_input: String,
    pub blank_due_time_input: String,
    pub is_team_admin: bool,
    pub blank_points_input: String,
    pub blank_priority_input: String,
    /// Stage 2 of docs/list-filtering-plan.md — the list screen's non-default filters at the
    /// moment this page/dialog was opened, pre-encoded via `ListFilters::query_string()` (empty
    /// at all-default filters). Round-tripped through the "Add multiple at once" batch form's
    /// hidden `filtersQuery` field so a redirect back to the list after a batch create lands on
    /// the same filtered view — see `ProjectTaskForm::filters_query`/`BatchForm::filters_query`.
    pub filters_query: String,
    /// Set when this dialog is opened from a context with no Tasks list underneath to swap
    /// into (e.g. `project_calendar`'s "+ Task" button, which replaces the whole page rather
    /// than opening over a list) — see `new_page.html`'s own doc comment. Renders the hidden
    /// `redirect` field and points the form at itself (`hx-target="this"`, `hx-swap="none"`)
    /// instead of `#items-list`, so the client-side pre-flight target resolution htmx does
    /// before even sending the request never fails for lack of an `#items-list` to find — the
    /// create still succeeds via the same `HX-Redirect`-back-to-the-list path
    /// `redirect_to_project_tasks` already serves the "Add multiple at once" batch form.
    pub redirect_after_create: bool,
    pub nav_html: String,
}

/// Stage 1 of docs/dialog-item-forms-plan.md — the read-only detail dialog wrapping
/// `ProjectTaskDetailView`'s already-rendered `view` HTML. See `detail_dialog.html`'s own doc
/// comment for why this is deliberately lighter than the full detail page.
#[derive(Template)]
#[template(path = "project_tasks/detail_dialog.html")]
pub struct ProjectTaskDetailDialog {
    pub name: String,
    pub complete: bool,
    pub view: String,
    pub edit_url: String,
}

impl ProjectTaskDetailDialog {
    pub fn new(id: &str, project_id: &str, name: &str, complete: bool, view: String) -> Self {
        Self {
            name: name.to_string(),
            complete,
            view,
            edit_url: format!("/web/projects/{project_id}/tasks/{id}/edit"),
        }
    }
}

/// See docs/item-detail-full-page-retirement.md's revert note — this full page is back
/// (header with Edit/Back/Delete, `view`'s own read-only fields including its inline Move
/// button and Parent/Linked event/Series links, Sub-items management). `dialog` is still
/// rendered and embedded (`hidden`, in detail_page.html) purely so the row's own hx-get
/// (target=#action-dialog, select=#dialog-fragment — see components/row.html) can pluck it out
/// of this same route's response; as of 2026-08-25 nothing auto-opens it when this page is
/// loaded directly (see detail_page.html's own comment) — full-page access is popover-only now
/// (components/row_actions_menu.html's "View full page" entry), never from inside the dialog
/// itself, so the two navigation modes never race each other. No separate Move button is
/// needed here (`view` already renders it inline) but the header link now mirrors Simple
/// Lists' — see `parent_link` below — closing `docs/issues_and_features.md`'s "Back to
/// <item-type>" → "Up to <parent item>" item.
#[derive(Template)]
#[template(path = "project_tasks/detail_page.html")]
pub struct ProjectTaskDetailPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub complete: bool,
    pub view: String,
    pub dialog: String,
    pub nav_html: String,
    /// `Some((parent_name, parent_url))` when this task has a `parent_item_id` — the header
    /// link then reads "Up to {parent_name}" (linking to the parent's own detail page) instead
    /// of the generic "Back to tasks" list link, matching `project_simple_lists/detail_page.html`.
    pub parent_link: Option<(String, String)>,
}

#[derive(Template)]
#[template(path = "project_tasks/edit_page.html")]
pub struct ProjectTaskEditPageTemplate {
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{EventItem, ItemType, TaskItem};
    use crate::storage::sqlite::{
        ItemDependencyRepo, ItemSeriesRepo, MockItemDependencyRepo, MockItemRepo,
        MockItemSeriesRepo, RepoError,
    };

    #[tokio::test]
    async fn resolve_parent_link_returns_none_when_item_is_top_level() {
        let item = Item::default();
        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());

        let result = resolve_parent_link(&repo, "p1", &item).await;

        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn resolve_parent_link_returns_none_when_parent_was_deleted() {
        // A parent can be deleted without cascading to its children (see
        // `service::project_items::delete_project_item`), so a dangling `parent_item_id`
        // must degrade to "no link," not a 500.
        let item = Item {
            item_type: ItemType::Task(TaskItem {
                parent_item_id: Some("deleted-parent".to_string()),
                ..TaskItem::default()
            }),
            ..Item::default()
        };
        let mut mock = MockItemRepo::new();
        mock.expect_get_by_project()
            .returning(|_, _| Err(RepoError::NotFound));
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let result = resolve_parent_link(&repo, "p1", &item).await;

        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn resolve_parent_link_routes_to_events_for_an_event_parent() {
        let item = Item {
            item_type: ItemType::Task(TaskItem {
                parent_item_id: Some("parent1".to_string()),
                ..TaskItem::default()
            }),
            ..Item::default()
        };
        let mut mock = MockItemRepo::new();
        mock.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "parent1".to_string(),
                name: "Company Picnic".to_string(),
                item_type: ItemType::Event(EventItem {
                    schedule: Default::default(),
                    recurrence: Default::default(),
                    event_type: None,
                    series_id: None,
                    google_event_id: None,
                    calendar_subscription_id: None,
                }),
                ..Item::default()
            })
        });
        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let result = resolve_parent_link(&repo, "p1", &item).await.unwrap();

        assert_eq!(
            result,
            Some((
                "Company Picnic".to_string(),
                "/web/projects/p1/events/parent1".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn resolve_series_link_returns_none_when_parent_series_was_deleted() {
        // Regression for the "delete a series, then its materialized occurrence 404s"
        // bug: `delete_series` orphans (see its doc comment) rather than cascading, so
        // `item.series_id` can point at a series row that no longer exists.
        let item = Item {
            item_type: ItemType::Task(TaskItem {
                series_id: Some("deleted-series".to_string()),
                ..TaskItem::default()
            }),
            ..Item::default()
        };
        let mut mock = MockItemSeriesRepo::new();
        mock.expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(mock);

        let result = resolve_series_link(&series, "p1", &item).await;

        assert_eq!(result.unwrap(), None);
    }

    fn series_fixture() -> crate::domain::item_series::ItemSeries {
        crate::domain::item_series::ItemSeries {
            id: "s1".to_string(),
            project_id: "p1".to_string(),
            name: "Standup".to_string(),
            description: None,
            event_type: None,
            recurrence: "every 7 days".to_string(),
            anchor_date: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            item_type: crate::domain::item::ItemKind::Task,
            cursor_date: None,
            basis: None,
            template_item_id: None,
            assigned_to_user_id: None,
            points: None,
            priority: None,
        }
    }

    #[test]
    fn from_series_links_to_the_series_edit_page() {
        let series = series_fixture();

        let view = ProjectTaskSeriesOccurrenceView::from_series(
            &series,
            series.anchor_date,
            "p1",
            false,
            &HashMap::new(),
            None,
            false,
            true,
            0,
        );

        assert_eq!(
            view.series_link,
            (
                "Standup".to_string(),
                "/web/projects/p1/series/s1/edit".to_string()
            )
        );
    }

    fn task(id: &str, name: &str, complete: bool, series_id: Option<&str>) -> Item {
        Item {
            id: id.to_string(),
            name: name.to_string(),
            project_id: Some("p1".to_string()),
            item_type: ItemType::Task(TaskItem {
                complete,
                series_id: series_id.map(str::to_string),
                ..TaskItem::default()
            }),
            ..Item::default()
        }
    }

    #[tokio::test]
    async fn depends_on_picker_data_excludes_completed_and_series_occurrence_siblings() {
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_list_by_project().returning(|_, _| {
            Ok(vec![
                task("i1", "Self", false, None),
                task("i2", "Open sibling", false, None),
                task("i3", "Done sibling", true, None),
                task("i4", "Series occurrence", false, Some("series1")),
            ])
        });
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec![]));
        let item_dependencies: Arc<dyn ItemDependencyRepo> = Arc::new(dep_repo_mock);
        let item = task("i1", "Self", false, None);

        let (options, selected) = depends_on_picker_data(&repo, &item_dependencies, "p1", &item)
            .await
            .unwrap();

        assert_eq!(
            options,
            vec![("i2".to_string(), "Open sibling".to_string())]
        );
        assert!(selected.is_empty());
    }

    #[tokio::test]
    async fn depends_on_picker_data_keeps_an_already_selected_sibling_even_if_now_ineligible() {
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_list_by_project().returning(|_, _| {
            Ok(vec![
                task("i1", "Self", false, None),
                task("i3", "Done sibling", true, None),
                task("i4", "Series occurrence", false, Some("series1")),
            ])
        });
        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec!["i3".to_string(), "i4".to_string()]));
        let item_dependencies: Arc<dyn ItemDependencyRepo> = Arc::new(dep_repo_mock);
        let item = task("i1", "Self", false, None);

        let (mut options, selected) =
            depends_on_picker_data(&repo, &item_dependencies, "p1", &item)
                .await
                .unwrap();

        options.sort();
        assert_eq!(
            options,
            vec![
                ("i3".to_string(), "Done sibling".to_string()),
                ("i4".to_string(), "Series occurrence".to_string()),
            ]
        );
        assert_eq!(selected, vec!["i3".to_string(), "i4".to_string()]);
    }
}
