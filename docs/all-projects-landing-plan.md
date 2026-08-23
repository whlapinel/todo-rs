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
