# Cross-project "Scheduled start in past" catch-up screen + mass reschedule

Status: **planned, not yet implemented**. Written 2026-08-13 after a planning session; supersedes the informal notes in `features.md` under "Skip current" / "Individual and Mass rescheduling" — implement from this doc instead.

## Context

Recurring items in this app only advance their `scheduled_date` (or `due_date`, depending on `recurrence_basis`) when they're explicitly marked complete — nothing advances them just because time has passed. If the user doesn't touch the app for a week, every daily/weekly `SCHEDULED_DATE`-basis recurring task sits stuck on the same stale date, and the only way to "fix" it today is to either mark it complete (semantically wrong — nothing was actually done) or manually re-edit the date by hand, one item at a time. Non-recurring items with a stale `scheduled_date` have the same problem, just without even a recurrence pattern to fall back on.

This plan adds a cross-project view of every incomplete top-level Task whose `scheduled_date` is in the past, plus a one-click "reschedule all" action that:
- for recurring items whose recurrence actually targets `scheduled_date` (`recurrence_basis` = `SCHEDULED_DATE` or `COMPLETION_DATE`), jumps forward to the next *future* occurrence per the item's own recurrence pattern (skipping every missed cycle in between, not just one), instead of completing/cloning the item;
- for everything else (no recurrence, or recurrence that targets `due_date` instead), simply moves the stale `scheduled_date` to today, keeping the original time-of-day;
- in both cases, never touches `due_date`, and shifts `scheduled_end_date` by the same delta so any window's duration survives.

Research (three parallel codebase investigations, 2026-08-13) confirmed there is currently **no cross-project item query for this**, and **no bulk-action pattern anywhere in the app** — the closest analogs are the existing `assigned_items.rs` cross-project screen (flat SQL query, no per-project fan-out) and the CSV importer's best-effort-per-row execution model.

Decisions confirmed with the user during planning: (1) build this as a new dedicated cross-project screen rather than a per-project dashboard preset, since the whole point is cutting across all projects at once; (2) non-recurring items reschedule to "today, same time-of-day" (not a user-prompted custom date); (3) scope this to **Tasks only** for now (Events can be added later using the same mechanism).

For team-backed projects, only items assigned to the requesting user are included; personal (non-team) projects show all matching items regardless of assignment, since `assignedToUserId` isn't a meaningful concept there.

## Design

### 1. New repo method: `ItemRepo::list_scheduled_overdue`

Add to the `ItemRepo` trait (`src/storage/sqlite/mod.rs`, alongside `list_assigned`/`list_due_by_project`) and implement in `src/storage/sqlite/items.rs`, following `list_due_by_project`'s query-building conventions (`src/storage/sqlite/items.rs:401-436` — `format!`/`sqlx::query` string, positional `.bind()`, `db_err` mapping, `row_to_item`):

```rust
async fn list_scheduled_overdue(&self, user_id: &str, now: i64) -> Result<Vec<DueItem>, RepoError>;
```

