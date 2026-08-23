# All-projects landing page (Tasks / Events / Calendar)

## How to use this doc across a context clear

This is a multi-stage plan, executed one stage per conversation (the user clears
context between stages to keep each session focused). Whoever is executing a stage
must, before ending that turn:

1. Implement the stage.
2. Verify (`cargo build` at minimum; see Verification section).
3. **Update this doc**: append to the "Progress log" section at the bottom with what
   was actually done, any deviations from the plan below, exact function/route names
   verified or discovered along the way, and anything the *next* stage's implementer
   (who will start with zero conversation context, only this file) needs to know.
4. Commit the changes (implementation + this doc's update) in one commit. Do not ask
   for permission first — this is standing authorization for this plan's stage
   commits specifically. Do not add a `Co-Authored-By` trailer (see root `CLAUDE.md`).
5. Tell the user the stage is done and it's safe to clear context, and name the next
   stage number.

Each new stage's implementer should start by reading this entire file (plan +
progress log) before touching code — the progress log is the authoritative record of
what's already true in the codebase, taking precedence over the original plan text
below wherever they conflict.

## Context

`docs/issues_and_features.md`'s top entry asks to replace the current cross-project
"Home" landing (`main_calendar.rs`, mounted at `/web/calendar` + `/web/calendar/list`)
with a real all-projects sidebar context, mirroring what each project already gets.
The main page was switched from a dashboard to a calendar grid a while back for
reasons the user no longer recalls being right, and having a flat "list view of the
calendar" (merging Tasks+Events by date) never made much sense once dedicated
per-type screens existed.

Scope, confirmed with the user via AskUserQuestion:
- All-projects sidebar gets exactly **Tasks, Events, Calendar** — no Simple Lists,
  Templates, or Series at the cross-project level.
- The calendar's flat **list** view is removed entirely, both cross-project
  (`/web/calendar/list`) and per-project (`/web/projects/:id/calendar/list`) — the
  calendar **grid** + day-drawer stay as-is.
- The landing page (`/`) defaults to the new all-projects **Tasks list**, not the
  calendar grid.
