# Retiring item detail "full pages"

Status: **partially reverted** (2026-08-24, revised 2026-08-25, extended to Tasks same day) —
see "Revert" and "Revision" sections at the end. Simple Lists' and Tasks' full pages are back;
Events/Template children/series-occurrence pages are still in the dialog-only shape this doc
originally describes, pending the same treatment.

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

## Revert (2026-08-24, later same day)

Retiring the full pages turned out to be the wrong call — trading a real but narrow bug (see
"Context" above) for losing the pages outright. Reverted for **Simple Lists only** as a first
trial; the same treatment is expected to follow for Tasks/Events/Template children/series
occurrences once this is confirmed to feel right.

The actual fix for the original race: `detail_dialog.html`'s "View full page" link no longer
closes `#action-dialog` via a synchronous `onclick` (which ran at click time, before the async
full-page request even started — so by the time the response landed and its own Decision-3
auto-open script ran, `dialog.open` already read `false` and the script reopened it). It now
closes the dialog via `hx-on::after-request="if(event.detail.successful) …close()"` instead —
the same "close `#action-dialog` once a targeted request lands" convention already used by
`reschedule_dialog.html`/`quick_assign_dialog.html`/each screen's own `detail_fields.html`. That
guarantees the full-page swap (and its embedded auto-open script) runs first, while the dialog
is still open, so the script's `!dialog.open` guard correctly no-ops; only afterward does the
link's own handler close it. Restored in full otherwise: `ProjectSimpleItemDetailPageTemplate`
regained `id`/`project_id`/`description`; `ProjectSimpleItemDetailDialog` regained
`full_page_url`; `detail_page.html` is back to its original header (Edit/Back/Delete), Move
button, and Sub-items section verbatim. The batch-add addition to `add_child_dialog.html` from
the retirement pass was left in place (additive, not conflicting with the restored page's own
single-add form).

## Revision (2026-08-25) — full-page access moved to the row popover, not the dialog

Even with the previous fix's close-timing corrected, the dialog↔full-page round trip (dialog's
"View full page" link → full page auto-opens its own dialog via Decision 3 → that dialog's own
"View full page"/parent link → …) kept surfacing new bugs as it was driven around by hand:
htmx's default whole-`document.body` history-cache snapshot fighting with `#action-dialog`'s
open state on back/forward (fixed via `hx-history-elt` on `#page`), a stale dialog left open
after an unrelated navigation elsewhere (fixed via a `htmx:historyCacheHit` listener that closes
`#action-dialog`/`#error-dialog`/the row-actions popover), and finally two items' dialog markup
stacking in the same `<dialog>` at once (fixed via clearing `#action-dialog`'s content before
each move-in). Each fix addressed a real bug, but the pattern kept producing new ones — because
the underlying design let the dialog and the full page navigate into each other, and a modal
`<dialog>`'s "nothing outside it is clickable" invariant only holds if nothing *inside* it can
trigger navigation either.

The actual fix: stop letting the dialog navigate to the full page at all. `detail_dialog.html`'s
"View full page" link is gone; the *only* way to reach an item's full page is now
`components/row_actions_menu.html`'s new "View full page" entry, on the row's `⋮` popover
(non-modal, only reachable when no dialog is open in the first place). `detail_page.html` no
longer auto-opens a dialog on load either (Decision 3 retired) — a full-page load, however
reached (popover link, bookmark, back/forward), just renders the plain page; the dialog fragment
is still embedded in the same GET response (wrapped `<div hidden>`) purely so the row's own
`hx-get`/`hx-select="#dialog-fragment"` click can still pluck it out of that same route, but
nothing ever shows it there automatically anymore. With the dialog never triggering navigation
and the full page never auto-showing a dialog, the two can't race — the earlier three fixes
(`hx-history-elt`, `historyCacheHit`, dialog-content-clear) are still in place as defense in
depth, but none of them have anything left to defend against on this particular path.

`ProjectSimpleItemDetailDialog` lost `full_page_url` (nothing renders it anymore).
`ProjectSimpleItemDetailPageTemplate` keeps `dialog` (still needed for the `hidden`-wrapped
embed) but the field's own doc comment no longer describes an auto-open.

Scope note: `components/row_actions_menu.html` is shared by every screen using the common `Row`
(Tasks, Events, Simple Lists, the cross-project/calendar screens), so the new "View full page"
entry now shows up for all of them, not just Simple Lists — harmless today since it just points
at the same dialog-only `detail_page.html` those screens already had (see "Status" above), same
destination a bookmark already reached. The dialog's own "View full page" link removal, and
`detail_page.html`'s Decision-3 removal, were only done for Simple Lists — Tasks/Events/Template
children/series-occurrences still have their in-dialog link and auto-open behavior untouched,
pending the same full-page-restoration treatment this doc's Status line describes.

## Tasks restoration (2026-08-25, later same day)

Applied the same revert+revision treatment to Tasks. `ProjectTaskDetailPageTemplate` regained
`id`/`project_id`/`complete`/`view` (the pre-retirement fields); `detail_page.html` is back to
its original header (Edit/Back to tasks/Delete) and Sub-items management (single-add form with
`dueOffsetDays`), verbatim from before retirement. Unlike Simple Lists, no separate Move button
or header parent-link was needed — `detail_view.html` (rendered into `view`) already has both
inline (its own checkbox row's Move button, and a "Parent"/"Linked event"/"Part of series" `<dl>`
row), so restoring `view` onto the page for free restores those too. The pre-retirement page's
"Save as template" button (next to the Sub-items heading) and "Add multiple at once" batch form
were **not** restored — both are already reachable from the row's "⋮" menu
(`save_as_template_url`, and `add_child_dialog.html`'s batch-add section via `post_batch_url`;
see the "What changed" section above), so re-adding them to the page would just be a second path
to the same action — the same "already redundant, don't restore" call this doc's Revert section
made for Simple Lists' Move button. `detail_page.html` follows the same `hidden`-wrapped
`{{ dialog|safe }}` embed and no-auto-open-script shape the Revision section describes; the
dialog fragment itself never had a "View full page" link to remove (Tasks' dialog was built
after the original retirement pass, so it was already this shape). `detail_dialog.html`'s doc
comment was updated to say Sub-items management lives on the full page again, not "the row's own
'Add sub-item' action" only.
