# New/edit item forms as dialogs, all-projects creation, and the canonical Personal project

Status: **planned, not yet implemented**. Written 2026-08-23 from the first entry in `docs/issues_and_features.md`, which bundles several tenuously-related asks and explicitly invites splitting into stages — this doc does that split and records the design decisions made along the way.

## Workflow across stages

Each stage is implemented in its own context window: finish the stage, update this doc with whatever the next stage needs to know (design decisions made mid-implementation, actual field/route/file names that ended up differing from what was planned, gotchas hit), commit, *then* clear context before starting the next stage. This doc is the hand-off mechanism between stages — a fresh context picks up entirely from what's written here, not from prior conversation, so err on the side of over-recording anything a later stage depends on (exact new symbol names, migration version numbers used, any deviation from this plan and why) rather than assuming it'll be remembered. Each stage's section below should be updated in place (not left describing only the pre-implementation intent) once that stage is actually done, and its status line changed from planned to done.

## Context

Today, creating or editing an item (Task/Event/Simple item/Template child/Item series) is a full page navigation: a row's "⋮" menu has an `Edit` link (`hx-target="#page"`, boosted nav) to `.../:id/edit`, and each list page has a `+ Task`/`+ Event`/etc. header link to `.../new` — both swap the entire `#page` region. The ask is to replace both flows with a dialog overlaid on the current page instead, matching the pattern already established for **row actions** (Reschedule/Assign): a persistent `<dialog id="action-dialog">` in `templates/base.html`, opened by a plain `<button hx-get="..." hx-target="#action-dialog" hx-select="unset">` (no URL push, since it's a `<button>` not an `<a>`), rendering a fragment shaped like `templates/components/reschedule_dialog.html` (a backdrop div + centered form panel, no `<dialog>` tag of its own — the persistent element in `base.html` supplies that).

This item also folds in two more asks:
- **All Projects Tasks/Events lists have no "+ New" button** (`src/web_ui/all_projects_tasks.rs` at `/web/tasks`, `all_projects_events.rs` at `/web/events`) — already tracked as a near-duplicate later in the same doc (issue "Add a New-item button to the cross-project Tasks/Events screens"), which itself concludes this is "the same gap ... likely one piece of work, not two." Both entries are addressed together here. Since these screens aren't scoped to one project, their New dialog needs a project `<select>`, defaulting to **the user's Personal project**, with the form's `hx-post` target rebuilt client-side when the selection changes (the URL is project-scoped: `/web/projects/{project_id}/tasks`).
- **A canonical `personal_project_id` on the user row.** Today "the" Personal project is whatever `ProjectRepo::find_personal_project` happens to find (any team-less project owned by the user) — ambiguous, because a user can create additional personal (team-less) projects via `/web/projects` (`create_project_form` → `service::projects::create_project`, no uniqueness constraint). The all-projects New dialog's "default to Personal" requirement needs an unambiguous answer to "which one," which is what makes this a real dependency of this work, not just a loosely-related aside. The further ask — marking that project non-deletable — has no enforcement to attach to yet: project deletion doesn't exist (separate doc entry, "No way exists to delete projects"). This plan only adds the column and populates it; enforcement is that other item's job once it exists.

## Decisions (answering "let me know what you think")