SQL (single flat query across every project the user belongs to — mirrors `list_assigned`'s "no per-project fan-out" precedent rather than looping `ProjectRepo::list_for_user` + N `list_by_project` calls):

```sql
SELECT items.*, COALESCE(parent.name, '') AS parent_name,
       EXISTS(SELECT 1 FROM items c WHERE c.parent_item_id = items.id) AS has_children,
       projects.name AS project_name, projects.id AS project_id_out
FROM items
JOIN project_members pm ON pm.project_id = items.project_id AND pm.user_id = ?
JOIN projects ON projects.id = items.project_id
LEFT JOIN items parent ON items.parent_item_id = parent.id
WHERE items.item_type = 'TASK'
  AND items.complete = 0
  AND items.parent_item_id IS NULL
  AND items.scheduled_date IS NOT NULL
  AND items.scheduled_date < ?
  AND (projects.team_id IS NULL OR items.assigned_to_user_id = ?)
ORDER BY items.scheduled_date ASC
```

This directly encodes the personal-vs-team distinction: `projects.team_id IS NULL` (personal project) always matches; a team-backed project only matches when the item is assigned to the requesting user. `parent_item_id IS NULL` excludes offset-driven children (which can't carry a manual `scheduled_date` at all per `Item::validate()`), and `item_type = 'TASK'` excludes Events/Simple/Templates per the scoping decision above.

Reuse the existing `DueItem { item, parent_name }` shape (`src/storage/sqlite/mod.rs:16-19`) — no new struct needed; the project name for display can be looked up client-side from `item.project_id` via a small `HashMap` built from `ProjectRepo::list_for_user`, the same way `project_dashboard.rs` already builds a `names` map for assignees.

### 2. New domain helper: `Item::reschedule_forward`

Add near `next_recurrence`/`deadline_from_offset` in `src/domain/item.rs` (around line 452-504):

```rust
pub fn reschedule_forward(&self, now: DateTime<Utc>, tz_offset_minutes: i32) -> Option<(DateTime<Utc>, Option<DateTime<Utc>>)>
```

Returns `None` if there's no `scheduled_date` to move. Otherwise:

- **Recurrence targets `scheduled_date`** (`recurrence_pattern().is_some()` and `recurrence_basis()` is `"SCHEDULED_DATE"` or `"COMPLETION_DATE"`): parse the pattern and call `recurrence::next_date(&rule, reference, tz_offset_minutes)`, reference = current `scheduled_date` (`SCHEDULED_DATE` basis) or `now` (`COMPLETION_DATE` basis) — exactly the reference-selection logic `next_recurrence`'s else-branch already uses (`src/domain/item.rs:471-476`), just without requiring `self.complete` or cloning the item. `next_date` already advances past *every* missed cycle in a single call (confirmed: `src/domain/recurrence.rs:138-145`'s `while next <= now` loop), so one call correctly skips all past instances.
- **Otherwise** (no recurrence, or recurrence targets `due_date`): compute today's local calendar date from `now` and `tz_offset_minutes`, combine it with the *original* local time-of-day of `scheduled_date` (or start-of-day if `has_scheduled_time()` is false) — same date/time-split approach as `combine_local_to_utc` (`src/web_ui/project_tasks/mod.rs:108-122`), just operating on an existing `DateTime<Utc>` instead of parsed form strings.
- Either branch: if `scheduled_end_date` is set, shift it by `new - old` to preserve window length — same pattern as `next_recurrence`'s existing delta-shift (`src/domain/item.rs:478-479`). `due_date` is never read or written by this method.

Unit-test alongside the existing `next_recurrence` tests (`src/domain/item.rs`'s `#[cfg(test)]` module): a `SCHEDULED_DATE`-basis weekly-recurring item stale by 3 cycles lands on the correct next future date; a `DUE_DATE`-basis recurring item (recurrence untouched) falls back to "move to today"; a plain non-recurring item moves to today, same time; a case with `scheduled_end_date` set confirms the delta shift.

### 3. Bulk-apply handler

New module `src/web_ui/scheduled_overdue.rs`, registered via `pub mod scheduled_overdue;` in `src/web_ui/mod.rs` and wired into `build_web_router()` in `src/main.rs`:

- `GET /web/scheduled-overdue` — page handler modeled on `assigned_items_page` (`src/web_ui/assigned_items.rs:73-99`): calls `repo.list_scheduled_overdue(&auth_user.user_id, Utc::now().timestamp())`, builds a project-name lookup via `ProjectRepo::list_for_user`, renders one row per item (name, project name, old scheduled date, recurrence indicator) plus a "Reschedule all" button.
- `POST /web/scheduled-overdue/reschedule` — re-fetches the same eligible set server-side (never trusts a client-submitted id list, so the batch always matches what's currently displayed), and for each item:
  1. Computes the new dates via `item.reschedule_forward(now, tz)`.
  2. Builds `UpdateProjectItemParams` by round-tripping every other field from the current item (same convention as `toggle_project_dashboard_item_complete`, `src/web_ui/project_dashboard.rs:424-446`), overriding only `scheduled_date`/`scheduled_end_date`.
  3. Calls `project_items::update_project_item(...)` (`src/service/project_items.rs:178`), which already dispatches personal vs. team-backed correctly and — for free — re-anchors any offset-driven children via `sync_offset_children` if the item's anchor (`due_date.or(scheduled_date)`) actually changed.
  4. Collects success/failure per item rather than aborting the batch on the first error — following the CSV importer's best-effort precedent (`src/service/import.rs`) rather than the batch-create bail-on-first-error precedent (`create_project_tasks_batch`), since one item hitting a validation edge case shouldn't block fixing the rest.

  `complete` stays `false` throughout, so none of `update_team_item`'s completion/points machinery is triggered, and the existing `scheduled_end_date < scheduled_date` validation is automatically satisfied since both fields shift by the same delta.

  Re-renders the (now smaller or empty) list on success, same response shape as the `GET`.

### 4. Templates and nav

- `templates/scheduled_overdue/page.html`, `row.html` — modeled on `templates/assigned_items/page.html`/`row.html`. Row shows item name (linking to its detail page via the same `detail_url` pattern as `assigned_items.rs`), project name, current scheduled date, and a recurrence badge if the item has a pattern. Page has a single "Reschedule all" button (plain JS `confirm()` before the POST, consistent with a destructive-ish bulk action) — no per-row action in this first pass.
- `templates/nav_sidebar_inner.html` — add a new link next to "Assigned to me" (`templates/nav_sidebar_inner.html:24-28`), `hx-boost="false"` matching that link's convention.

## Critical files

| File | Change |
|---|---|
| `src/storage/sqlite/mod.rs` | Add `list_scheduled_overdue` to `ItemRepo` trait |
| `src/storage/sqlite/items.rs` | SQL implementation + unit tests |
| `src/domain/item.rs` | Add `reschedule_forward`, unit tests |
| `src/web_ui/scheduled_overdue.rs` (new) | Page + bulk-reschedule handlers |
| `src/web_ui/mod.rs` | `pub mod scheduled_overdue;` |
| `src/main.rs` | Register the two new routes in `build_web_router()` |
| `templates/scheduled_overdue/page.html`, `row.html` (new) | UI |
| `templates/nav_sidebar_inner.html` | Sidebar link |

## Out of scope (deferred)

- Event items — mechanism is kind-agnostic in `reschedule_forward`, so extending to Events later is just loosening the `item_type = 'TASK'` filter.
- Per-item "skip just this one" action — only "reschedule all" is being built in this pass; the same `reschedule_forward` helper would back a future per-row button.
- Prompting for a custom target date — non-recurring items always go to "today, same time" per the decision above.

## Verification (when implemented)

- New unit tests for `Item::reschedule_forward` (`src/domain/item.rs`) and `list_scheduled_overdue` (`src/storage/sqlite/items.rs`), covering: `SCHEDULED_DATE`-basis recurring item stale by several cycles lands on the correct next future date; `DUE_DATE`-basis recurring item falls back to "move to today" without touching its recurrence/due_date; non-recurring item moves to today at the same time; `scheduled_end_date` shifts by the same delta; personal-project items show regardless of assignment; team-project items are excluded unless assigned to the requesting user; children/Simple/Event/complete items never appear.
- `cargo test` for the full suite.
- Manual smoke test via `task run`: create a personal-project task with `scheduled_date` a few days in the past, and a team-project task assigned to yourself with a stale weekly `SCHEDULED_DATE`-basis recurrence; confirm both appear on `/web/scheduled-overdue`; click "Reschedule all"; confirm the recurring item lands on the correct next future date (not just "today"), the plain item lands on today at its original time, `due_date` is untouched on both, and the list is now empty.
