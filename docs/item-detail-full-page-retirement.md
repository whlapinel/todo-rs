# Retiring item detail "full pages"

Status: **done** (2026-08-24).

## Context

Every item type's `GET .../:id` detail route rendered two things at once: the read-only detail
*dialog* (`docs/archived/dialog-item-forms-plan.md`'s Stage 1/2 — a `<dialog>` fragment, opened
in place by a row's name-click) auto-opened on top of, and layered over, a full legacy detail
*page* underneath it (header with Edit/Back/Delete, a "Sub-items"/"Linked tasks" management
section, a "Save as template" button). The dialog's own "View full page" link was the only
thing that ever navigated to that full page interactively.

That link was a real, reproducible bug, not just redundant duplication: it did
`hx-target="#page" hx-select="#page" hx-swap="innerHTML"` **and** `onclick="…close()"` on the
same click. The `close()` ran synchronously; the htmx request was async. By the time the
full-page response landed and got swapped into `#page`, the dialog had already closed — but the
swapped-in page's own Decision-3 auto-open `<script>` (`if (dialog && frag && !dialog.open)`)
saw a closed dialog and immediately reopened it with the exact same read-only content the user
had just closed. Net effect: clicking "View full page" reopened the same quick-look dialog
instead of taking you anywhere new.

Given the full page was largely redundant with the dialog anyway, the fix was to retire it
rather than patch the race.

## What changed

Every `.../:id` detail route (Tasks, Events, Simple Lists, Template children, and the two
Task/Event series-occurrence detail routes) now renders **only** the dialog fragment plus the
Decision-3 auto-open script — the same minimal shape `edit_page.html`/`new_page.html` already
used. This also means these routes now share those pages' existing accepted gap: loading
`.../:id` directly (no list page underneath) and then, from a "View full page" link, there's
nothing left to link to, so that link/button was deleted outright rather than fixed. Each
`*DetailDialog` struct dropped its `full_page_url` field; each `*DetailPageTemplate` struct
dropped every field that page's now-removed header/management section had used (`id`,
`project_id`, `complete`/`is_imported` gating, the raw `view` string — the dialog itself still
carries `view`).

Three pieces of functionality that only ever lived on the retired full pages needed a new home:

1. **Sub-item batch-add** (Tasks, Simple Lists). The full detail page's "Add multiple at once"
   `<details>`/textarea, posting to the existing `.../batch` route (already accepted
   `parentItemId`/`redirect` — the detail page just posted to it with a narrower
   `#children-list` target instead of a full redirect). Moved into
   `components/add_child_dialog.html` itself (the "Add sub-item" row action), gated on a new
   `post_batch_url: String` field on `AddChildDialog` — always set for Tasks/Simple Lists, since
   both already have the batch route.

2. **"Save as template"** (Tasks, Events — Simple Lists never had this route, despite what an
   older revision of CLAUDE.md's Web UI section claimed; verified against `src/main.rs`'s actual
   route table before relying on it). Moved to the row-actions "⋮" menu as a new
   `hx-post`/`hx-swap="none"` button, gated on a new `Row.save_as_template_url: Option<String>`
   field. `hx-swap="none"` because the popover closes itself on any successful action from an
   element inside it (`base.html`'s generic `htmx:afterRequest` listener) before the old
   "Saved" confirmation text would ever be seen — same as `duplicate_url`'s existing button.
   Events' version is intentionally **not** gated on `is_imported`, matching the old page's own
   unconditional button.

3. **Events' "New linked task" form** — the one piece that wasn't a drop-in reuse of an
   existing row action. Events create a plain Task referencing themselves via `sourceEventId`
   (never `parentItemId` — Events can never have structural children, see `ProjectEventRow`'s
   doc comment), so the generic `add_child_dialog.html`/`add_child_url` mechanism Tasks/Simple
   Lists use doesn't fit. Added a **new**, Event-specific dialog
   (`components/add_linked_task_dialog.html`, `project_events::templates::AddLinkedTaskDialog`)
   and a **new** `Row.add_linked_task_url: Option<String>` field (mutually exclusive with
   `add_child_url` — never both `Some` on the same row), wired only in
   `ProjectEventRow::from_item`. `create_project_event_child_form`
   (`src/web_ui/project_events/handlers.rs`) gained `redirect: Option<String>` handling
   (mirroring every other create form's `redirect=1` convention) since, unlike the old detail
   page, a row-action-opened dialog has no `#children-list` on whatever page it was opened from
   to target instead — `GET /projects/:project_id/events/:item_id/add-linked-task` is the new
   route that opens it.

## Deliberately dropped, not migrated

The two series-occurrence detail pages
(`project_tasks`/`project_events` `series_occurrence_detail_page.html`) each had a unique
"materialize this virtual occurrence by adding a sub-item/linked task" form with no equivalent
anywhere else in the UI. Per the existing Stage 2 note in
`docs/archived/dialog-item-forms-plan.md`, nothing in the UI ever linked to either route
interactively in the first place (confirmed again before this pass, via
`grep -rn 'occurrences/{{ occurrence_ts }}"' templates/` — no hits) — both pages were reachable
only by typing the URL directly. Since that materialize-via-child mechanism had no other UI
entry point, it was deleted along with the rest of the page rather than given a new home. If a
UI path to a still-virtual occurrence's detail page is ever added, this capability would need
to be rebuilt at that point, not assumed still present.

Simple Lists' full detail page also had a "Move" button — dropped without replacement since it
was already fully redundant with the row's own existing "Move" row action (`move_url`), unlike
the other cases above which had no existing equivalent.

## Files touched

- `src/web_ui/components/row.rs`: new `save_as_template_url`/`add_linked_task_url` fields.
- `templates/components/row_actions_menu.html`: new "Save as template"/"Add linked task" menu
  entries.
- `templates/components/add_child_dialog.html`: new batch-add `<details>` section.
- `templates/components/add_linked_task_dialog.html` (new): Events' own add-linked-task dialog.
- Per screen (`project_tasks`, `project_events`, `project_simple_lists`, `project_templates`
  children, and the Task/Event series-occurrence pair): `templates.rs`/`handlers.rs` struct and
  handler simplification, `detail_dialog.html` (drop "View full page"), `detail_page.html`
  (strip to dialog-only).
- `src/main.rs`: new `GET /projects/:project_id/events/:item_id/add-linked-task` route.

`cargo build`/`cargo fmt`/`cargo test` clean (489 passed, same count as before this pass — no
new tests added; this is template/wiring restructuring over already-tested service/handler
logic, same precedent `docs/archived/dialog-item-forms-plan.md` itself used). `task web-styles`
run, no `static/style.css` diff (every class used was already present elsewhere). Not yet
verified live in a browser — the user's own pass per this repo's standing "no Playwright
click-through" policy (see CLAUDE.md).