1. **Scope, revised per user feedback (2026-08-23): row-click opens a read-only detail dialog; edit stays a separate, explicit action.** Three distinct triggers, all swapping into the same `#action-dialog`:
   - A row's name-click (`components/row.html`'s `item_url` link) opens the **read-only detail dialog**, replacing today's boosted navigation to the detail page.
   - The row-actions-menu `Edit` link opens the **edit dialog directly** — it does not go through the detail dialog first.
   - The detail dialog's own `Edit` link/button also opens the edit dialog, via another `hx-get` targeting `#action-dialog` (the dialog element stays open/mounted; only its innerHTML swaps from the detail fragment to the edit fragment — no close/reopen, same mechanic `reschedule_url`/`assign_url` already use to swap `#action-dialog`'s content while it's showing).
   
   `components/row.html` is shared by every screen in the app (calendar day-drawer, `assigned_items`, `project_activity`, not just the per-project/all-projects list screens this plan targets), so this can't be a blanket behavior change to `item_url`'s link. Add an opt-in `Row.detail_via_dialog: bool` field (`src/web_ui/components/row.rs`), defaulting `false`, following the same per-field-opt-in convention the struct already uses for `type_badge`/`parent_name`/`project_name` (screen-specific fields that stay `None`/unset everywhere else). Only `project_tasks`, `project_events`, `project_simple_lists`, `project_templates` (top-level + children — both have their own detail page/view already), `project_item_series`, and Stage 3's `all_projects_tasks`/`all_projects_events` set it `true`; calendar and assigned-items rows are untouched and keep today's page-nav behavior unless a later pass opts them in too.
   
   Direct-URL navigation to a detail page (`GET .../:id`) keeps working for bookmarks/non-JS the same way as Decision 3 below (full page, auto-opened dialog on load) — this decision only changes what an *interactive* row-click does, not whether the underlying page route still exists.
