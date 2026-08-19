# Unify virtual & materialized series occurrences

Status: **Stage A implemented (2026-08-19). Stages B–D planned, not yet implemented.**

## Context

This is `docs/features.md`'s last open item (labeled "Major change" there) and `docs/issues.md`'s ranked item 2, treated as one piece of work rather than two, since they're the same request from two angles:

> "I want virtual occurrences to look the same as materialized occurrences, except that I want to carry over the UI from virtual rows (e.g. display current if current, and skip action is available). This implies that skipping should be available for materialized occurrences, which means we need to essentially delete the occurrence and skip it in a single pass for that path. Additionally and more broadly, I want to materialize only when necessary, i.e. when marked complete, or when the occurrence is actually modified in some other way. So we need to have a way of viewing details for a virtual occurrence that looks the same as materialized occurrences - just use the series id and occurrence date instead of the item id for the URL. I want the distinction between virtual and materialized to be fully hidden from the user."

`issues.md`'s ranked item 2 already independently arrived at the same "skip a materialized occurrence = delete + exdate in one pass" conclusion, plus three still-open gaps: (a) no link from a materialized item's detail page back to its series, (b) no visible "skipped" indicator anywhere, (c) no "unskip" action.

### Key findings from research (three parallel investigations, 2026-08-19)

- **`items` had no `series_id` column.** The only link from a materialized item back to its series was a reverse lookup, `ItemSeriesRepo::find_occurrence_by_item_id`, used only by internal write-path hooks (complete/uncomplete/delete gates in `service::project_items::update_project_item`/`delete_project_item`) — never by any render path. The user asked for this column twice in `issues.md`'s history. **Added in Stage A** — see below.
- **The URL-hiding ask is looser than it first sounds.** The user's own wording — "hidden from the user... unless they are observant enough to deduce from URL differences" — means materialized items can keep their existing URLs (`/tasks/:id`, `/events/:id`) unchanged; only a *new* route for still-virtual occurrences is needed (keyed by `series_id`+`occurrence_date`), plus making the two look the same wherever they're rendered. This significantly shrinks Stage C/D's scope versus migrating every existing item URL onto a series-scoped scheme.
- **"Materialize only when necessary" reopens a decision made the other way on purpose.** Stage 5 of the original item_series design (`docs/archived/recurring-events-virtual-occurrences-rough-plan.md`) explicitly considered lazy materialization and rejected it: "the user confirmed materializing on click/view is fine." Not a blocker — the same user is revisiting it — but Stage C should be built knowing it's a deliberate reversal, not a gap-fill.
- **"Skip on a materialized occurrence" is already precisely specified**, and composes two existing primitives: `unlink_deleted_item_occurrence` (already un-materializes on plain delete, without marking exdate — this was itself a three-pass design correction recorded in `issues.md`, concluding that delete and skip are different intents) and `skip_occurrence`'s `mark_exdate` + cursor-advance. Stage B just needs a thin wrapper that does "if materialized, delete the item first" before doing what `skip_occurrence` already does.
- **`docs/scheduled-catchup-plan.md` is a landmine, not part of this work.** It's written against the old item-level `recurrence` field (retired in Stage 10 of the item_series redesign) and has zero series-awareness. If implemented as currently written, its "reschedule overdue items" bulk action would directly overwrite `scheduled_date` on a stale materialized series occurrence — desyncing it from the series cursor and reintroducing the exact "stuck series" bug fixed 2026-08-19 (see `issues.md`'s "major bug" entry). **Whoever picks up `scheduled-catchup-plan.md` next must add a `series_id IS NULL` guard to its query** — trivial now that the column exists, but easy to miss if that doc is implemented blind. Not otherwise touched by this plan.

## Staging

Four stages, each a separate, independently committable/reviewable unit. Per the user: commit at the end of each stage, then clear context before starting the next one — each stage below is written to stand alone for a fresh session.

### Stage A — foundation (implemented 2026-08-19)

Purely additive. Nothing in `web_ui` changed; no existing behavior changed.

- `items.series_id TEXT` (nullable), migration `AddItemSeriesId` (version 23), index `idx_items_series_id`. Added to baseline `CREATE TABLE IF NOT EXISTS items` too.
- `Item.series_id: Option<String>` (domain). Not user-settable — no Smithy field, CLI flag, or MCP parameter. Set once, only by `service::item_series::get_or_materialize_occurrence`, via a new internal-only `series_id` field on `CreateItemParams`/`CreateTeamItemParams`/`CreateProjectItemParams`. Every JSON API/web UI/import construction site explicitly passes `None`.
- Carried forward on every update (`items::update_item`, `team_items::update_team_item`) from `current.series_id`, the same way `project_id` already is — an item's series membership never changes after creation.
- New service function `item_series::list_occurrence_states_for_project` + `OccurrenceState { Materialized { item_id }, Skipped, Virtual }` + `ProjectOccurrence`, alongside the existing `list_virtual_occurrences_for_project_unchecked`. Where that function *excludes* any date with an `item_occurrences` row, this one classifies every candidate date into one of the three states — the single data source Stage D's rendering unification builds on, so screens stop separately querying materialized items and virtual occurrences and merging them by hand. Returns `item_id` only (not a full `Item`) for materialized dates, so callers batch-fetch via their existing `list_by_project`/`list_due_by_project` calls rather than duplicating item-fetch logic. **New and unused by any `web_ui` code as of Stage A** — Stage D wires screens onto it.

