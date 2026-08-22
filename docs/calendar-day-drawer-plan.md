# Calendar day-drawer + calendar-as-default view

## Context

Today, four screens have a month-grid calendar view as an *alternate* view next to a list view: the cross-project Home dashboard (`main_dashboard.rs`, `/web/dashboard` list vs. `/web/dashboard/calendar`), the per-project dashboard (`project_dashboard.rs`), the per-project Tasks screen (`project_tasks/`), and the per-project Events screen (`project_events/`). Each calendar page is a month grid (`calendar_page.html`) with a plain `<div id="calendar-day-list">` sitting *below* the grid; clicking a day cell does an htmx `hx-get` that swaps only that div's contents (`calendar_day_panel.html`), not the grid itself.

This plan:

1. **Makes the calendar the default/primary view** for the two screens that keep one at all — the cross-project Home dashboard and the per-project dashboard (see point 3).
2. **Replaces the below-grid day panel with a slide-in drawer** (Tailwind Plus's `overlays/drawers/08-contact-list-example.html`, available locally at `~/tailwind-ui-html/html/overlays/drawers/08-contact-list-example.html`), with its own prev/next-*day* arrows (distinct from the grid's prev/next-*month* arrows) and an All/Tasks/Events tab bar plus an "assigned to me" toggle.
3. **Retires the per-project Tasks and Events calendar views entirely.** Once the per-project dashboard's drawer has All/Tasks/Events tabs, it's a strict superset of what the Tasks-only and Events-only calendars showed — a day's full task+event list, filterable down to just one type. Tasks and Events keep their **list** view only; the sidebar links for Tasks/Events go straight to the (unchanged) filterable list, with no calendar toggle anymore.
4. **Fixes the selected-day highlight to update client-side** on every day click / drawer arrow click, without ever re-fetching or re-rendering the month grid.

None of this retires the Home/per-project dashboard **list** pages, `assigned_items.rs` ("assigned to me" cross-project list), or the Events child-item sub-routes (`.../events/:id/children`) — all raised and explicitly deferred during planning; see "Explicitly out of scope" below.

This plan also folds in three related, already-written entries from `docs/issues_and_features.md` that the user asked to include because they intersect directly with this work:

- *"Need button on each row with vertical ellipses icon that gets dialog with row actions (reschedule/duplicate/delete/assign/move/edit/etc.) — `#action-dialog` in `base.html`."* — the Tailwind Plus drawer example's own row markup has exactly this (an `el-dropdown`/`el-menu` "•••" button per row), which is what prompted revisiting it now. See Stage 1.
- *"Events list rows still lack the 'duplicate' action... Fold into a vertical-ellipses menu alongside edit/delete/assign once that exists."* — small parity add-on inside Stage 1, once the menu exists.
- *"Clicking cell in calendar will render the correct day but the highlighted cell doesn't update properly for some reason."* — this is the exact bug Point 4 above fixes; resolved by Stage 2.
- *"Calendar should have same filter as list - assigned to user should be default, can select assigned to any."* — implemented in Stages 3/4.

Once this plan is fully implemented, move those four bullets out of `docs/issues_and_features.md` (into `docs/archived/archived_issues_and_features.md`), and reconsider the remaining "Let's shrink the calendar view by at least 50%... put the month beside the list [on desktop]" bullet — the "put list beside grid" half is moot once the list is a drawer overlay rather than page content competing for space; the "shrink the grid" half is still a legitimate, independent, small follow-up worth keeping open.

