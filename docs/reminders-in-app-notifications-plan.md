# Reminder read-only UI + in-app notifications

Status: **implemented (Stages D and E both landed, 2026-08-28)**. Written 2026-08-28 after a design discussion. Scoped down `docs/issues_and_features.md`'s "Add reminder schema and UI to tasks, series, and events" (UI half only — schema landed as Stages A–C, `07f591d`/`7059ff3`/`1c332e0`) and "Add in-app notifications for reminders." — the latter is now archived in `docs/archived/archived_issues_and_features.md`; the former is narrowed to "custom reminder mutation UI" only, still open. **Explicitly excluded** "Turn app into a PWA" and "Add push notifications" — those need a service worker + VAPID subscriptions + PWA manifest, a materially bigger and more platform-fussy lift (iOS requires an installed PWA before Web Push works at all), and are deferred to their own future plan once this one proves out the data model.

## Current state (confirmed by reading the code, not the stale docs bullet)

- `reminders` table + migration, `ReminderKind` enum, `ReminderRepo` (`sync_auto_reminders`/`delete_for_item`/`list_for_item`), and `service::reminders::sync_item_reminders` all exist and are wired into every create/update/delete path (`src/service/project_items.rs`).
- **Nothing reads a reminder anywhere.** `list_for_item` is dead code. There is no reminder UI at all — not even read-only — and no notification delivery of any kind.
- No background process exists. The only precedent for one is the Google Calendar sync sweep (`src/main.rs:407-429`, a `tokio::spawn` sleep-loop calling `sync_all_subscriptions` every 15 min) — the first and only background loop in this codebase.
- **This plan doesn't need that precedent.** Since delivery here is in-app only (no push), "checking periodically" is just the browser's own htmx poll hitting the server; the server can filter `remind_at <= now()` at request time. No sweep, no new background loop, no new migration for scheduling — see Stage E.

## Two pre-existing bugs found during this design pass

Two correctness gaps in the already-shipped Stage A–C code surfaced while designing Stage E below. Both were tracked as their own standalone entries in `docs/issues_and_features.md` (found 2026-08-28), separately from this plan:

1. **`sync_auto_reminders` clobbers dismissal state.** It did a blind delete-then-reinsert of every `AUTO` row on every item edit. The moment anything writes to `Reminder.sent_at` (e.g. a "dismiss" action), an unrelated edit to the item — with the reminder's `remind_at` unchanged — reset it back to unsent, since the resync didn't compare against what's already there. **Fixed 2026-08-28** as a prerequisite for Stage E's dismiss endpoint (see below) — `sync_auto_reminders` now upserts keyed by `(item_id, kind)`, preserving `sent_at` when `remind_at` is unchanged. Archived in `docs/archived/archived_issues_and_features.md`.
2. **`sync_item_reminders` ignores `item.complete`.** A completed item's already-past reminder row stays in the table forever with nothing marking it stale. **Not fixed** — still open in `docs/issues_and_features.md`. Stage E's completed-item filtering (below) works around it by filtering at read time instead of waiting for a write-side fix.

**Dependency this created for Stage E below:** Stage E's "dismiss" feature would not have been correct without bug 1 fixed first — without it, a dismissed notification could reappear on the next unrelated edit to its item. Bug 1 was fixed before Stage E shipped, per this note. Bug 2 doesn't block Stage E, since the completed-item filter works around it at read time.

## Stage D: read-only reminders section on item detail pages

Mirrors the "Depends on" convention already shipped in `ProjectTaskDetailView` (`src/web_ui/project_tasks/templates.rs:574-577`, resolved in the handler via `resolve_depends_on_links` and rendered in `templates/project_tasks/detail_view.html:84-94`) — same shape, no links needed since a reminder isn't a separate navigable entity.

