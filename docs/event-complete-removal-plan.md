# Remove "complete" from Events + auto-advance-on-read for recurring events

Status: **superseded, implemented in reduced form 2026-08-15**. Written 2026-08-13 after a planning discussion, when Events could still recur at the item level and completion was the only mechanism that advanced a recurring Event's date — see the Context/Design sections below for that now-obsolete premise.

Item-level recurrence (including for Events) was fully retired in Stage 10 of `docs/recurring-events-virtual-occurrences-rough-plan.md` (`8797ac9`), before this plan's sections 2/3/5 (`advance_if_stale`, the read-time advance wrapper, and the CLI/MCP client-side guard) were ever built. Recurring Events now live on `item_series`, advanced by materialization independent of any `complete` field — so the "complication" this doc was written to solve no longer exists, and sections 2/3/5 were never implemented and never will be.

**Section 4 (UI removal) was implemented as originally scoped**, once the blocker cleared — see `docs/archived-features.md` for the shipped write-up: `Item::validate()` rejects `complete: true` for `ItemKind::Event`, and every checkbox/Done-label/"Show completed" filter was stripped from `templates/project_events/*.html`, `src/web_ui/project_events/*.rs`, and `src/web_ui/project_dashboard.rs`'s Event rows, exactly matching this doc's section 4 file list.

## Context