Critical files: `src/storage/migrations/add_item_series_id.rs`, `src/storage/sqlite/mod.rs` (baseline table/index, `row_to_item`), `src/storage/sqlite/items.rs` (SELECT/INSERT/UPDATE), `src/domain/item.rs`, `src/service/items.rs`, `src/service/team_items.rs`, `src/service/project_items.rs`, `src/service/item_series.rs`.

### Stage B — skip/unskip unification + series link (planned)

- New service function (working name: `skip_or_delete_project_item_series_occurrence`) that, given `(series_id, occurrence_date)`: if the occurrence is materialized, first deletes the item via the existing `project_items::delete_project_item` (which already calls `unlink_deleted_item_occurrence`, reverting the row to virtual/no-row), then runs the existing `skip_occurrence` logic (`require_current_occurrence` gate, `mark_exdate`, cursor advance) unchanged. This is a *composition* of two already-correct primitives, not new mutation logic.
- New "unskip" function, mirroring `record_task_uncompletion`'s cursor-safety rigor: for a Task-typed series, only the occurrence at `cursor_date` can be unskipped (no self-heal past it — same reasoning `require_cursor_occurrence` already uses for uncompleting), and on success retreats the cursor one step (`recurrence::retreat_once`) or clears it if it was the anchor. For an Event-typed series (no cursor), any exdate occurrence can be unskipped unconditionally. Both call the existing `ItemSeriesRepo::delete_occurrence` to remove the exdate row.
- New route `POST /projects/:project_id/series/:series_id/occurrences/:occurrence_ts/unskip`, alongside the existing `.../skip` route (which gets the new delete-aware behavior above, not a new endpoint — the existing Skip button now works whether the occurrence it's attached to is virtual or materialized).
- Visible "skipped" indicator: everywhere an occurrence currently renders (task/event rows, calendar entries), check `OccurrenceState::Skipped` (from Stage A's `list_occurrence_states_for_project`) and render struck-through + "Skipped" label + an Unskip button instead of the current per-screen virtual/materialized split.
- Series link on a materialized item's detail page: `item.series_id.is_some()` → fetch the series (`ItemSeriesRepo::get_series`) → render a "Part of series: {name}" link to the series detail page. Closes `issues.md` ranked item 2(a).

### Stage C — deferred materialization + virtual occurrence detail page (planned)

- New `GET /projects/:project_id/series/:series_id/occurrences/:occurrence_ts` route: renders a detail page visually matching the materialized item detail page (same template/struct shape, extended to accept either a real `Item` or a synthesized read-only view built from `ItemSeries` + `occurrence_date` + `OccurrenceState`), with **no side effect** — does not call `get_or_materialize_occurrence`. This is genuinely new territory (today, opening a virtual occurrence always materializes-then-redirects); no existing route to retrofit.
- Every mutation reachable from that page (complete checkbox, "Save" on an edit form, adding a sub-item) wraps its existing handler with "materialize first if not already, then perform the write" — `get_or_materialize_occurrence` becomes an internal step of the mutation, not something the UI triggers separately via a redirect.
- Task/Event list rows and calendar entries for a still-virtual occurrence link to this new `GET` route instead of directly `hx-post`-ing the materialize route.
- Materialized items keep their existing URLs (`/tasks/:id`, `/events/:id`) unchanged — see the "URL-hiding ask is looser than it sounds" finding above.

### Stage D — collapse duplicated rendering (planned)

- Replace the parallel struct pairs — `ProjectTaskRow`/`ProjectTaskVirtualRow`, `CalendarTaskEntry`/`CalendarVirtualTaskEntry` (Tasks currently uses two full parallel types per surface) — with one shape built on Stage A's `ProjectOccurrence`/`OccurrenceState`, closer to the precedent Events' calendar already set with `CalendarEventEntry` (one struct, `Option`/`bool` fields distinguishing virtual vs. materialized, single render block) rather than Tasks' fully bifurcated approach.
- Extend Events' list view (`project_events_page`) to show virtual/skipped occurrences too — today it has **zero** virtual-occurrence support (only the Events calendar does); Tasks' list view already merges them.
- Consolidate the `entry_id`/`materialize_url`/`skip_url` string-building logic, currently duplicated three times (Tasks calendar, Events calendar, Tasks virtual row) with an identical format string, into one helper.

## Verification (Stage A)

- `cargo test`: `series_id_round_trips_through_create_and_update` (`src/storage/sqlite/items.rs`), the extended `materializes_a_new_event_when_no_occurrence_row_exists` assertion (`src/service/item_series.rs`), and two new tests for `list_occurrence_states_for_project` covering all three `OccurrenceState` variants plus the Task-series current-occurrence injection case. Full suite: 378 passed.
- `cargo build` clean.
- Live smoke check via `task run`: materialize a series occurrence through the existing UI flow, confirm the resulting item's `series_id` matches the series.
- No existing behavior changed: task/event list, calendar, and detail pages render identically to before, since nothing in `web_ui` was touched this stage.