- Add `reminders: Vec<String>` (pre-formatted labels, e.g. `"Due reminder: Aug 30, 5:00 PM"`, `"Scheduled start reminder: ..."`) to `ProjectTaskDetailView` and `ProjectEventDetailView` (event's equivalent struct/template — check current name in `src/web_ui/project_events/templates.rs`).
- New small helper (`src/web_ui/reminders.rs` or inline in each screen per this codebase's "duplicate small per-screen helper" precedent — see `CLAUDE.md`'s Web UI section): `fn reminder_labels(reminders: &[Reminder], tz: i32) -> Vec<String>` mapping `ReminderKind` to a display prefix (`Due` / `Scheduled start` / `Scheduled end`) + `format_display_date(to_local(r.remind_at, tz), true)`.
- Wire `reminders.list_for_item(&item.id).await` into the GET detail handler and the create/update handlers, at the same call sites that already resolve `depends_on` (`src/web_ui/project_tasks/handlers.rs:199-200`, and the two other `ProjectTaskDetailView::from_item` call sites at lines 1042/1230). `ReminderRepo` is already `Extension`-injected into every one of these handlers (used today only for the write-path sync) — no new wiring needed to get the repo into scope.
- Add the equivalent block to `templates/project_tasks/detail_view.html` and its Event counterpart — a `{% if !reminders.is_empty() %}` dt/dd row listing each label, no interactivity (per the "read-only for now" scope decision).
- **Not in scope for this stage:** Simple/Template items (no date fields / no real commitment — `sync_item_reminders` already excludes them, see `src/service/reminders.rs:26-31`), and any add/edit/delete UI or custom offset config.

## Stage E: in-app notifications — badge + list + dismiss

Cross-project by nature (a reminder's `user_id` spans every project that user belongs to), so this follows `assigned_items.rs`'s established shape (`src/web_ui/assigned_items.rs`) rather than any per-project screen: one flat query, a local `detail_url` helper duplicated per that file's own precedent (lines 38-45) rather than shared, `TzOffset` for display formatting.

### New repo methods (`ReminderRepo`, `src/storage/sqlite/mod.rs` + `src/storage/sqlite/reminders.rs`)

```rust
/// Reminders due (remind_at <= now) and not yet dismissed, for `user_id`, across every
/// project — the query the notification badge/list poll against. Excludes reminders whose
/// item is already complete (checked as a second small lookup against ItemRepo per result,
/// not a SQL join — see design note below; result sets here are small by construction).
async fn list_due_for_user(&self, user_id: &str, now: DateTime<Utc>) -> Result<Vec<Reminder>, RepoError>;

/// Marks one reminder dismissed (`sent_at = now()`), scoped to `user_id` so one user can't
/// dismiss another's reminder by guessing an id.
async fn dismiss(&self, id: &str, user_id: &str) -> Result<(), RepoError>;
```

`list_due_for_user`'s SQL: `SELECT ... FROM reminders WHERE user_id = ? AND remind_at <= ? AND sent_at IS NULL ORDER BY remind_at ASC`. Deliberately **not** joined against `items` in SQL — filtering out a completed item's stale reminder is a correctness fix (see below) best done by reusing `ItemRepo::get_by_project`, which the caller needs anyway to build the display label, rather than adding a cross-table join to this repo's trait for the first time. If the list ever grows large enough for N+1 lookups to matter, revisit then — not a premature concern for a personal notification list.

This works around bug 2 above (`sync_item_reminders` never clearing a completed item's stale reminder row) by filtering at read time instead: the service function below (`list_due_notifications_for_user`) fetches each candidate reminder's item via `ItemRepo` and drops any where `item.complete`. Simpler than teaching the write-side sync about completion, and self-heals if an item is later un-completed (the "unify completion-undo" pass, `CLAUDE.md`'s Points section) without extra bookkeeping — so, unlike bug 1, this one doesn't block Stage E.

### New service function (`src/service/reminders.rs`)

```rust
pub struct DueNotification {
    pub reminder: Reminder,
    pub item_name: String,
    pub detail_url: String,
}

pub async fn list_due_notifications_for_user(
    reminders: &Arc<dyn ReminderRepo>,
    items: &Arc<dyn ItemRepo>,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<DueNotification>, ItemError>
```

Calls `list_due_for_user`, then for each row fetches the item via `items.get_by_project(&r.project_id, &r.item_id)`, skips it if `item.complete` or the item is missing (deleted since the reminder was created — `delete_for_item` should already prevent this in the normal path, but don't assume), and builds `detail_url` via a small local `ItemKind`-match helper (same shape as `assigned_items.rs::detail_url`, duplicated per that file's own precedent rather than extracted — Task/Event only, since `sync_item_reminders` never creates rows for Simple/Template).

### New web_ui module: `src/web_ui/notifications.rs`

Three routes, registered in `build_web_router()` alongside `assigned_items.rs`'s route:

- `GET /web/notifications/badge` — tiny fragment: a count (or nothing, rendered as an empty/hidden badge, if zero). This is the `hx-trigger="load, every 30s"` poll target — the header's bell icon badge span polls this directly, no page-level involvement.
- `GET /web/notifications` — the dropdown/list fragment: each `DueNotification` as a row (item name linked via `detail_url`, kind label, localized `remind_at`, a "Dismiss" button `hx-post`ing to the third route).
- `POST /web/notifications/:id/dismiss` — calls `ReminderRepo::dismiss(id, user_id)`, then re-renders and returns the list fragment (same `hx-target`/`hx-select` scoping convention used everywhere else in this codebase — see `CLAUDE.md`'s row-editing convention — rather than introducing `hx-swap-oob`, which has zero precedent in this codebase's templates). The badge simply catches up on its own next poll (worst case ~30s of a stale count after a dismiss) — a deliberate simplicity trade-off, not an oversight; revisit only if that lag is actually reported as annoying.

### Header wiring (`templates/base.html`)

A bell icon + count badge in the header (visible on every page, not gated by `ActiveContext`, since notifications are user-global). Use a `<details>/<summary>` disclosure for the open/close dropdown state rather than new JS/localStorage state — no reason to follow the sidebar's `localStorage` pattern here since there's nothing to persist across loads (a closed dropdown on every fresh page load is the correct default, unlike the sidebar's collapsed/expanded preference).

## Explicitly out of scope for this pass

- Custom reminder mutation UI (add/edit/delete a reminder, configurable offsets like "15 min before") — `source = 'CUSTOM'` stays unused; a later stage once this read-only + notification pass proves the model out.
- PWA manifest/service worker and real push notifications (`docs/issues_and_features.md` items 1 and 4) — needs its own plan; shares no code with this one except that a future push-delivery stage would want the same `list_due_for_user`-style query, run from a background sweep (the calendar-sync precedent) instead of driven by a live poll.
- Email notifications — not on the backlog for this pass either.
- Any change to `sync_item_reminders`'s recipient-resolution logic (personal owner / team assignee) — unaffected by this plan.

## Touch-point checklist

| File | Change |
|------|--------|
| `src/storage/sqlite/reminders.rs` | Upsert fix in `sync_auto_reminders`; add `list_due_for_user`, `dismiss` |
| `src/storage/sqlite/mod.rs` | Add the two new methods to `ReminderRepo` trait (+ mock via `automock`) |
| `src/service/reminders.rs` | Add `list_due_notifications_for_user` + `DueNotification` |
| `src/web_ui/project_tasks/templates.rs`, `.../project_events/templates.rs` | Add `reminders: Vec<String>` field to detail view structs |
| `src/web_ui/project_tasks/handlers.rs`, `.../project_events/handlers.rs` | Resolve reminder labels at each `*DetailView::from_item` call site |
| `templates/project_tasks/detail_view.html`, `templates/project_events/detail_view.html` | Read-only "Reminders" dt/dd block |
| `src/web_ui/notifications.rs` (new) | Badge/list/dismiss handlers |
| `src/web_ui/mod.rs` | `pub mod notifications;` |
| `src/main.rs` | Register the three new routes in `build_web_router()` |
| `templates/base.html` | Bell icon + badge markup, polling `hx-trigger` |
| `templates/notifications/*.html` (new) | Badge fragment, list fragment |
| `docs/issues_and_features.md` | Once landed, narrow the two bullets (schema/UI item becomes "custom mutation UI only"; in-app notifications item moves to archived) rather than deleting outright |