Events don't represent an action someone performs — unlike Tasks, there's no real "done" semantic for a calendar-style entry. The request is to remove the "complete" concept from Events entirely, front-to-back, mirroring the precedent already set for Simple items (`features.md`'s "Simple lists shouldn't even have complete/done UI" entry: `Item::validate()` rejects `complete: true` for that kind, and every checkbox/label/filter tied to completion was stripped from the UI).

**The complication, found during planning**: Events currently *can* recur (`recurrence`/`recurrence_basis` are live, user-facing fields on the Event create/edit forms — this is a real, actively-built capability, not dead code), and the *only* mechanism that advances a recurring item's date today is `Item::next_recurrence`, which is gated on `self.complete == true`. There's also real machinery riding on top of that same completion trigger: `repoint_source_event_tasks`/`sync_source_event_tasks` (`src/service/items.rs`, duplicated in `team_items.rs`) keep any Task linked to an Event via `sourceEventId` correctly dated relative to the event's current occurrence. Removing `complete` outright, with no replacement, would freeze every recurring event's date forever and orphan that sync machinery.

Compared two replacement designs (see the rough-plan doc for the fuller one): a full iCalendar/RRULE-style virtual-occurrence engine (where occurrences are computed on demand rather than stored, the way Google/Apple/Outlook calendars work) versus a much lighter **auto-advance on read** — keep today's one-row-per-current-occurrence storage model, and just change the trigger from "user marks it done" to "the system notices the stored date has passed and rolls it forward in place." The fuller rewrite is a materially bigger feature (see that doc) and isn't justified by this app's actual usage pattern (caring about "what's the next occurrence," not browsing arbitrary future ranges of a series). Auto-advance-on-read was chosen for now.

One relevant finding from investigation: Event completion on a **team-backed** project is already broken today — `assigned_to_user_id` is structurally always `None` for Events (only `ItemType::Task` carries a `TeamAssignment`), and `update_team_item`'s completion guard unconditionally rejects completing an unassigned item. So this removal doesn't break any currently-working team workflow.

## Design

### 1. `Item::validate()` — close off `complete: true` for Events

Mirror the existing `Simple` rejection (`src/domain/item.rs:389-433`):

```rust
if self.complete && self.kind() == ItemKind::Event {
    return Err("events cannot be marked complete".to_string());
}
```

This is the single shared enforcement point reached by every surface (web UI, JSON API, CLI, MCP), same as the Simple precedent — no per-surface validation needed elsewhere. `complete` stays a flat field on `Item` structurally (it's shared across every kind at the struct level, not per-`ItemType`-variant) — only the allowed-value policy changes.

### 2. New domain helper: `Item::advance_if_stale`

Add near `next_recurrence`/`deadline_from_offset` (`src/domain/item.rs:452-504`):

```rust
pub fn advance_if_stale(&self, now: DateTime<Utc>, tz_offset_minutes: i32) -> Option<(DateTime<Utc>, Option<DateTime<Utc>>)>
```

For an `ItemKind::Event` with a recurrence pattern set, whose basis-driven date field (`due_date` or `scheduled_date`, per `recurrence_basis` — same selection logic as `next_recurrence`'s existing branches, `src/domain/item.rs:465-484`) is in the past relative to `now`: computes the next future date via `recurrence::next_date` (already walks forward past *every* missed cycle in one call — confirmed via `src/domain/recurrence.rs:138-145`'s `while next <= now` loop, so a daily event stale for months still resolves correctly in one call). Returns `None` if nothing needs to change (not recurring, not an Event, or not actually stale).

**Deliberately different from `next_recurrence`**: no new `id`, no clone, no archive. The row is updated **in place**. Events have no need for a history of past occurrences the way a completed Task's archived row might be useful for, and in-place update avoids leaving behind a graveyard of "recurred while nobody was looking" rows. Same delta-shift treatment for `scheduled_end_date` as `next_recurrence` already does (`src/domain/item.rs:478-479`), to preserve window duration. `due_date` is untouched unless the event's own recurrence basis targets it.

Unit tests alongside the existing `next_recurrence`/`reschedule_forward`-style tests: multi-cycle-stale weekly event lands on the correct next future date in one call; a non-recurring or non-stale Event returns `None`; `scheduled_end_date` shifts by the same delta; a `DUE_DATE`-basis recurring event advances `due_date` not `scheduled_date`.

### 3. Where the read-time check runs

Per the "on read" choice (not a periodic sweep), every service-layer entry point that returns Event items needs to call this and persist the result — kept in the **service layer**, not the repo layer, to preserve the existing repo-is-dumb-CRUD / service-holds-business-logic split (`CLAUDE.md`'s Storage Layer section). A single shared helper, e.g.:

```rust
async fn advance_stale_events(repo: &Arc<dyn ItemRepo>, items: &mut [Item], tz: i32) -> Result<(), ItemError>
```

— checks each item, and for any stale recurring Event: persists the new date via the repo's existing update method, then re-runs `sync_source_event_tasks` (reused as-is) for any Tasks linked via `sourceEventId`.

Call sites needing this wrapper (accepted cost of "on read" vs. a sweep — flagged explicitly during the design discussion as the main tradeoff):

- `project_items::get_project_item` / `list_project_items` (`src/service/project_items.rs`) — covers the Event detail page, the Events list screen, and (transitively, since CLI/MCP go through the same API) `prl items get/list` and the MCP `get_item`/`list_items` tools.
- `project_dashboard.rs`'s list and calendar views (both currently call `list_due_by_project` directly rather than through `project_items`'s service wrapper — need to confirm at implementation time whether they route through the same wrapped path or need their own call).
- Any JSON API handler that reads items directly from the repo rather than through `service::project_items` (verify at implementation time — flagged as a verification step below rather than assumed).

### 4. UI removal (mirrors the Simple precedent exactly)

- `templates/project_events/detail_view.html` — remove the complete-toggle checkbox + "Done"/"Not done" label.
- `templates/project_events/detail_fields.html` — remove the `complete` `<select>` (Done/Not done dropdown) from the edit form.
- `templates/project_events/detail_page.html` — the `{% if !complete %}` Edit-link gate becomes unconditional (events are always editable, matching Simple's precedent — no completion state can ever lock them).
- `templates/project_events/list_page.html` — remove the "Show completed" filter checkbox.
- `templates/project_events/new_page.html` — remove the hidden `showComplete` passthrough field.
- `src/web_ui/project_events/handlers.rs` — remove `ShowCompleteQuery` and its threading through `project_events_page`/`new_project_event_page`/`redirect_to_project_events`; remove the recurring-just-completed `NotFound`→`hx-refresh` special case in `update_project_event_form` (dead once recurrence never routes through the complete-and-clone path for Events).
- `src/web_ui/project_events/mod.rs` — `render_rows`'s `.filter(|i| show_complete || !i.complete)` (line 269) becomes unconditional; `update_params_from_form`'s `complete: overlay_bool(&form.complete, current.complete)` (line 236) hardcodes `false`, matching `project_simple_lists`'s existing precedent for its own update-params builder.
- `src/web_ui/project_events/templates.rs` — `ProjectEventRow` sets `complete_url: None` (exact mirror of `project_simple_lists/templates.rs:21,32-33`) to suppress the shared `components/row.html` checkbox with zero changes to that shared component.
- `src/web_ui/project_dashboard.rs` — `ProjectDashboardRow::from_due_item` currently gives Events a working complete-toggle checkbox and includes them in the `show_complete`-gated filter (`render_rows`, line 198) same as Tasks; needs the same `complete_url: None`-style suppression for Event rows specifically (Task rows keep their toggle).

### 5. CLI/MCP (nice-to-have, not required for correctness)

Server-side `validate()` is the actual enforcement point (no Smithy-level enum restricts `complete` per item type, confirmed during investigation). Optionally add a client-side pre-check in `prl items done`/the MCP `update_item` tool rejecting `complete: true` when `--item-type`/`itemType` is `event`, for a cleaner error message than a raw 400 — same precedent as the existing `--event-type` client-side guard in `todo-cli/src/items.rs`.

## Critical files

| File | Change |
|---|---|
| `src/domain/item.rs` | `validate()` rejection; new `advance_if_stale`; unit tests |
| `src/service/project_items.rs` (or wherever the shared read wrapper lands) | `advance_stale_events` helper + wiring into `get_project_item`/`list_project_items` |
| `src/service/items.rs` | Reuse `sync_source_event_tasks` from the new caller |
| `src/web_ui/project_events/{mod,handlers,templates}.rs` | Strip completion UI/state per section 4 |
| `src/web_ui/project_dashboard.rs` | Strip Event complete-toggle from dashboard rows |
| `templates/project_events/*.html` | Strip completion markup |
| `todo-cli/src/items.rs`, `mcp-server/src/index.ts` | Optional client-side guard |

## Verification (when implemented)

- Unit tests for `advance_if_stale` (multi-cycle-stale event resolves correctly in one call; in-place id preserved, no clone/archive; `scheduled_end_date` delta preserved; `DUE_DATE`-basis vs `SCHEDULED_DATE`/`COMPLETION_DATE`-basis behave correctly; non-stale/non-recurring returns `None`).
- Unit test confirming `Item::validate()` rejects `complete: true` for `ItemKind::Event`.
- Unit test confirming a stale event with a `sourceEventId`-linked task gets that task re-synced when the event advances.
- `cargo test` for the full suite.
- Manual smoke test via `task run`: create a weekly-recurring event several weeks stale; confirm visiting its list row, detail page, and the project dashboard (list + calendar views) all show it silently rolled forward to the correct next future date, with no "complete" affordance visible anywhere on the Events screens; confirm a Task linked via `sourceEventId` re-anchors to the new date too; confirm the Events edit form no longer offers a Done/Not done option and the "Show completed" filter is gone from the list page.