- Explicitly out of scope: a "+ New Task/Event" button on the two new cross-project
  list screens (creation is inherently project-scoped; a project-picker-driven
  creation flow is separately tracked as the next `docs/issues_and_features.md`
  bullet and as `docs/issues_and_features.md:53`'s existing gap — not built here).

## Key existing structure (verified by reading the code)

- `src/web_ui/main_calendar.rs` and `src/web_ui/project_calendar.rs` each currently
  serve **two** views that share some but not all code: a grid+day-drawer (KEEP) and
  a flat list (REMOVE). The split:
  - `calendar_row()` (both files) is used by **both** the day-drawer and the list —
    it unconditionally suffixes `?view=main-calendar`/`?view=project-calendar` onto
    every row's `reschedule_url`/`assign_url`. This suffix is what makes a saved
    Reschedule/Assign edit re-render with calendar styling (recent commit "Preserve
    calendar row styling on Reschedule/Assign saves"). **This must be kept** — the
    day-drawer still needs it.
  - `list_main_calendar_rows()`/`list_calendar_rows_for_project()` (the list-only row
    aggregators) and the `view=main-calendar`/`view=project-calendar` **checkbox/skip/
    unskip rebuild branches** in `project_tasks/handlers.rs`,
    `project_events/handlers.rs`, and `project_item_series/handlers.rs` are reachable
    **only** from the flat list's virtual-occurrence rows (confirmed:
    `MainCalendarVirtualRow::from_occurrence`'s `list_query` param, which bakes the
    `?view=...` suffix onto complete/skip/unskip URLs, is `Some` only when called from
    the list, `None` from the day-drawer). These become fully dead once the list pages
    are removed — **safe and correct to delete**.
  - `project_item_series/handlers.rs`'s `skip_project_item_series_occurrence_form`/
    `unskip_project_item_series_occurrence_form` each have their own
    `"project-calendar"`/`"main-calendar"` branches (calling
    `rebuild_project_calendar_list_response`/`rebuild_main_calendar_list_response`) —
    a second, independent dead-code chain to remove alongside the above.
  - `project_events/handlers.rs` reuses `project_tasks::{RowViewQuery,
    normalize_row_view}` directly — one shared extension point, not two.
- `ProjectTaskRow`/`ProjectEventRow::from_item` both build the shared `components::
  row::Row`, which already has a `project_name: Option<String>` field (used today only
  by `main_calendar::calendar_row`'s overlay). The new all-projects Tasks/Events
  screens should mirror the **per-project** Tasks/Events list shape (top-level items +
  expandable children, single item-kind, no flattening-by-date, no type badge) — not
  `main_calendar`'s merged/date-sorted shape — so their row-builders are simpler than
  `main_calendar::calendar_row`: just `from_item(...)` + `row.project_name =
  Some(...)` + rewritten `complete_url`/`reschedule_url`/`assign_url`.
- `src/web_ui/nav.rs`: `ActiveContext` is `Project(String) | None`; `build_nav_html`
  only populates `section_links`/`calendar_href`/`activity_href` for
  `Project(id)`. `templates/nav_sidebar_inner.html` renders `calendar_href` (as
  "Calendar") then `section_links`, then a fixed bottom block including a hardcoded
  "Home" → `/web/calendar` link. `templates/nav.html`'s header logo also links to
  `/web/calendar`.
- Grid pages (`templates/main_calendar/calendar_page.html`,
  `templates/project_calendar/calendar_page.html`) each have a "List view" `<a>` link
  that must be removed alongside the routes.

## Implementation stages

**Stage 1 — `nav.rs` + routing skeleton**
- `nav.rs`: add `ActiveContext::AllProjects`. In `build_nav_html`, add a 3rd match arm:
  `section_links` = just Tasks + Events (fixed hrefs `/web/tasks`/`/web/events`, not
  `section_href`-derived since that helper is project-scoped), `calendar_href =
  Some("/web/calendar")`, `activity_href = None`.
- `main_calendar.rs`: both `build_nav_html` calls (grid page + day fragment) switch
  from `ActiveContext::None` to `ActiveContext::AllProjects`.
- `src/main.rs`: add routes `GET /web/tasks`, `GET /web/events`,
  `PUT /web/tasks/projects/:project_id/items/:item_id`. Retarget `/` (both
  `internal`/`caddy` branches) from `/web/calendar` to `/web/tasks`. Retarget
  `redirect_main_dashboard_list`'s target to `/web/tasks` and
  `redirect_project_dashboard_list`'s target to `/web/projects/{project_id}/tasks`
  (legacy-bookmark insurance, same rationale the existing redirects already document).
- `templates/nav.html` header logo and `templates/nav_sidebar_inner.html`'s "Home"
  link: `/web/calendar` → `/web/tasks`.
- Note: at the end of this stage, `/web/tasks`/`/web/events` routes exist but their
  handlers don't yet (Stages 2/3) — either stub them minimally (e.g. a placeholder
  `Html("TODO")` handler) so `cargo build` passes end-to-end, or hold the route
  additions until Stage 2/3 and keep Stage 1 to just the nav.rs/redirect/template
  changes that don't depend on the new handlers existing. Pick whichever keeps this
  stage's `cargo build` green; note the choice in the progress log.

**Stage 2 — All-projects Tasks screen** (new `src/web_ui/all_projects_tasks.rs` +
`templates/all_projects_tasks/list_page.html`, mirroring `project_tasks`'s per-project
list minus the "+ New Task" button)
- Handler loops `project_service::list_projects`, gathers each project's Task items +
  current non-materialized Task-series occurrences (same shape as
  `project_tasks::list_task_rows_for_project`, duplicated into this module per this
  codebase's established "duplicate small per-screen helpers" precedent).
- Reuses `main_calendar::is_included`'s exact filtering semantics (Task: unrestricted
  on personal projects, `assigned_to == requester` — or `assigned_to_any` if a toggle
  is added — on team-backed ones); relocate/duplicate this predicate.
- New toggle-complete handler `PUT /web/tasks/projects/:project_id/items/:item_id`
  mirroring `main_calendar::toggle_main_calendar_item_complete`.

**Stage 3 — All-projects Events screen** (new `src/web_ui/all_projects_events.rs` +
template), same shape as Stage 2 but no completion (Events aren't completable) and no
assignment/points.

**Stage 4 — Wire the shared save/skip/unskip handlers**
- `project_tasks/mod.rs::normalize_row_view`: accept `"all-tasks"`.
- `project_tasks/handlers.rs::update_project_task_form`: add `Some("all-tasks")` row-
  render arm; `complete_project_item_series_occurrence_form`: add an `"all-tasks"`
  rebuild branch (mirroring the `"tasks-list"` one).
- `project_events/handlers.rs::update_project_event_form`: add an `"all-events"` arm.
- `project_item_series/handlers.rs`: on both skip/unskip handlers, remove the dead
  `"project-calendar"`/`"main-calendar"` branches, add `"all-tasks"`/`"all-events"`
  branches (mirroring the existing `"tasks-list"` branch's shape).

**Stage 5 — Remove the calendar-list views**
- Remove routes, handlers (`main_calendar_list_page`, `list_main_calendar_rows`,
  `main_calendar_items_inner_html`, `MainCalendarListPageTemplate`,
  `MainCalendarListQuery`, `calendar_list_query`, and the `project_calendar.rs`
  equivalents), and their now-dead call sites identified above. Keep the grid,
  day-drawer, `calendar_row`, `toggle_*_item_complete`, and the legacy `/dashboard`
  redirects (retargeted in Stage 1).
- Delete `templates/main_calendar/page.html` /
  `templates/project_calendar/page.html` (list-page templates); trim the "List view"
  links out of both `calendar_page.html`s.
- Re-grep for `main_calendar_list_page|project_calendar_list_page|
  list_main_calendar_rows|list_calendar_rows_for_project|
  main_calendar_items_inner_html|calendar_items_inner_html|calendar_list_query` to
  confirm no leftover call sites.
- Verify whether `PRESETS`/`preset_range` in `main_calendar.rs` are list-only (delete)
  or also used by the grid's own date-window logic (keep) before removing.

**Stage 6 — Docs**
- Update `CLAUDE.md`'s Web UI section: `main_calendar.rs` is no longer the default
  landing/sole cross-project screen; note the two new screens/modules; note
  `ActiveContext::AllProjects`; note the list-view removal.
- Update `docs/issues_and_features.md`: move this entry to
  `docs/archived/archived_issues_and_features.md` once done; update the "Add a
  New-item button to the cross-project Home calendar" entry (line 53) to reference
  `/web/tasks`/`/web/events` instead of the old single calendar page, since that
  future work item is still open and now applies to two screens, not one.
- This plan doc (`docs/all-projects-landing-plan.md`) itself can be deleted or moved
  to `docs/archived/` once Stage 6 lands, matching this repo's convention for
  completed plan docs.

## Verification

No Playwright/browser click-through (per CLAUDE.md's UI-verification rule) — verify
with:
- `cargo build` after each stage (Askama embeds templates at compile time).
- `task web-styles` if any new Tailwind classes are introduced in the new templates.
- `cargo test` for the Rust suite.
- A careful read of the final diff against this plan's file list, plus the re-grep in
  Stage 5 to confirm no dangling references to removed functions/routes/templates.
- State explicitly to the user that live-in-browser behavior (sidebar rendering,
  actual Reschedule/Assign save styling on the new screens, mobile layout) was not
  confirmed in a browser and needs their own check.

## Progress log

(Append one entry per completed stage, newest last.)

**Stage 1 — done (2026-08-23).**
- `nav.rs`: added `ActiveContext::AllProjects`. `build_nav_html`'s new match arm builds
  `section_links` from a fixed `[(Tasks, "/web/tasks"), (Events, "/web/events")]` list (not
  `section_href`, which is project-scoped), `calendar_href = Some("/web/calendar")`,
  `activity_href = None`.
- `main_calendar.rs`: found only **two** `ActiveContext::None` call sites in this file, not
  three — `main_calendar_page` (grid) and `main_calendar_list_page` (flat list). There is no
  separate `build_nav_html` call in `main_calendar_day_fragment`; the plan's "grid page + day
  fragment" wording was inaccurate (the day fragment renders only the drawer partial, no nav).
  Both existing call sites switched to `ActiveContext::AllProjects`; nothing left on
  `ActiveContext::None` in this file.
- Deviation from the plan's route-skeleton instructions: chose the "stub minimally" option
  over "hold off entirely," and also went one step further — created the real Stage-2/3 module
  files now (`src/web_ui/all_projects_tasks.rs` with `all_projects_tasks_page`,
  `src/web_ui/all_projects_events.rs` with `all_projects_events_page`), each just
  `Html("TODO")`, registered in `src/web_ui/mod.rs` and routed as `GET /web/tasks`/
  `GET /web/events` in `build_web_router()` (`src/main.rs`). Rationale: keeps `/` and the new
  Home/logo links actually resolving (200, not 404) between stages, in case the app gets
  smoke-tested before Stage 2/3 land. Stage 2/3's implementer should edit these two existing
  files in place rather than creating new ones.
- **Not added**: the `PUT /web/tasks/projects/:project_id/items/:item_id` toggle route. No
  meaningful stub exists without the real list screen (nothing links to it yet), so this is
  deferred to Stage 2 in full, along with the real handler plan describes
  (`toggle_main_calendar_item_complete`-style). Note this when wiring Stage 2's routes.
- `src/main.rs`: retargeted both auth-mode branches' `.route("/", get(|| async {
  Redirect::to(...) }))` from `/web/calendar` to `/web/tasks`.
  `main_calendar::redirect_main_dashboard_list` retargeted from `/web/calendar/list` to
  `/web/tasks` (doc comment updated); `project_calendar::redirect_project_dashboard_list`
  retargeted from `.../calendar/list` to `.../projects/{project_id}/tasks` (doc comment
  updated). `redirect_main_dashboard`/`redirect_project_dashboard` (the non-`/list` legacy
  paths) were **left pointing at `/web/calendar`/`.../calendar`** — unchanged, correct, since
  the grid view itself isn't moving, only the default landing page is.
- `templates/nav.html`: header logo `href` `/web/calendar` → `/web/tasks`.
  `templates/nav_sidebar_inner.html`: bottom-block "Home" link `/web/calendar` → `/web/tasks`.
- Verified: `cargo build` clean (only pre-existing unrelated dead-code warnings, unchanged
  from before this stage). `cargo test`: 464 passed, 0 failed. Grepped for stray
  `"/web/calendar"` string literals afterward — the only three remaining are correct on
  inspection: `redirect_main_dashboard`'s own target (intentionally unchanged, see above),
  `nav.rs`'s new `AllProjects` `calendar_href` (intentionally still `/web/calendar`, that's
  the grid page), and `templates/main_calendar/page.html`'s internal nav link (the flat-list
  template itself, slated for deletion in Stage 5 — left as-is since editing dead-page-to-be
  markup isn't worth it).
- Not verified (no browser): actual sidebar rendering/highlighting for `AllProjects` context,
  and that `/web/tasks`/`/web/events` genuinely render `Html("TODO")` end to end — needs the
  user's own check per CLAUDE.md's UI-verification rule.
- **Next: Stage 2** — build the real `all_projects_tasks.rs` handler (replace the stub body),
  its `templates/all_projects_tasks/list_page.html`, and the
  `PUT /web/tasks/projects/:project_id/items/:item_id` toggle route/handler deferred from
  here.

**Stage 2 — done (2026-08-23).**
- `src/web_ui/all_projects_tasks.rs` (replaced the Stage 1 stub in place, as instructed) now has
  a real `all_projects_tasks_page` handler (`GET /web/tasks`) plus a new
  `toggle_all_projects_task_complete` handler wired as
  `PUT /web/tasks/projects/:project_id/items/:item_id` (`src/main.rs`, `build_web_router()`,
  right after the `/tasks` route).
- Row assembly is a new `list_all_projects_task_rows` function — duplicated (not shared) from
  `project_tasks::list_task_rows_for_project`'s gather shape per the plan's own precedent
  citation, looping `project_service::list_projects`, per project listing top-level
  (`parent_item_id: None`) items narrowed to `ItemKind::Task` via
  `project_item_service::list_project_items_unchecked`, plus each project's current
  non-materialized Task-series occurrences via
  `item_series_service::list_occurrence_states_for_project` (`is_current` + `!Materialized`
  filter, mirroring `project_tasks_page`'s own query). Real items and virtual occurrences are
  merged into one `Vec<(i64 timestamp, String html)>`, sorted by timestamp, same pattern
  `render_rows_with_virtual`/`list_main_calendar_rows` already use — this *is* sorted by due
  date across every project (a single flat list), which does not conflict with the plan's "no
  flattening-by-date" line — that line contrasts with `main_calendar`'s merged-Task+Event/
  type-badge shape, not with per-project's own due-date ordering; see the new row builder's own
  doc comment for the reasoning spelled out where a future stage's implementer will actually see
  it.
- `task_included(is_team_project, assigned_to, user_id)`: a narrowed, Task-only duplicate of
  `main_calendar::is_included` (that function's `Event`/`Simple`/`Template` arms have no analog
  here — this screen's own gather loop already filters to `ItemKind::Task` before this predicate
  ever runs). **Deviation from the plan**: no `assigned_to_any` toggle was added — the plan
  explicitly left it optional ("or `assigned_to_any` if a toggle is added"); omitted to keep this
  stage's scope minimal, matching the flat calendar list's own pre-Stage-4 behavior. If a future
  stage adds this toggle, it's a straightforward extra `bool` parameter mirroring
  `main_calendar::is_included`'s own.
- New row builder `all_projects_task_row(...)`: `ProjectTaskRow::from_item(...)` (empty
  `siblings: &[]`, matching `main_calendar::calendar_row`'s own choice) +
  `row.project_name = Some(project_name)` + `complete_url` rewritten to
  `/web/tasks/projects/{project_id}/items/{item_id}` (this stage's new toggle route, not the
  per-project one — so the toggle's own response can re-render through this same function and
  keep its `project_name` tag/cross-project URLs) + `reschedule_url`/`assign_url` each suffixed
  `?view=all-tasks`. That suffix is inert today — `project_tasks::normalize_row_view` only
  recognizes `"project-calendar"`/`"main-calendar"` until Stage 4 adds `"all-tasks"` — so a
  Reschedule/Assign save from this screen currently re-renders via the plain `ProjectTaskRow`
  arm (losing the `project_name` tag until Stage 4 lands), exactly the transient gap the plan
  anticipated by putting that wiring in its own stage. No `parent_name`/`type_badge` set (this
  screen lists only top-level items — see above — and is Task-only already), matching the plan's
  explicit "simpler than `main_calendar::calendar_row`" framing.
- New template `templates/all_projects_tasks/virtual_row.html` + backing struct
  `AllProjectsTaskVirtualRow` (in `all_projects_tasks.rs`, not a separate `templates.rs` — this
  module has exactly one non-page template, unlike `project_tasks`'s multi-template split) —
  needed because `project_tasks::templates::ProjectTaskVirtualRow` has no `project_name` field
  to tag a cross-project row with; mirrors `main_calendar::MainCalendarVirtualRow` minus the
  type symbol/label (single-kind screen) and minus `in_list_view` (no in-place list rebuild
  target exists yet for this screen — deferred to Stage 4, which is where `project_tasks`'s own
  `"tasks-list"`-style rebuild branch would need an `"all-tasks"` counterpart; until then this
  virtual row's checkbox/Skip/Unskip fall back to default whole-page htmx behavior, same as the
  calendar day panel's `in_list_view: false` rows).
- `templates/all_projects_tasks/list_page.html`: mirrors `project_tasks/list_page.html` minus
  the "Up to projects" link (doesn't apply at the all-projects top level) and the "+ New Task"
  button (explicitly out of scope per the plan's Context section) — just the title, a "Show
  completed" checkbox (`hx-get="/web/tasks"`), and `#items-list`.
- `components/row.rs`'s `Row::project_name` doc comment updated to name both new cross-project
  screens alongside `main_calendar`, since it's no longer the only screen setting this field.
- Verified: `cargo build` clean (same pre-existing unrelated dead-code warnings as Stage 1, no
  new ones). `cargo test`: 464 passed, 0 failed (unchanged count — no new tests added; this
  stage's new code is handler/template wiring with no pure functions novel enough to warrant a
  unit test beyond what `project_tasks`/`main_calendar`'s own equivalents already cover by
  precedent).
- Not verified (no browser): actual sidebar highlighting on `/web/tasks`, row rendering/layout,
  the toggle checkbox's live round-trip, and virtual-row Skip/Unskip/materialize behavior on this
  screen specifically — needs the user's own check per CLAUDE.md's UI-verification rule.
- **Next: Stage 3** — build `all_projects_events.rs` (replace its Stage 1 stub) +
  `templates/all_projects_events/list_page.html`, same shape as this stage minus completion/
  assignment/points (Events aren't completable). No toggle route needed for Events. Stage 2's
  `all_projects_task_row`/`list_all_projects_task_rows`/`AllProjectsTaskVirtualRow` in
  `all_projects_tasks.rs` are a solid template to mirror structurally (swap `ProjectTaskRow` for
  `ProjectEventRow`, drop the `complete_url`/toggle-route machinery entirely, drop
  `task_included`'s Task-only restriction since Events are always included per
  `main_calendar::is_included`'s `Event => true` arm).

**Stage 3 — done (2026-08-23).**
- `src/web_ui/all_projects_events.rs` (replaced the Stage 1 stub in place) now has a real
  `all_projects_events_page` handler (`GET /web/events`, already routed since Stage 1 — no route
  change needed here). No toggle route — Events are never completable
  (`ProjectEventRow::from_item` already hardcodes `complete_url: None`), matching the plan.
- Row assembly is a new `list_all_projects_event_rows` function, structurally mirroring Stage 2's
  `list_all_projects_task_rows`: loops `project_service::list_projects`, per project lists
  top-level items narrowed to `ItemKind::Event` via `project_item_service::
  list_project_items_unchecked`, plus every still-virtual/skipped Event-series occurrence in
  `project_events::virtual_occurrence_window(Utc::now())`'s forward window (90 days) via
  `item_series_service::list_occurrence_states_for_project`, filtered to `item_type ==
  ItemKind::Event` and `!Materialized`. **Deviation from Stage 2's precedent**: reused
  `project_events::virtual_occurrence_window` directly (it's `pub(crate)`) rather than
  duplicating it — unlike the per-screen row/predicate helpers, it's a pure constant-window
  function with nothing screen-specific to diverge on, so duplicating it would just be a stale-
  copy risk with no benefit. Unlike Stage 2's Task rows (only the series' *current* occurrence),
  every occurrence in the window is included here — matching `project_events`'s own
  `render_rows_with_virtual` precedent, since an Event-typed series has no cursor/"current"
  concept at all (see `ItemSeries::cursor_date`'s doc comment, and `ProjectEventVirtualRow`'s own
  doc comment making the same point).
- No `task_included`-style predicate at all: `main_calendar::is_included`'s `Event => true` arm
  means Events are never assignment-restricted, so (unlike Stage 2's Task screen) this loop has
  no inclusion filter beyond the `ItemKind::Event` retain.
- New row builder `all_projects_event_row(...)`: `ProjectEventRow::from_item(...)` +
  `row.project_name = Some(project_name)` + `reschedule_url` suffixed `?view=all-events`. No
  `complete_url` rewrite (already `None` for every Event) and no `assign_url` rewrite (`Row::
  assign_url` is already `None` for every `ProjectEventRow`, per that struct's own doc comment —
  Events never carry assignment). Sort key mirrors `project_events::sort_key`:
  `scheduled_date().or(due_date())`, undated last.
- New template `templates/all_projects_events/virtual_row.html` + backing struct
  `AllProjectsEventVirtualRow` (in `all_projects_events.rs`, same one-non-page-template-per-
  module shape Stage 2 used) — mirrors `project_events::templates::ProjectEventVirtualRow` plus
  a `project_name` tag, rendered as a small pill span (same markup pattern
  `all_projects_tasks/virtual_row.html` already uses for its own `project_name` pill). No
  `is_current`/assignee fields, matching `ProjectEventVirtualRow`'s own shape.
- `templates/all_projects_events/list_page.html`: mirrors `project_events/list_page.html` minus
  "Up to projects", "Manage Google Calendars", and "+ New Event" (all out of scope/inapplicable
  at the all-projects level) — just the title and `#events-list`. No "Show completed" checkbox
  either (Events have no `complete` concept at all, unlike Stage 2's Tasks screen).
- Verified every Tailwind class used in both new templates is already present in an existing
  compiled template (`project_events/list_page.html`, `project_events/virtual_row.html`,
  `all_projects_tasks/virtual_row.html`) via a `comm`/`diff` check before building — `task
  web-styles` was **not** run since nothing new needed compiling in.
- Verified: `cargo build` clean (same pre-existing unrelated dead-code warnings as Stages 1-2, no
  new ones). `cargo test`: 464 passed, 0 failed (unchanged count, same rationale as Stage 2 — no
  new pure functions novel enough to warrant a dedicated unit test).
- Not verified (no browser): actual sidebar highlighting on `/web/events`, row rendering/layout,
  and virtual-row Skip/Unskip/materialize behavior on this screen specifically — needs the user's
  own check per CLAUDE.md's UI-verification rule.
- **Next: Stage 4** — wire the shared save/skip/unskip handlers for the `"all-tasks"`/
  `"all-events"` row views: `project_tasks/mod.rs::normalize_row_view` needs to accept
  `"all-tasks"` (and, per Stage 4's own listed scope, `project_events/handlers.rs` reuses that
  same `normalize_row_view` — confirm whether `"all-events"` needs a matching arm there or a
  separate one); `project_tasks/handlers.rs::update_project_task_form` needs a `Some("all-tasks")`
  row-render arm plus the `complete_project_item_series_occurrence_form` rebuild branch;
  `project_events/handlers.rs::update_project_event_form` needs an `"all-events"` arm (see its
  existing `if view == "main-calendar" { ... } else { ... }` branch around line 606 — this needs
  a third case, or the two-screen dispatch there needs restructuring to cover all four
  `view` values: `project-calendar`/`main-calendar`/`all-tasks`/`all-events`); and
  `project_item_series/handlers.rs`'s skip/unskip handlers need the dead `"project-calendar"`/
  `"main-calendar"` branches removed and `"all-tasks"`/`"all-events"` branches added. Until this
  lands, a Reschedule/Assign save from either new cross-project screen re-renders via the plain
  `ProjectTaskRow`/`ProjectEventRow` shape (losing the `project_name` tag transiently) — this is
  the exact, expected gap both Stage 2's and this stage's progress notes above already flagged.