**Deferred, final-stage rename:** the user wants the per-project and cross-project "Dashboard" screens renamed to "Calendar" (nav label, page titles, etc.) once this is otherwise complete — see Stage 7. Whether that also means renaming the URL paths (`/dashboard` → `/calendar`) or just the display text is intentionally left open until that stage starts (see Stage 7's own note).

### Explicitly out of scope (raised, then deliberately deferred by the user while planning)

- **Not retiring** the Tasks list page or the Events list page — only their *calendar* views (Stage 5). Both list pages keep every bit of their current functionality (filters, New button, row actions), unchanged.
- **Not retiring** either dashboard's **list** page — calendar becomes the *default* landing view (Stage 6), but the list stays reachable at an explicit URL, unchanged in functionality.
- **Not retiring** `assigned_items.rs` (`/web/assigned-items`), even though the dashboard calendars gain their own "assigned to me" toggle in this plan. Both can coexist.
- **Not touching** `.../events/:id/children` (an Event's own sub-item management) — a detail-page concept, unrelated to this list/calendar work.
- **Not a CLI/MCP change** — `prl` and the MCP server have no calendar-rendering concept at all; nothing here needs a `todo-cli`/`mcp-server` touch-point.

### Confirmed design decisions

- **All/Tasks/Events tabs and the assigned-to-me toggle live on the two dashboard calendars only** (there's no other per-project calendar left once Stage 5 retires the Tasks/Events ones, and the cross-project Home calendar is dashboard-shaped by definition).
- **The month grid's per-day tally badge is always unfiltered** (counts everything, regardless of the drawer's active type-tab). Only the drawer's own list body reflects the type-tab filter. This avoids coupling a same-day tab click to a month-wide re-render/re-fetch, and keeps the "how much is on this day, roughly" badge simple. The **assigned-to-me toggle**, unlike the type tabs, *does* reload the whole calendar page (full month tally + drawer both become "mine"-scoped) — it's a rarer action than clicking through days, matching how the existing list views already reload fully when `assignedToAny`/`showComplete` change, so no new partial-reload architecture is needed for it.
- **New-item creation**: the per-project dashboard calendar gains a small "New" button + Task/Event type picker (Stage 3), since it's about to become the primary way to browse a project's tasks/events by day, and today's dashboards have no creation entry point at all. The Tasks/Events list pages keep their own existing "New task"/"New event" buttons unchanged. The **cross-project Home** calendar does *not* get a New button — item creation is inherently project-scoped and Home has no single natural project to create into.
- **The row-actions "⋮" menu (Stage 1) reuses the existing `#action-dialog` convention**, not the Tailwind Plus example's `<el-dropdown>`/`<el-menu>` custom elements. This app has zero existing usage of `@tailwindplus/elements` anywhere (confirmed by grep) — `error_dialog.html`'s reference to "`el-dialog` primitives" is borrowing that component family's *Tailwind transition classes* (`data-closed:`, `data-enter:`, etc.) onto this app's own plain native `<dialog>`, not loading the actual JS library. Introducing the real custom-element library would mean a new CDN script dependency this app has deliberately avoided everywhere else (see `CLAUDE.md`'s "no browser-side SPA" architecture note); a native `<dialog>` swapped via `#action-dialog` gives the same "click ⋮, small menu of actions appears" UX without one.
- **Default-routing swap (Stage 6) keeps the old `.../calendar` URL alive as a redirect to the new base path**, rather than deleting it — cheap, and avoids breaking anything that already links directly to `.../calendar` (bookmarks, the sidebar's own historical links if any survive an incomplete audit).

## Why staged, and what's in scope per stage

Same process as `docs/archived/google-calendar-import-plan.md` / `docs/archived/project-abstraction-plan.md`: each stage is independently landable, leaves the app's running behavior unchanged (or only additively changed) until wired up, and is done in its own session — **compact/clear context between stages**, with this file as the only thing that survives the handoff. Before ending a stage, update that stage's section below with an **Implementation notes** entry: exact names if they ended up different, deviations and why, test/build status, anything discovered that changes a later stage's assumptions.

**"Independently landable" describes the code, not a standing instruction to `git commit`.** Per this repo's global CLAUDE.md/git policy, commits only happen when the user explicitly asks — the user's own framing of this task ("we'll commit and clear context between stages and only push when complete with the entire plan") is that explicit ask, scoped to this plan: commit at the end of each stage, never push until every stage is done and the user separately confirms.

1. **Row actions → a "⋮" menu.** Touches the shared `templates/components/row.html` (already used by every list *and* every existing day panel), so it's foundational and worth landing before the drawer work below inherits it. No behavior change beyond consolidating icons into one menu; Events gains "duplicate" parity as a small included add-on.
2. **Drawer shell, prev/next-day arrows, and client-side selected-day highlight — proven on the per-project Dashboard calendar** (chosen as the sole per-project calendar going forward, and already mixed-type, so the drawer mechanism is proven against both Task and Event rows in one pass — no throwaway work on a screen that's about to be retired). No filtering yet in this stage; the drawer shows exactly what today's day panel shows, just in the new shell.
3. **Add All/Tasks/Events tabs, the assigned-to-me toggle, and a New-item button** to the now-drawer-equipped per-project Dashboard calendar.
4. **Port the drawer (+ tabs + toggle, no New button) to the cross-project Home Dashboard calendar.**
5. **Retire the Tasks and Events calendar views.** Delete their `calendar_page.html`/`calendar_day_panel.html` templates, handlers, and routes; remove the "Calendar view" link from both list pages; audit for any other reference to `.../tasks/calendar` or `.../events/calendar` before deleting.
6. **Default-view routing swap**, for the two screens that still have both a list and a calendar view (Home dashboard, per-project dashboard): the base URL now serves the calendar page; the list view moves to an explicit `.../list` route; old `.../calendar` URLs redirect to the base path.
7. **Rename "Dashboard" to "Calendar"** (nav label, page titles, and possibly the URL paths — see the stage's own note) — deliberately last, per the user's explicit "defer that rename til the very end."

---

## Stage 1 — Row actions consolidated into a "⋮" menu

`templates/components/row.html` currently renders, inline, whichever of these `Option<String>` URL fields the caller supplied: `reschedule_url` (calendar icon → `hx-get` into `#action-dialog`), `assign_url` (same pattern), `duplicate_url` (`hx-post` with `hx-confirm`), plus an unconditional-if-`!is_imported` delete button (`hx-delete`) and a `skip_url` "Skip" text button for series occurrences. Replace all of these (delete/skip excepted — see below) with a single trailing "⋮" (vertical-ellipsis) button per row that `hx-get`s a small actions-menu fragment into `#action-dialog`, following the exact mechanism `reschedule_url`/`assign_url` already use today (so the *fetch* mechanism is unchanged — only what's fetched, and when, changes).

**New menu fragment** (`templates/components/row_actions_menu.html`, new): a small list of action links/buttons inside `#action-dialog`'s existing shell, visually modeled on the Tailwind Plus example's `el-menu` popover styling (rounded panel, `divide-y`, hover states) but built from plain markup, not the `<el-menu>` custom element (see Confirmed design decisions above). Rows:

- **Edit** — plain link to the item's edit route (where one exists), closes the dialog by navigating away.
- **Reschedule** — present iff `reschedule_url.is_some()`; `hx-get`s that URL into `#action-dialog` itself (same target, swaps the menu for the existing reschedule form — no new plumbing needed, `#action-dialog`'s content is just replaced).
- **Assign** — present iff `assign_url.is_some()`; same swap-in-place pattern.
- **Duplicate** — present iff `duplicate_url.is_some()`; `hx-post` with the existing `hx-confirm`, same as today.
- **Delete** — always present unless `is_imported`; `hx-delete` with the existing `hx-confirm`, targeting `#item-{id}` exactly as today (not `#action-dialog` — deleting closes the dialog *and* removes the row in one swap; needs `hx-on::after-request` or a small script to also call `action-dialog.close()` after a successful delete, since the delete's own `hx-target`/`hx-swap` isn't the dialog).

**`skip_url`'s "Skip" button is left where it is, outside the menu** — it's a frequent, single-click action for a recurring occurrence (not a rare/destructive one like the others), and burying it a click deeper would be a regression, not a consolidation. Same reasoning for the **complete checkbox** — untouched, stays inline.

**Events duplicate parity** (the `docs/issues_and_features.md` add-on): `src/web_ui/project_events/templates.rs`'s `ProjectEventRow::from_item` currently hardcodes `duplicate_url: None`. Wire it up the same way `ProjectTaskRow` already does (needs a `duplicate_project_event`-shaped handler + route mirroring `project_tasks`'s existing `/tasks/:id/duplicate`, if one doesn't already exist under a different name — confirm during this stage before assuming a new handler is needed).

**Row width**: with reschedule/assign/duplicate/delete collapsed into one "⋮" button, `row.html`'s per-row markup shrinks from up to 5 trailing icon buttons to at most 2 (Skip, ⋮) plus the checkbox — worth double-checking the row still reads well at the day-drawer's narrower width (the drawer panel is `max-w-md`, notably narrower than a full list page) once Stage 2 exists to test it against, but the row markup itself doesn't need to know anything about being inside a drawer vs. a list page.

**Files touched:** `templates/components/row.html`, `templates/components/row_actions_menu.html` (new), `src/web_ui/project_events/templates.rs` + `handlers.rs` + `mod.rs` (duplicate parity, if not already present), `templates/base.html` (small JS addition: closing `#action-dialog` after a successful delete triggered from inside it).

**Verification:** `cargo build`, `task web-styles`; live click-through (via the `run` skill or Playwright) on at least one Tasks row and one Events row confirming: ⋮ opens the menu, Reschedule/Assign swap the dialog content in place, Duplicate still confirms and duplicates, Delete still confirms, removes the row, and closes the dialog, Skip and the checkbox are unaffected and still work directly from the row.

### Implementation notes (fill in before ending this stage)

---

## Stage 2 — Drawer shell, day arrows, and client-side highlight — proven on the per-project Dashboard calendar

**Drawer shell** (`templates/project_dashboard/calendar_page.html` + a restyled `templates/project_dashboard/calendar_day_panel.html`): replace the plain `<div id="calendar-day-list">` with a persistent `<dialog id="day-drawer">`, following this app's existing `#action-dialog`/`#error-dialog` convention exactly (a stable-id `<dialog>` present in the page's own markup — not `base.html`, since the drawer is calendar-page-specific — whose *innerHTML* is what gets swapped by htmx; JS opens it via `showModal()` on swap, mirroring the existing `htmx:afterSwap` listener in `base.html` that already does this for `#action-dialog`/`#error-dialog`). Markup/transition classes adapted from `~/tailwind-ui-html/html/overlays/drawers/08-contact-list-example.html` (the `el-dialog-panel`-style slide-in-from-the-right treatment, `data-closed:translate-x-full`, etc. — same Tailwind transition-class family `error_dialog.html` already borrows, not the custom element itself).

Day-cell buttons' `hx-target`/`hx-select` change from `#calendar-day-list` to the drawer's inner content wrapper; the `.../dashboard/calendar/day?date=...` fragment route is unchanged in URL shape but now renders drawer-shaped content (header with the date label + close button + prev/next-day arrows, then the row list body — still unfiltered by type in this stage, exactly matching what `day_list_rows` renders today).

**Prev/next-*day* arrows** (new, distinct from the grid's existing prev/next-*month* arrows): rendered by the day-fragment handler itself (`project_dashboard_calendar_day_fragment` in `src/web_ui/project_dashboard.rs`), which already knows the requested `date` — compute `date - 1 day`/`date + 1 day` server-side and render two arrow buttons at the top of the drawer, each `hx-get`ing the same fragment route with the adjacent date, targeting the same drawer content wrapper. This mirrors the month-nav arrows' own already-established shape (plain `<a>`/`hx-get` pair either side of a label) so no new client-side date math is needed anywhere.

**Client-side selected-day highlight (fixes the `docs/issues_and_features.md` bug directly)**: give every day-cell button in the month grid a `data-date="{{ day.date }}"` attribute (it doesn't have one today). Add one generic listener in `base.html` (alongside `toggleSidebar()`'s precedent): on `htmx:afterSwap`, if the swapped-in root element carries a `data-date` attribute (the day-drawer fragment's own root gets one — this is what both a day-cell click *and* a drawer prev/next-day-arrow click end up triggering, since both ultimately swap the same fragment), find the calendar grid's own day-cell button matching that date (`[data-date="..."]` within the current page's grid) and toggle the highlight classes (`ring-2 ring-inset ring-indigo-500`) onto it, removing them from whichever cell currently has them. One listener, reusable unmodified by Stage 4's port to the Home dashboard.

This is a pure client-side class toggle — it never re-fetches or re-renders the month grid itself, satisfying the plan's "without re-rendering the entire month view" requirement directly.

**Files touched:** `templates/project_dashboard/calendar_page.html`, `templates/project_dashboard/calendar_day_panel.html`, `src/web_ui/project_dashboard.rs` (`project_dashboard_calendar_page`/`project_dashboard_calendar_day_fragment`, prev/next-day date computation, `data-date` on the fragment root), `templates/base.html` (the new generic highlight-updater listener), `styles/input.css` if any new utility combination needs registering, `task web-styles`.

**Verification:** `cargo build`, `task web-styles`; live click-through: open the drawer from a day cell, confirm the highlight moves there; use the drawer's own prev/next-day arrows repeatedly and confirm the month-grid highlight tracks each move; confirm via browser dev-tools network tab that none of this ever issues a request to the month-page route itself (only `.../calendar/day` fragment requests); exercise a checkbox toggle on both a Task and an Event row, a Skip, and the Stage 1 "⋮" menu's Reschedule/Assign (dialog-inside-a-dialog case) from a row *inside* the open drawer, confirming both dialogs stack/close correctly and don't fight each other's `htmx:afterSwap`-driven auto-open behavior.

### Implementation notes (fill in before ending this stage)

---

## Stage 3 — Tabs, assigned-to-me toggle, and New-item button on the per-project Dashboard calendar

**All/Tasks/Events tabs**: rendered in the drawer header below the date label/arrows, styled on the Tailwind Plus example's tab markup (`border-indigo-500 text-indigo-600` for the active tab, `border-transparent text-gray-500 hover:...` for inactive — copied classes, no new component). Each tab `hx-get`s the same day-fragment route with an added `?type=all|task|event` query param, targeting a *narrower* wrapper than the full drawer content (`#day-drawer-list`, say) so switching tabs doesn't re-fetch/re-render the date label or arrows — just the row list. `project_dashboard_calendar_day_fragment` gains a `type: Option<String>` query param, applied as a filter over `day_list_rows`'s existing item-kind data (`ItemKind::Task`/`Event`) before rendering. Per the Confirmed design decisions above, this filter **does not** affect the month grid's per-day tally.

**Assigned-to-me toggle**: mirrors `project_dashboard.rs`'s existing list-view `assigned_to_any: Option<String>` convention exactly (`None`/absent = filtered to the requester; present = show everyone's). Default is "mine" (matching `docs/issues_and_features.md`'s explicit spec and the list view's existing default). Unlike the type tabs, this toggle is a plain link/form that reloads the **whole calendar page** (`project_dashboard_calendar_page`'s own `CalendarQuery` gains `assigned_to_any: Option<String>`, threaded into `build_calendar_days`'s tally computation the same way `project_dashboard_list_rows`'s list-view filtering already threads it) — so the month tally and the drawer's default (unfiltered-by-type) view are both consistently "mine" or "everyone's" after toggling.

**New-item button**: a "New" button in the calendar page's header opening a tiny inline type picker (Task / Event — two links, no new dialog needed, just two `<a href="{{ ... }}/tasks/new">`/`.../events/new` links styled as a small dropdown or a two-button group) — reuses the existing, unchanged `/tasks/new`/`/events/new` create-form routes; no prefill of the selected day in this first pass (flag as a nice-to-have deferred, not required for this stage).

**Files touched:** `templates/project_dashboard/calendar_page.html`, `templates/project_dashboard/calendar_day_panel.html`, `src/web_ui/project_dashboard.rs` (`CalendarQuery`, `build_calendar_days`, `day_list_rows`, both calendar handlers).

**Verification:** `cargo build`, `task web-styles`; live click-through: tabs filter the drawer body only (network tab confirms no month-page refetch); assigned-to-me toggle changes both the tally and the drawer and does reload the page; New button reaches both create forms.

### Implementation notes (fill in before ending this stage)

---

## Stage 4 — Port the drawer (+ tabs + toggle) to the cross-project Home Dashboard calendar

Same shape as Stages 2–3 combined (drawer port + tabs + assigned-to-me toggle), applied to `templates/main_dashboard/calendar_page.html` + `calendar_day_panel.html` + `src/web_ui/main_dashboard.rs`'s `main_dashboard_calendar_page`/`main_dashboard_calendar_day_fragment`/`build_calendar_days`/`day_list_rows`/`gather_calendar_data`. No New-item button here (see Confirmed design decisions above). One extra wrinkle Stage 3 didn't have: `main_dashboard.rs` already applies its own baked-in assignment restriction independent of any toggle (per `CLAUDE.md`'s Web UI section: "Tasks show unrestricted on a personal project but only assigned to requester on team-backed one") — confirm during this stage exactly how the new toggle composes with that existing baked-in rule (most likely: the toggle only relaxes the team-backed-project restriction when set to "everyone," and has no effect on personal-project tasks, which were never restricted to begin with) rather than assuming they're independent of each other.

**Files touched:** `templates/main_dashboard/calendar_page.html`, `templates/main_dashboard/calendar_day_panel.html`, `src/web_ui/main_dashboard.rs`.

**Verification:** `cargo build`, `task web-styles`; live click-through mirroring Stage 3's, plus a specific check that the assigned-to-me toggle behaves sensibly across a mix of a personal project and a team-backed project's items in the same cross-project view.

### Implementation notes (fill in before ending this stage)

---

## Stage 5 — Retire the Tasks and Events calendar views

Now that the per-project dashboard's drawer (Stage 3) shows a day's tasks and events together, filterable to just one type via the All/Tasks/Events tabs, the Tasks-only and Events-only calendars are redundant — delete them rather than maintaining three overlapping day-browsing UIs per project.

- Delete `templates/project_tasks/calendar_page.html`, `templates/project_tasks/calendar_day_panel.html`, and their handlers (`project_tasks_calendar_page`, `project_tasks_calendar_day_fragment`) + route registrations in `src/main.rs`. Same for `project_events/calendar_page.html`, `calendar_day_panel.html`, `project_events_calendar_page`, `project_events_calendar_day_fragment`.
- Remove the "Calendar view" link from `templates/project_tasks/list_page.html` and `templates/project_events/list_page.html` (the Tasks/Events list pages otherwise keep every bit of their current functionality, unchanged).
- **Audit before deleting**: grep the whole repo (templates, handlers, `docs/`) for any reference to `.../tasks/calendar` or `.../events/calendar` that isn't the two things above — e.g. confirm no reschedule/quick-catchup flow, no "New task" success-redirect, and no `nav.rs`/sidebar entry depends on either route surviving.
- `build_calendar_days`/`day_list_rows`/related helper functions in `project_tasks/mod.rs` and `project_events/mod.rs` may become entirely dead code once their only callers (the deleted handlers) are gone — remove them too rather than leaving unused code behind, unless something else in either module still calls them (double-check; don't assume).

**Files touched:** `templates/project_tasks/calendar_page.html` (deleted), `templates/project_tasks/calendar_day_panel.html` (deleted), `templates/project_events/calendar_page.html` (deleted), `templates/project_events/calendar_day_panel.html` (deleted), `src/web_ui/project_tasks/handlers.rs` + `mod.rs` + `templates.rs`, `src/web_ui/project_events/handlers.rs` + `mod.rs` + `templates.rs`, `src/main.rs` (route removals), `templates/project_tasks/list_page.html`, `templates/project_events/list_page.html`.

**Verification:** `cargo build` (confirm no leftover dead-code warnings beyond expected ones, no broken template references), `task check`; live click-through confirming the Tasks and Events list pages work exactly as before minus the now-removed "Calendar view" link, and that `.../tasks/calendar`/`.../events/calendar` return a clean 404 rather than a broken page.

### Implementation notes (fill in before ending this stage)

---

## Stage 6 — Default-view routing swap (calendar becomes the default for the two remaining calendar screens)

For each of the two screens that still have both a list and a calendar view (Home dashboard, per-project dashboard):

1. Move the existing list handler to an explicit `.../list` route (e.g. `/web/projects/:project_id/dashboard/list`), keeping its own logic untouched.
2. Make the existing calendar handler serve the *base* path (e.g. `/web/projects/:project_id/dashboard`) instead of `.../calendar`.
3. Keep the old `.../calendar` path alive too, as a redirect to the base path (per the Confirmed design decisions above) — cheap insurance against any stale link.
4. Update the calendar page's "List view" link to point at the new `.../list` path; update the list page's "Calendar view" link to point at the (now-default) base path instead of `.../calendar`.
5. **Audit every internal `Redirect::to(...)`/form-post/link across both modules that currently targets the base path**, since the base path's *meaning* just changed from "the list" to "the calendar." A handler that redirects back to the base path after a mutation (e.g. after toggling a dashboard row's completion) almost certainly means "go back to the list I was on" and must be updated to target `.../list` explicitly, not silently start landing users on the calendar instead. This is the stage's single highest-risk item — grep each module for every reference to its own base path (not just the obvious redirect helpers) before considering the stage done.
6. `src/web_ui/nav.rs`'s `section_href`/sidebar links, the header logo link, and `GET /`'s redirect target already point at each section's base path — confirm (don't assume) that none of them hard-code a `.../calendar` or `.../list` suffix that would need updating too.

**Files touched:** `src/main.rs` (route table for both screens), `src/web_ui/main_dashboard.rs`, `src/web_ui/project_dashboard.rs`, `templates/main_dashboard/*.html`, `templates/project_dashboard/*.html`, `src/web_ui/nav.rs` (if the audit in step 6 finds anything).

**Verification:** `cargo build`, `task check`; live click-through of both screens' base URLs now landing on the calendar; both list pages still reachable via their new `.../list` link; both old `.../calendar` URLs still work (redirects); every mutation (complete/skip/delete/duplicate/reschedule/assign, from both the list view and the new drawer) still redirects/reloads to the page the user was actually on, never silently bouncing them to the other view.

### Implementation notes (fill in before ending this stage)

---

## Stage 7 — Rename "Dashboard" to "Calendar"

Deliberately last, per the user's explicit "defer that rename til the very end" — and deliberately open-ended until this stage actually starts. At minimum: nav sidebar label, page `<title>` blocks, and any user-facing "Dashboard" wording across `templates/main_dashboard/*.html` / `templates/project_dashboard/*.html` become "Calendar". **Confirm with the user at the start of this stage** whether it's display-text-only, or also a full route/module rename (`/web/dashboard` → `/web/calendar`, `main_dashboard.rs`/`project_dashboard.rs` module names, `MainDashboardRow`/`ProjectDashboardRow` struct names, etc.) — the latter is a much larger blast radius (every internal link, every redirect target from Stage 6, `nav.rs`, this plan's own file, `CLAUDE.md`'s Web UI section) for comparatively low functional value, so don't assume it's wanted without asking.

### Implementation notes (fill in before ending this stage)

---

## After this plan is complete

- Move the four folded-in bullets (row-actions ellipsis menu, Events-duplicate-parity, calendar highlight bug, calendar assigned-filter) out of `docs/issues_and_features.md` into `docs/archived/archived_issues_and_features.md`.
- Revisit the remaining "shrink the calendar view... put the month beside the list" bullet — reword or split it, since the drawer resolves the layout-competition problem it was originally about; the "shrink the grid" half may still be worth doing independently.
- Move this file itself (`docs/calendar-day-drawer-plan.md`) into `docs/archived/`.