2. **Reuse the existing `#action-dialog` element**, not a second dialog. It's already a generic swap target with no semantic coupling to "row action" beyond convention; New/Edit forms are just another fragment shape swapped into it. Each form keeps its own `hx-target`/`hx-select` for what happens *on submit* (see Stage 1), same as `reschedule_dialog.html` already does independent of the dialog element itself.
3. **Direct-URL navigation to `.../new` / `.../:id/edit` keeps working**, for bookmarks and non-JS/non-htmx access, via the same trick the calendar day-drawer already uses for `?date=...` deep links (`has_selected_date`-gated inline auto-open script, see `templates/base.html`'s comments and `project_calendar/calendar_page.html`). The route keeps rendering a full page (`{% extends "base.html" %}`) whose `#page` content embeds the same dialog markup; a small inline script calls `document.getElementById('action-dialog').showModal()` on load. The `+ Task`/`Edit` buttons, when clicked interactively, use `hx-select` to pluck just the dialog's inner fragment out of that same full-page response — no new response-shape branching, no `HX-Request` sniffing (this codebase has no precedent for that and this doesn't need to start one).
4. **`users.personal_project_id` is additive only in this pass** — no existing call site (`find_personal_project`, import defaults, series defaults) is changed to read it instead. It exists solely to give the new all-projects dialog an unambiguous default. Migrating other call sites off the `find_personal_project` heuristic is worth doing eventually but is its own cleanup, out of scope here.

## Stage 0 — `users.personal_project_id`

**Status: done** (2026-08-23). Small and independent of the dialog work; landed first since Stage 3 depends on it.

What actually landed, for Stage 3 (and anyone resuming after a context clear) to build on:

- `src/domain/user.rs`: `User` gained `personal_project_id: Option<String>`, doc-commented as the unambiguous counterpart to `ProjectRepo::find_personal_project`'s ambiguous heuristic. `User::new` sets it to `None`.
- `src/storage/sqlite/mod.rs`: `CREATE TABLE IF NOT EXISTS users` baseline gained `personal_project_id TEXT`; `row_to_user` reads it; `UserRepo` trait gained `async fn set_personal_project_id(&self, user_id: &str, project_id: &str) -> Result<(), RepoError>` (mockall auto-generates `MockUserRepo::expect_set_personal_project_id`).
- `src/storage/sqlite/users.rs`: implements `set_personal_project_id` (plain `UPDATE users SET personal_project_id = ? WHERE id = ?`, `not_found()` on zero rows affected). Every `SELECT ... FROM users` in this file, and the two `Ok(User { .. })` literals in `get_or_create_by_google_id`/`get_or_create_by_email`, were updated for the new column.
- `src/storage/migrations/add_user_personal_project_id.rs` (new): version **27** (next after `AddUserTimezone`'s 26), name `"add users.personal_project_id"`, same `column_exists`-guarded `ALTER TABLE` shape as `add_user_timezone.rs`. Registered in `all_migrations()` in `src/storage/migrations/mod.rs`. The three `assert_eq!(applied_count, 26)` migration-count assertions in that file's test module are now `27`.
- `src/service/projects.rs::ensure_default_project`: **signature changed** from `(projects: &Arc<dyn ProjectRepo>, user_id: &str)` to `(projects: &Arc<dyn ProjectRepo>, users: &Arc<dyn UserRepo>, user: &User)` — takes the caller's already-fetched `User` (every call site has one fresh off `get_or_create_by_google_id`/`get_or_create_by_email`) rather than doing its own `UserRepo::get`. Behavior: creates the default project and calls `set_personal_project_id` when the user has zero projects (unchanged trigger condition, now also persists the id); when the user already has ≥1 project and `user.personal_project_id.is_none()`, resolves one via the existing `find_personal_project` heuristic and persists it if found — a one-time backfill per user, after which this function never touches that heuristic for them again. Three unit tests cover: fresh-user creation path, already-set-id no-op (asserts via bare `MockUserRepo::new()` with zero expectations that neither `find_personal_project` nor `set_personal_project_id` gets called), and the backfill path.
- All 4 call sites in `src/auth.rs` (`auth_callback`, `auth_me`, `caddy_auth_token`, `caddy_header_middleware`) updated to pass their already-in-scope `UserRepo` (`state.user_repo` or `repo`, depending on the function) and `&user` instead of `&user.id`.
- Two pre-existing hand-rolled `CREATE TABLE users (...)` test fixtures that don't go through `run_migrations` (`src/storage/sqlite/projects.rs::test_pool`, `src/storage/sqlite/teams.rs::test_pool`) needed `personal_project_id TEXT` added directly — both feed `list_members` queries that `SELECT`-list every `users.*` column by name (not `SELECT *`) into `row_to_user`, so those two queries needed the new column added to their column lists too (`src/storage/sqlite/projects.rs::list_members`, `src/storage/sqlite/teams.rs::list_members`). If a later stage adds another explicit `users.<col>`-listing query, check it against `row_to_user`'s full field set the same way — `row_to_user` panics (not a compile error) on a missing column, so this class of bug only surfaces at runtime/test time, not `cargo build`.
- No enforcement, no UI change — this stage only makes the id available for Stage 3 to read. `cargo build` clean (only pre-existing warnings); `cargo test` 465 passed / 0 failed (462 pre-existing + 3 new `ensure_default_project` tests).

## Stage 1 — Proof of concept: Project Tasks new/edit as a dialog

Do this screen first, get it reviewed/working end to end, then replicate mechanically in Stage 2.

- **Templates**: restructure `templates/project_tasks/new_page.html`, `edit_page.html`, **and `detail_page.html`/`detail_view.html`** so each renders a self-contained fragment shaped like `templates/components/reschedule_dialog.html` (backdrop div + centered panel, no outer `<dialog>`) — three fragment shapes now (view/edit/new), all swapped into the same `#action-dialog`. The form fragments reuse the existing `macros::scheduled_fields`/`due_date_fields`/`points_field`/`button_primary` calls already in there; the detail fragment reuses whatever `detail_view.html` already renders (fields + the complete-toggle checkbox — see CLAUDE.md's row-editing-convention note), plus an `Edit` button (`hx-get` back into `#action-dialog`, per Decision 1) and a `Close` button (same idiom as `reschedule_dialog.html`'s Cancel). Each underlying page still `{% extends "base.html" %}` and embeds its own fragment inside `#page`, plus the auto-open `<script>` from Decision 3. The "Add multiple at once" batch form on `new_page.html` stays outside the dialog fragment for now (still a plain page section) — folding batch-create into the same dialog isn't asked for and adds its own UX questions (multiple names vs. one project-select), so it's left as-is.
- **Triggers**: 
  - `templates/project_tasks/list_page.html`'s `+ Task` link → `<button hx-get="/web/projects/{{ project_id }}/tasks/new" hx-target="#action-dialog" hx-select="#task-form-dialog" hx-swap="innerHTML">`.
  - `templates/components/row.html`'s name-click (`item_url`) → conditional on the new `detail_via_dialog` field (Decision 1): when `true`, a `<button hx-get="{{ item_url }}" hx-target="#action-dialog" hx-select="unset" hx-swap="innerHTML">` instead of the current `<a href hx-target="#page">`.
  - `templates/components/row_actions_menu.html`'s `Edit` link (shared by every screen, so this change alone affects all of them structurally — content still varies per screen's own `edit_url`) → same `hx-get`/`hx-target="#action-dialog"`/`hx-select` shape instead of `hx-target="#page"` boosted nav, going straight to the edit fragment (not the detail one).
  - The new detail-dialog fragment's own `Edit` button → same `hx-get`-into-`#action-dialog` shape, pointed at the edit URL.
- **Submit behavior**: `create_project_task_form`/`update_project_task_form` (`src/web_ui/project_tasks/handlers.rs`) keep their existing logic; only the *form's* `hx-target`/`hx-select`/`hx-swap` change:
  - New: `hx-target="#tasks-list"` (or whatever wraps `rows_fragment.html`) `hx-select="#tasks-list"`, so the created row appears in the list; on success, close the dialog (`hx-on::after-request="if(event.detail.successful) document.getElementById('action-dialog').close()"`, same idiom `reschedule_dialog.html` uses).
  - Edit: keep the existing three-fragment response (row + fields + view) `update_project_task_form` already renders — `hx-select="#item-{{ id }}"` picks out just the row for the dialog's own submit, same as today's page-based flow already does for its narrower targets; close the dialog the same way on success.
  - A failed submit (validation error) re-renders the dialog's own fragment in place (`hx-target` stays the form's own wrapper), not `#error-dialog` — same as `reschedule_dialog.html`'s pattern, unlike the row-checkbox's stricter case in `base.html`.
- **Cancel**: same `onclick="document.getElementById('action-dialog').close()"` button `reschedule_dialog.html` already uses.

## Stage 2 — Roll out to the remaining screens

Mechanical repeat of Stage 1's pattern, one screen at a time, confirming each still passes `cargo build`/`cargo test`:

| Screen | New page | Edit page |
|---|---|---|
| `project_events` | `templates/project_events/new_page.html` | `edit_page.html`, `series_occurrence_edit_page.html` |
| `project_simple_lists` | `templates/project_simple_lists/new_page.html` | `edit_page.html` |
| `project_templates` (children only — templates themselves have no dedicated `new_page`, created via "Save as template") | — | `child_edit_page.html` |
| `project_item_series` | `templates/project_item_series/new_page.html` | `edit_page.html` |
| `project_tasks` series occurrences | — | `series_occurrence_edit_page.html` |

`row_actions_menu.html`'s `Edit` link and `row.html`'s `detail_via_dialog` change from Stage 1 already cover all of these structurally; this stage is per-screen: convert each `detail_page`/`detail_view`, `new_page`, `edit_page` (and `child_detail_page`/`child_detail_view`/`child_edit_page` for templates) into dialog fragments, set `detail_via_dialog: true` on that screen's `Row` construction, and confirm `cargo build`/`cargo test` per screen.

## Stage 3 — All-projects "+ New Task" / "+ New Event" with a project selector

Closes both this doc's opening line and the later "Add a New-item button to the cross-project Tasks/Events screens" entry.

- New handlers in `src/web_ui/all_projects_tasks.rs` / `all_projects_events.rs`: `GET /web/tasks/new` / `GET /web/events/new`. Build a project `<select>` from `ProjectRepo::list_for_user(user_id)` (same source `projects.rs`'s own list page already uses), pre-selected to `auth_user`'s `personal_project_id` (Stage 0) if present, else the first project returned. Render the same dialog fragment Stage 1 built for `project_tasks`/`project_events`, parameterized by the selected project (team-admin/assignee fields conditional on that project's `team_id`, same as today).
- **Dynamic POST url**: the `<select>`'s `onchange` updates the form's `hx-post` (`form.setAttribute('hx-post', '/web/projects/' + this.value + '/tasks')` + `htmx.process(form)`) and re-renders the team-only fields (assignee/points) if the newly selected project's team-backed-ness differs — simplest correct approach is an `hx-get` on the `<select>` itself back to `GET /web/tasks/new?project=...`, re-rendering the whole dialog fragment server-side for the new project, rather than duplicating the conditional-fields logic in JS. Mirrors `list_page.html`'s existing project switcher (`hx-get="..." hx-trigger="change"`, see the grep above) rather than inventing a new client-side pattern.
- On submit, same `create_project_task`/`create_project_event` handlers as the per-project screens already use (URL now points at whichever project was selected) — no new creation logic, just a new entry point.
- **Trigger buttons**: add `+ New Task` / `+ New Event` to `templates/all_projects_tasks/list_page.html` / `all_projects_events/list_page.html` (currently absent — this is the literal "no new-task button" complaint), using the same `hx-get`-into-`#action-dialog` pattern as Stage 1.

## Critical files

| File | Change |
|---|---|
| `src/storage/sqlite/mod.rs` | `users` table gains `personal_project_id` |
| `src/storage/migrations/` (new file) | Backfill-safe column add |
| `src/domain/user.rs`, `src/storage/sqlite/users.rs` | `User.personal_project_id` field + row mapping, `set_personal_project_id` |
| `src/service/projects.rs` | `ensure_default_project` sets the new column; on-login backfill for existing users |
| `templates/base.html` | No structural change — `#action-dialog` already generic; comments may need updating once it's used for more than row actions |
| `src/web_ui/components/row.rs`, `templates/components/row.html` | New opt-in `detail_via_dialog: bool` field; conditional link vs. `hx-get`-button |
| `templates/components/row_actions_menu.html` | `Edit` link becomes `hx-get`-into-dialog, targeting the edit fragment directly (affects every screen at once) |
| `templates/project_tasks/{new_page,edit_page,detail_page,detail_view}.html` + `src/web_ui/project_tasks/{handlers,templates}.rs` | Stage 1 proof of concept — view/edit/new all become dialog fragments |
| Same trio for `project_events`, `project_simple_lists`, `project_templates` (children), `project_item_series` | Stage 2 |
| `src/web_ui/all_projects_tasks.rs`, `all_projects_events.rs` (new handlers) + `templates/all_projects_tasks/list_page.html`, `all_projects_events/list_page.html` | Stage 3 |

## Out of scope (deferred)

- Opting calendar day-drawer, `assigned_items`, and `project_activity` rows into `detail_via_dialog` — they keep today's page-nav behavior (Decision 1). Worth revisiting later for consistency, not part of this pass.
- Project deletion and actual "non-deletable" enforcement — separate doc entry ("No way exists to delete projects"); Stage 0 only stores the id.
- Migrating `find_personal_project`'s existing call sites (import defaults, series defaults) onto `users.personal_project_id` — Decision 4.
- Folding the "Add multiple at once" batch-create form into the dialog.
- Team detail page (`teams.rs`) — a member-management page, not a fields-editing one per CLAUDE.md's own note, untouched by this convention already.

## Verification (when implemented)

- Per stage: `cargo build`, `cargo test`, and a careful read of the template/handler diff (per this repo's standing policy, no Playwright click-through — the user verifies interactively).
- Manual smoke test via `task run` per stage: open a list page, click a row name and confirm the read-only detail dialog opens (no URL change, no navigation); click that dialog's `Edit` button and confirm it swaps to the edit fragment in place; separately, open the row-actions menu and confirm `Edit` goes straight to the edit dialog without passing through the detail one; click `+ Task`, confirm that dialog too opens with no URL change; submit an edit and a new-item form and confirm each closes the dialog and updates the row/list without a full-page reload; then paste the `.../:id`, `.../new`, or `.../:id/edit` URL directly into a fresh tab and confirm each still opens (auto-opened dialog on a full page) rather than 404ing or rendering a bare fragment.
- Stage 3 specifically: confirm the project `<select>` defaults to the Personal project, changing it swaps the assignee/points fields correctly for a team-backed project, and the created item lands in the selected project (verify via that project's own Tasks/Events list).
