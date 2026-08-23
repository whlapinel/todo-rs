# List filtering — Project Tasks screen

Status: **Stages 1 and 2 done and committed.** Scopes the "Need filtering for all lists" entry in `docs/issues_and_features.md`, narrowed to a single screen first — see Out of scope.

## Progress (read this first if picking this up in a fresh context)

- **Stage 1 is complete**: `src/web_ui/list_filters.rs` (new file, registered via `pub mod list_filters;` in `src/web_ui/mod.rs`) implements `ListFilterQuery`, `ListFilters`, `AssignedToFilter`/`DueDateFilter`/`ScheduleFilter`, `ListFilters::from_query`/`matches`/`query_string`, plus 14 passing unit tests (`cargo test --bin todo web_ui::list_filters`). This also folds in what the original draft called "Stage 3" (the predicate logic + its tests) — `matches` is fully implemented and tested here, not a stub, so there is no separate predicate-writing stage left to do.
- `matches`'s actual signature ended up as `matches(&self, item: &Item, requester_user_id: &str, is_team_project: bool, now: DateTime<Utc>) -> bool` — two params beyond the original sketch (`is_team_project` to gate the assignment check, `now` for the due/schedule comparisons, passed in rather than read via `Utc::now()` so the function stays unit-testable). Stage 2 supplies both everywhere it calls `matches` (directly or via `render_rows_with_virtual`).
- **Stage 2 is complete**: `project_tasks_page`, `new_project_task_page`, `list_task_rows_for_project`, `render_rows_with_virtual`, `ProjectTaskVirtualRow::from_occurrence`, `create_project_task_form`, `create_project_tasks_batch`, and `templates/project_tasks/list_page.html`/`new_page.html` all now go through `ListFilters` — `showComplete` is no longer the only working filter. `AssignedToFilter`/`DueDateFilter`/`ScheduleFilter` each gained an `as_value()` helper (canonical string form for a `<select>`'s `selected` comparison, also reusable by later screens).
  - **Two real deviations from the Stage 2 design sketch, both load-bearing:**
    1. `ListFilterQuery`'s field names can't be embedded as individual hidden `<input>`s inside `ProjectTaskForm`/the "New task" dialog form — `ListFilterQuery::due_date`'s wire name (`dueDate`) collides with that same form's own item-due-date input (`macros::due_date_fields`). Fixed by round-tripping a single **opaque, pre-encoded** `ListFilters::query_string()` string as one new field (`filters_query` on `ProjectTaskForm` and `BatchForm`, rendered as a hidden `filtersQuery` input) rather than one field per filter dimension. `redirect_to_project_tasks` takes this raw string directly and appends it to the redirect URL — it never reconstructs a `ListFilters` from it.
    2. Two other query-param structs also had to gain the same 5 fields to keep compiling once `list_task_rows_for_project`'s signature changed to take `&ListFilters`: `project_tasks::handlers::OccurrenceRowActionQuery` (the row-checkbox completion route) and, in a different module, `project_item_series::handlers::OccurrenceRowActionQuery` (Skip/Unskip). Both are `Query` extractors (not form bodies sharing a namespace with item fields), so no collision there — a new shared helper `project_tasks::list_filters_from_parts` builds a `ListFilters` from five loose `Option<String>` parts for exactly this case. Each struct's `view=all-tasks`/`view=all-events` branches still pass a bare `bool` to `all_projects_tasks`/`all_projects_events` unchanged — those screens are still Out of scope (see below) and weren't touched.
  - `render_rows`/`render_scope_fragment`/`render_children_fragment`/`render_source_event_fragment` (children/subordinate sub-lists, not the top-level filtered list) were deliberately **not** touched — the plan's Stage 2 design only named `project_tasks_page`/`list_task_rows_for_project`/`render_rows_with_virtual`, and sub-item lists aren't reachable through the filter bar's own controls.
- Verified: `cargo fmt` (repo-root, no path args) made only whitespace changes; `cargo build` (whole workspace) and `cargo test --bin todo` (483 tests) both pass clean, no new warnings beyond pre-existing unrelated dead-code lints.

## Decisions confirmed with the user (2026-08-23)

- Scope this pass to the **Project Tasks screen only** (`/web/projects/:project_id/tasks`, `src/web_ui/project_tasks/`). The other four list screens (`project_events`, `project_simple_lists`, `all_projects_tasks`, `all_projects_events`) are a deliberate follow-up, not part of this plan — see Out of scope.
- Filters are **URL query params only** — no cookie/localStorage/user-settings persistence. A fresh visit to `/tasks` always shows the defaults below; a filtered view is only "remembered" via its own URL (shareable/bookmarkable), matching how `showComplete` already works today.
- **Revised 2026-08-23, after the first pass of this plan**: drop the `skipped` filter and drop multi-select entirely. `assignedTo` is a plain single-select — `Me` / `Unassigned` / `All` / one specific member — a normal `<select>`, no customizable-select experiment, no repeated-query-key handling. This removes both flagged complications from the first draft (see git history of this file for the abandoned multi-select/customizable-select/skipped-window design if it's ever revisited).

## Context

Today `project_tasks` has exactly one filter — `showComplete` — and it's threaded through by hand in half a dozen places: the page's own `Query<ShowCompleteQuery>`, a hidden `<input>` on the new-item dialog, an `hx-vals`-free baked-in query suffix on each row's own checkbox/skip/unskip URLs (`ProjectTaskRow::from_item`'s `show_complete: bool` param), and `ProjectTaskForm.show_complete` round-tripped through every update. This plan generalizes that exact pattern to six filter dimensions rather than inventing a new mechanism — the row-URL-threading problem below is the same shape `showComplete` already solved, just wider.

Filters, from the issue, as they apply to a single-project Tasks screen (project filter is dropped here — meaningless on a screen already scoped to one project; see Out of scope):

| Filter | Default | Values | Backing field |
|---|---|---|---|
| complete | `false` (hide) | show / hide | `Item::complete` (already exists as `showComplete`) |
| assigned to | `Me` | Me / Unassigned / All / one specific active project member (**single-select**) | `Item::assigned_to_user_id()` — team-backed projects only, see below |
| due date | `all` | all / overdue / none | `Item::is_overdue(now)` (already exists) / `Item::due_date()` |
| schedule | `all` | all / scheduled-in-past / none | `Item::scheduled_date()` compared to now |
| recurring | `true` (show) | show / hide | `Item::series_id` (`Some` ⇒ came from an `item_series`, whether still virtual or already materialized) |

Sort: due date, undated last, is already what `list_project_tasks`'s `sort_key` does. The issue's "other options to be added later (maybe)" for sort is explicitly speculative — not scoped here.

`skipped` (from the original issue) is dropped from this plan entirely — see Out of scope.

All five of these are pure in-memory predicates over the `Vec<Item>` that `list_project_tasks`/`list_task_rows_for_project` already fetch unconditionally today — no new repo method, no new SQL, no multi-value query-string parsing (every field here is single-valued, so `axum::Query`'s ordinary `Option<String>` deserialization is all that's needed — no repeated-key concerns to design around). This mirrors how `showComplete` already works: the fetch is unfiltered, filtering happens in Rust before rendering.

## Design

### Stage 1 — shared filter vocabulary (new, built for reuse beyond this screen) — ✅ done

`src/web_ui/list_filters.rs` (sibling to `nav.rs`, not nested under `project_tasks/`, since every other screen in Out of scope will reuse this) implements:

```rust
#[derive(serde::Deserialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListFilterQuery {
    pub show_complete: Option<String>,  // existing convention: presence = true
    pub assigned_to: Option<String>,    // "me" | "unassigned" | "all" | a specific user id | absent = "me" (the default)
    pub due_date: Option<String>,       // "overdue" | "none" | absent = all
    pub schedule: Option<String>,       // "past" | "none" | absent = all
    pub recurring: Option<String>,      // "no" | absent = show (default true)
}

pub enum AssignedToFilter { Me, Unassigned, All, User(String) }
pub enum DueDateFilter { All, Overdue, None }
pub enum ScheduleFilter { All, Past, None }

pub struct ListFilters {
    pub show_complete: bool,
    pub assigned_to: AssignedToFilter,
    pub due_date: DueDateFilter,
    pub schedule: ScheduleFilter,
    pub recurring: bool,
}

impl ListFilters {
    pub fn from_query(q: ListFilterQuery) -> Self { .. }
    pub fn matches(&self, item: &Item, requester_user_id: &str, is_team_project: bool, now: DateTime<Utc>) -> bool { .. }
    pub fn query_string(&self) -> String { .. } // for baking into row/dialog URLs, replaces today's ad hoc `?showComplete=1`
}
```

`query_string()` centralizes what today is hand-built per call site (`list_query` in `templates.rs:299`, the `?showComplete=1` literal in `list_page.html`/`new_page.html`) — every URL-building call site in Stage 2 should call this instead of formatting its own suffix. It returns a bare `key=value&key2=value2` fragment (no leading `?`/`&`, empty string at all-default filters) — see its doc comment in the module for how a caller is expected to combine it with an existing prefix. This is the piece that makes extending to the other four screens later (Out of scope) cheap: they reuse `ListFilterQuery`/`ListFilters` as-is, only their own per-screen `matches`-adjacent glue (e.g. no `assigned_to` concept on `project_simple_lists`) differs.

`matches` already implements and unit-tests every predicate in the table above (complete/assignedTo/dueDate/schedule/recurring) — see the 14 tests in `list_filters.rs`'s own `#[cfg(test)] mod tests`. There is nothing left to design here; Stage 2 is purely about calling this from the actual screen.

### Stage 2 — wire into Project Tasks: page load, list rebuild, filter bar — ✅ done

- `project_tasks_page`, `list_task_rows_for_project`, `render_rows_with_virtual` (`src/web_ui/project_tasks/{handlers,mod}.rs`) take `&ListFilters` instead of `show_complete: bool`; the `visible` predicate becomes `filters.matches(item, &auth_user.user_id, project.team_id.is_some(), Utc::now())` instead of `show_complete || !i.complete` (both extra args are real params on the already-implemented `matches` — see Stage 1 above, not something to design). The `recurring=no` case additionally drops `virtual_occurrences` entirely (a virtual row only ever represents a series occurrence) and filters materialized items by `item.series_id.is_none()`.
- `templates/project_tasks/list_page.html`: replace the single checkbox with a `<form id="filter-bar" hx-get=".../tasks" hx-trigger="change" hx-target="#page" hx-select="#page" hx-swap="innerHTML" hx-push-url="true">` wrapping all filter controls — one shared `hx-get` instead of each control repeating the attributes (today's pattern doesn't generalize past one control). Controls: existing checkbox for complete; a plain `<select name="assignedTo">` (options: Me, Unassigned, All, each `active_member_options` entry — only rendered when `project.team_id.is_some()`, matching every other assignee-only-on-team-projects precedent in this codebase) styled with the same classes this app's other `<select>` inputs already use (`macros.html`); plain `<select>`s for `dueDate`/`schedule`/`recurring`.
- `ProjectTaskRow::from_item` (`templates.rs`), `ProjectTaskForm` (`mod.rs`), `OccurrenceRowActionQuery`, and every row-view branch in `update_project_task_form` (`handlers.rs:876-1083`) that currently thread `show_complete: bool`/`q.show_complete` swap to threading `&ListFilters`/`filters.query_string()` instead — mechanical but touches every one of the ~8 call sites `show_complete` touches today (checkbox `hx-vals`, skip/unskip URLs, reschedule/assign dialog URLs, the new-item dialog's hidden fields in `new_page.html`). This is the bulk of the diff's line count, none of it novel — same shape as the existing `show_complete` threading, just wider.

## Critical files

| File | Change | Status |
|---|---|---|
| `src/web_ui/list_filters.rs` (new) | `ListFilterQuery`/`ListFilters`, `matches`, `query_string` | ✅ done |
| `src/web_ui/mod.rs` | `pub mod list_filters;` | ✅ done |
| `src/web_ui/project_tasks/handlers.rs` | `project_tasks_page`, `new_project_task_page`, `complete_project_item_series_occurrence_form`'s `OccurrenceRowActionQuery` + `tasks-list` branch, `create_project_task_form`/`create_project_tasks_batch`/`redirect_to_project_tasks` — swap `show_complete`/`ShowCompleteQuery` for `ListFilters` | ✅ done |
| `src/web_ui/project_tasks/mod.rs` | `list_task_rows_for_project`, `render_rows_with_virtual`, `ProjectTaskForm`/`BatchForm` (`filters_query`, opaque — see Progress), new `list_filters_from_parts` helper | ✅ done |
| `src/web_ui/project_tasks/templates.rs` | `ProjectTaskVirtualRow::from_occurrence` (`list_query`), `ProjectTasksListPageTemplate`/`NewProjectTaskPageTemplate` new fields | ✅ done |
| `templates/project_tasks/list_page.html` | Filter bar form + controls | ✅ done |
| `templates/project_tasks/new_page.html` | Round-trip full filter set via hidden fields (was just `showComplete`) | ✅ done |
| `src/web_ui/project_item_series/handlers.rs` (not in original plan) | `OccurrenceRowActionQuery` (Skip/Unskip) + `rebuild_tasks_list_response` — a second, separate call site into `list_task_rows_for_project` outside `project_tasks/`, discovered while wiring Stage 2 | ✅ done |

`templates/macros.html` needed no changes — the filter bar's `<select>`s reuse the existing Tailwind classes inline (matching `quick_assign_dialog.html`'s precedent) rather than a new shared macro.

## Out of scope (this pass)

- `project_events`, `project_simple_lists`, `all_projects_tasks`, `all_projects_events` — same `ListFilters` type is meant to be reused (Stage 1 is deliberately screen-agnostic), but wiring each one in is separate follow-up work: `project_simple_lists` has no due/scheduled/recurring concept at all (only `complete` applies — see `Item::validate`'s `Simple` exclusions), `all_projects_tasks`/`all_projects_events` additionally need the `project` filter dimension (a single-select across `ProjectRepo::list_for_user`, or `all`) that a single-project screen has no use for.
- The `project` filter dimension entirely (only meaningful cross-project).
- The `skipped` filter — dropped from scope per the user's 2026-08-23 revision above, not deferred to a later stage of this plan. If it's wanted later it needs its own fresh scoping pass (the original draft's concern still applies: the Tasks list never fetches skipped occurrences today, and showing them raises an unresolved "since when" time-window question).
- Multi-select of any kind, and the native "customizable select" experiment — dropped per the same revision. `assignedTo` is a single value.
- Non-due-date sort options ("other options to be added later (maybe)" per the issue itself).
- Filter persistence beyond the URL (cookie/localStorage/user settings) — explicitly decided against for this pass.

## Verification

- ✅ Done (Stage 1): 14 unit tests for `ListFilters::matches`/`query_string`/`from_query` in `src/web_ui/list_filters.rs`, no DB/repo involved — `cargo test --bin todo web_ui::list_filters`.
- ✅ Done (Stage 2): added `as_value_round_trips_through_from_query` (15th `list_filters` test, covering the new `as_value()` helpers). `cargo build` (whole workspace) and `cargo test --bin todo` (483 tests, full suite) both pass clean; `cargo fmt` made only whitespace changes. No UI-only verification was done — per this repo's CLAUDE.md, browser-automation click-through isn't used here, and the user hasn't yet done their own manual smoke test of the filter bar against a live team-backed project (mixed assigned/unassigned/overdue/scheduled-past/recurring tasks, bookmarkable filtered URLs, filters surviving a row completion/reschedule) — that's still open, not something this change can self-certify.
