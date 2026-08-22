# Google Calendar import (per-project, user-supplied iCal URL)

## Context

The user is migrating off Google Calendar onto this app, but can't cut over all at once — their wife needs a polished, fully-functional calendar before she'll use it, so both systems need to coexist for a while. This plan makes Google Calendar events show up read-only inside a project's Events screen/calendar view, imported from a **private iCal URL the project admin pastes in**, not a hard-coded env var.

`family-board` (`/Users/wlapinel/family-board/src/calendar.rs`) already solves the "fetch a private Google iCal feed and cache it" problem for a single hard-coded `GCAL_ICAL_URL`, ephemeral in-memory display only (no persistence, no write-back, single fixed timezone, 14-day window, no RRULE expansion). This plan reuses its core idea (fetch iCal via `reqwest`, parse via the `ical` crate) but goes further in every direction that matters for this app: per-project subscriptions (plural), admin-gated, persisted as real `Item` rows (so they show up everywhere Events already do — list, calendar view, project dashboard), a proper create/update/delete diff against what's already imported, correct TZID handling (family-board's `parse_ical_dt` silently treats a non-`Z` local time as UTC, which is wrong whenever the source calendar's timezone isn't UTC — this plan fixes that), and full RRULE (recurring event) support.

### Design decisions confirmed with the user (2026-08-22)

- **Sync trigger: background periodic task**, not page-load-triggered. A tokio interval loop sweeps every calendar subscription roughly every 15 minutes regardless of who's viewing what; reads always hit local DB rows, no external-fetch latency on page loads.
- **Multiple calendar subscriptions per project**, each with its own iCal URL — not a single URL-per-project field. Covers the common case of merging several family members' calendars into one project.
- **Recurring (RRULE) events are in scope for this plan, not a "later, separately-requested" cut.** The user was explicit: don't make them ask again for recurring-event support. It's staged (Stage 7, below) purely because it's a materially bigger, separable piece of engineering (RRULE expansion, `RECURRENCE-ID` overrides, `EXDATE`), not because it's optional.
- **Events are already non-completable in this app** (`Item::validate()` rejects `complete: true` for `ItemType::Event` — confirmed in `src/web_ui/project_events/mod.rs`'s comments). So "imported events must be read-only" only has to worry about blocking edit/delete of name/date/etc. — there's no complete-toggle question to resolve here, unlike the personal/team completion-guard work elsewhere in this app.

### Other decisions made while planning (stated here so a later stage doesn't have to re-derive them)

- **Deleting a subscription cascades**: it deletes every `Item` row that has that `calendar_subscription_id`. Unsubscribing is the only way to bulk-remove imported events short of them disappearing from the upstream calendar.
- **No manual edit or delete of an individual imported item.** Allowing delete-but-not-edit would just have the item silently reappear on the next sync (confusing); allowing edit would have the next sync silently overwrite the user's edit (also confusing). Full read-only is the only option that isn't surprising.
- **Import window is bounded**, both to keep the `items` table from growing unboundedly and because an iCal feed can contain years of history. Recommended, tunable constants: non-recurring events import from `now - 30d` to `now + 365d`; recurring-event expansion (Stage 7) uses a tighter `now - 7d` to `now + 180d` window since it generates one row per occurrence and has to re-slide that window forward on every sync. Both are plain constants in `src/service/calendar_sync.rs`, not user-configurable in this first pass.
- **Imported items are always `ItemType::Event`** — never Task/Simple. Matches what these actually are, and Events already have their own dedicated screen (`project_events/`) for display.

## Why staged, and what's in scope per stage

Same process as `docs/archived/project-abstraction-plan.md`: each stage is independently landable, leaves the app's running behavior unchanged (or only additively changed) until wired up, and is done in its own session — **compact/clear context between stages**, with this file as the only thing that survives the handoff. Before ending a stage, update that stage's section below with an **Implementation notes** entry: exact names if they ended up different, deviations and why, test status, anything discovered that changes a later stage's assumptions.

1. **Schema only** — new `calendar_subscriptions` table, new nullable `items.google_event_id`/`items.calendar_subscription_id` columns. Nothing reads or writes them yet.
2. **Storage layer** — `CalendarSubscriptionRepo` trait + SQLite impl, plus the one new `ItemRepo` method the sync diff needs. Not called from anywhere yet.
3. **Sync engine (service layer)** — iCal fetch + parse (`ical` crate, TZID-aware via `chrono-tz`) + create/update/delete diff logic, non-recurring events only (anything with an `RRULE` is skipped here, picked up in Stage 7). Unreachable via HTTP yet — exercised only by unit tests against fixture `.ics` text.
4. **Item model + read-only enforcement** — `google_event_id`/`calendar_subscription_id` on the domain `Item`, Smithy fields (output-only), guards in `project_items`/`items`/`team_items` service functions rejecting edit/delete of an imported item, web UI badge + hidden Edit/Delete on imported rows.
5. **Subscription CRUD reachable via HTTP + the background sync task + web UI management screen.** This is the stage where the feature actually turns on.
6. **CLI + MCP parity** for subscription management (`prl`, MCP tools) — per this repo's own touch-point checklist convention.
7. **Recurring events (RRULE) expansion** — the `rrule` crate, per-occurrence synthetic ids, `RECURRENCE-ID` override handling, `EXDATE` handling, window-sliding re-sync.

---

## Stage 1 — Schema only

New migration file `src/storage/migrations/add_calendar_subscriptions.rs`, version **25** (next after `add_item_series_rotation_members`, version 24), registered in `all_migrations()` in `src/storage/migrations/mod.rs`:

```sql
CREATE TABLE IF NOT EXISTS calendar_subscriptions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    ical_url TEXT NOT NULL,
    created_by_user_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_synced_at INTEGER,
    last_sync_error TEXT
);
CREATE INDEX IF NOT EXISTS idx_calendar_subscriptions_project_id
    ON calendar_subscriptions (project_id);
```

Plus, guarded with `column_exists()` per this repo's standard pattern (SQLite has no `ADD COLUMN IF NOT EXISTS`):

```rust
if !column_exists(conn, "items", "google_event_id").await? {
    sqlx::query("ALTER TABLE items ADD COLUMN google_event_id TEXT").execute(&mut *conn).await?;
}
if !column_exists(conn, "items", "calendar_subscription_id").await? {
    sqlx::query("ALTER TABLE items ADD COLUMN calendar_subscription_id TEXT").execute(&mut *conn).await?;
}
```

Indexes (plain `CREATE INDEX IF NOT EXISTS`, no guard needed — index creation is already idempotent and order-independent as long as it runs after the columns exist, which this migration itself guarantees within the same `up()`):

```sql
CREATE INDEX IF NOT EXISTS idx_items_calendar_subscription_id ON items (calendar_subscription_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_items_calsub_google_event_id
    ON items (calendar_subscription_id, google_event_id)
    WHERE calendar_subscription_id IS NOT NULL;
```

The partial unique index is what makes the Stage 3 diff logic safe — it's a hard guarantee against ever double-importing the same upstream event into the same subscription, even if sync logic has a bug.

Also update, per the "Adding a DB column" workflow in this repo's `CLAUDE.md`:
- The baseline `CREATE TABLE IF NOT EXISTS calendar_subscriptions` and the two new `items` columns in `create_pool()` (`src/storage/sqlite/mod.rs`), so a fresh DB is correct without ever touching the migration.
- Follow the existing `idx_items_series_id`-style comment precedent if the new `items` indexes need the same "don't create before the migration that adds the column runs" ordering care on a pre-existing DB — worth double-checking against `add_item_series_id.rs`'s handling of this exact hazard (flagged as a general audit item in `docs/issues_and_features.md` already) before assuming a plain unconditional `CREATE INDEX IF NOT EXISTS` in the baseline is safe.

**Files touched:** `src/storage/migrations/add_calendar_subscriptions.rs` (new), `src/storage/migrations/mod.rs` (register it), `src/storage/sqlite/mod.rs` (baseline schema).

**Verification:** migration unit tests following `drop_team_member_points.rs`'s pattern (apply to a pre-migration schema, assert columns/table exist; apply twice, assert idempotent).

### Implementation notes (fill in before ending this stage)

_(empty until Stage 1 is actually done)_

---

## Stage 2 — Storage layer

New `CalendarSubscription` domain struct (`src/domain/calendar_subscription.rs` or alongside other small domain structs — match wherever `Team`/`Project` domain structs already live):

```rust
pub struct CalendarSubscription {
    pub id: String,
    pub project_id: String,
    pub ical_url: String,
    pub created_by_user_id: String,
    pub created_at: DateTime<Utc>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_sync_error: Option<String>,
}
```

New repo trait in `src/storage/sqlite/mod.rs`, alongside `TeamRepo`/`ProjectRepo`, `#[cfg_attr(test, mockall::automock)]`:

```rust
#[async_trait]
pub trait CalendarSubscriptionRepo: Send + Sync {
    async fn create(&self, project_id: &str, ical_url: &str, created_by_user_id: &str) -> Result<CalendarSubscription, RepoError>;
    async fn get(&self, id: &str) -> Result<CalendarSubscription, RepoError>;
    async fn list_by_project(&self, project_id: &str) -> Result<Vec<CalendarSubscription>, RepoError>;
    async fn delete(&self, id: &str) -> Result<(), RepoError>;
    async fn list_all(&self) -> Result<Vec<CalendarSubscription>, RepoError>; // for the Stage 5 background sweep
    async fn record_sync_result(&self, id: &str, synced_at: DateTime<Utc>, error: Option<String>) -> Result<(), RepoError>;
}
```

Implement in `src/storage/sqlite/calendar_subscriptions.rs`, following `src/storage/sqlite/projects.rs`'s query-building conventions.

One new `ItemRepo` method (`src/storage/sqlite/mod.rs`, alongside `list_by_source_event`):

```rust
async fn list_by_calendar_subscription(&self, calendar_subscription_id: &str) -> Result<Vec<Item>, RepoError>;
```

This is the full current set of imported items for one subscription — what Stage 3's diff compares the freshly-parsed iCal feed against. `delete` is reused as-is for removing an item that's dropped out of the feed (or cascaded when a subscription itself is deleted); no new delete method needed. Item **creation** during sync also reuses the existing `create`/`create_by_project`-shaped insert — Stage 3 will need to confirm whether the existing `create_by_project` signature already accepts a full `Item` (including the new `google_event_id`/`calendar_subscription_id` fields once Stage 4 adds them to the struct) or needs a thin variant; note the answer here once Stage 3 is done, since Stage 3 depends on Stage 4's field addition to actually persist these two columns — **Stage 3 and Stage 4 have a real ordering dependency: do Stage 4's domain-struct-only slice (not the read-only enforcement/Smithy part) before or alongside Stage 3, or Stage 3's writes will silently drop the two new fields.** (Flagging this now so it isn't rediscovered mid-Stage-3; consider merging Stage 3 and the domain-struct half of Stage 4 into one session if that's cleaner in practice.)

Not called from any service/handler code yet — unit-tested in isolation only (in-memory SQLite pool, same pattern every other repo test file uses).

**Files touched:** `src/domain/calendar_subscription.rs` (new), `src/storage/sqlite/mod.rs` (trait + `Domain`/`ItemRepo` addition), `src/storage/sqlite/calendar_subscriptions.rs` (new), `src/storage/sqlite/items.rs` (`list_by_calendar_subscription` impl + row mapping if Stage 4's columns are pulled forward here).

### Implementation notes

_(empty until Stage 2 is actually done)_

---

## Stage 3 — Sync engine (service layer, not yet reachable via HTTP)

New file `src/service/calendar_sync.rs`. New Cargo dependencies: `ical = "0.10"` and `chrono-tz` (matching the versions `family-board` already uses successfully). `reqwest` is already a dependency here (used for OAuth) — reuse it for the iCal fetch.

```rust
pub struct ParsedIcalEvent {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub all_day: bool,
    pub has_rrule: bool, // Stage 3 skips these; Stage 7 expands them
}

pub enum CalendarSyncError { Fetch(String), /* non-2xx, timeout, etc. */ }

pub fn parse_ical(content: &str) -> Vec<ParsedIcalEvent> { ... }
pub async fn fetch_ical(url: &str) -> Result<String, CalendarSyncError> { ... }

pub struct SyncSummary { pub created: usize, pub updated: usize, pub deleted: usize, pub skipped_recurring: usize }

pub async fn sync_subscription(
    subscription: &CalendarSubscription,
    item_repo: &dyn ItemRepo,
    calendar_repo: &dyn CalendarSubscriptionRepo,
) -> Result<SyncSummary, CalendarSyncError> { ... }
```

**TZID handling — the actual fix over `family-board`'s version.** `family-board`'s `parse_ical_dt` strips a trailing `Z` and otherwise treats the naive local string as if it were UTC — silently wrong for any `DTSTART;TZID=America/New_York:...`-style value (the common case for a calendar whose primary timezone isn't UTC). This plan's `parse_ical` instead: reads the `DTSTART`/`DTEND` property's `TZID` param when present, resolves it via `chrono_tz::Tz::from_str(tzid)`, localizes the naive datetime in that zone, then converts to UTC. Falls back to literal UTC only when the value ends in `Z` (already UTC) or has no `TZID` at all (rare, but some all-day-adjacent edge cases hit this) and there's no better signal. All-day events (`VALUE=DATE`, 8-digit value) keep `family-board`'s existing date-only handling — no time-of-day parsing needed for those, `all_day: true`, `end` computed as `start + 1 day` if `DTEND` gives an exclusive end date (RFC 5545 convention for all-day ranges), consistent with how `has_scheduled_time`/`has_end_time` distinguish this elsewhere in this app's own domain model.

**Diff logic**, given `parsed: Vec<ParsedIcalEvent>` (RRULE ones already filtered out — `has_rrule` events counted into `skipped_recurring` and otherwise ignored until Stage 7) and `existing: Vec<Item>` from `list_by_calendar_subscription`:

- Build `existing_by_uid: HashMap<String, &Item>` keyed on `google_event_id`.
- For each parsed event not in `existing_by_uid` → create (`ItemType::Event`, `name = summary`, `description`, `scheduled_date = start`, `scheduled_end_date = end`, `has_scheduled_time = !all_day`, `has_end_time = end.is_some() && !all_day`, `google_event_id = uid`, `calendar_subscription_id = subscription.id`, `project_id = subscription.project_id`).
- For each parsed event matching an existing item by uid → diff `summary`/`description`/`start`/`end`/`all_day` against the current item; call `update`/`update_by_project` only if something actually changed (avoids needless writes on every 15-minute sweep for unchanged events).
- For each existing imported item whose uid isn't in the freshly-parsed set → delete (it's gone from — or now outside the import window of, or now `CANCELLED` in — the upstream calendar).
- `STATUS:CANCELLED` events are filtered out during parse (same as `family-board`), so a cancelled-but-still-present-in-the-feed event is treated identically to a removed one — deleted if previously imported, never (re-)created.
- On success: `calendar_repo.record_sync_result(id, now, None)`. On fetch/parse failure: `record_sync_result(id, now, Some(error_message))`, leaving whatever items already existed untouched (a transient Google outage shouldn't delete anyone's calendar).

These writes go straight through the repo (`item_repo.create`/`update`/`delete`), **not** through `service::project_items::create_project_item`/`update_project_item` — those enforce per-request membership/role checks that don't apply here (the sync process already established its authority when the subscription was created by a project admin in Stage 5), and Stage 4's read-only guard on those very functions would otherwise reject the sync's own writes to a `google_event_id`-bearing item. This is a deliberate, documented exception to the "always go through the service layer" convention elsewhere in this app, scoped narrowly to this one sync path.

**Unit tests**: fixture `.ics` strings (a plain timed event, an all-day event, a `TZID`-bearing event in a non-UTC zone, a `CANCELLED` event, an `RRULE`-bearing event confirming it's skipped/counted not imported) run through `parse_ical` directly; `sync_subscription`'s diff logic tested against a mock `ItemRepo`/`CalendarSubscriptionRepo` (via `mockall`) covering create/update/delete/no-op cases.

**Files touched:** `Cargo.toml` (`ical`, `chrono-tz`), `src/service/calendar_sync.rs` (new), `src/service/mod.rs` (`pub mod calendar_sync;`).

### Implementation notes

_(empty until Stage 3 is actually done)_

---

## Stage 4 — Item model + read-only enforcement

**Domain** (`src/domain/item.rs`): add `google_event_id: Option<String>`, `calendar_subscription_id: Option<String>` to `Item`. Follows `series_id`'s precedent exactly (`CLAUDE.md`'s Domain Models section) — set once at creation (by the Stage 3 sync path only), carried forward unchanged on every update, `None` for every non-imported item.

**Storage row mapping** (`src/storage/sqlite/items.rs`): both columns added to every `SELECT`/`INSERT`/row-mapping site that already carries `series_id`.

**Smithy** (`model/src/main/smithy/project_item.smithy`): add both fields to `ProjectItem`'s `properties`, reference `$googleEventId`/`$calendarSubscriptionId` **only** in `GetProjectItem`'s output, `ListProjectItems`' output, and `ProjectItemSummary` — deliberately **not** referenced in `CreateProjectItem`'s or `UpdateProjectItem`'s `input :=` structures. This is the same mechanism `hasChildren` already uses to be server-computed/read-only at the Smithy level (no client can ever pass these fields in a request body, full stop — stronger than a service-layer check alone). Run `task codegen`.

**Service-layer guard**, belt-and-suspenders on top of the Smithy-level protection above (defense in depth, and this is the layer that actually produces a clean user-facing error rather than a generic deserialization failure): `service::project_items::update_project_item`/`delete_project_item` (and the `items.rs`/`team_items.rs` functions they dispatch to) reject any update/delete where `current.google_event_id.is_some()`, returning a new `ItemError::ImportedItemReadOnly` (or reuse `ItemError::Invalid` with a specific message, matching the `TEMPLATE`-rejection precedent's error shape) — `src/web_ui/error.rs`'s existing `IntoResponse for ItemError` picks this up automatically once the variant exists.

**Web UI** (`src/web_ui/project_events/`, `templates/project_events/*.html`): 
- Row template: small badge/label (e.g. a calendar icon + "Google Calendar") on rows where `google_event_id.is_some()`.
- Read-only detail view: hide the "Edit" link and any delete action the same way `complete == true` already hides Edit elsewhere in this app (see the Completion-transition-guards precedent in `CLAUDE.md`) — same conditional-render pattern, different predicate.
- List/calendar-view row-level delete action (if Events rows currently expose one inline) similarly hidden for imported rows.

**CLI/MCP**: `prl items get`/`list` and the MCP `get_item`/`list_items` output should surface `googleEventId` (read-only) so a user can tell an item came from an import; `prl items update`/`create_item`/`update_item` need no change since the Smithy input shapes simply don't carry the field.

**Files touched:** `src/domain/item.rs`, `src/storage/sqlite/items.rs`, `model/src/main/smithy/project_item.smithy`, `src/service/error.rs`, `src/service/project_items.rs` (+ `items.rs`/`team_items.rs`), `src/json_api/mod.rs` (field mapping), `src/web_ui/project_events/*`, `templates/project_events/*.html`, `todo-cli/src/items.rs` (display), `mcp-server/src/index.ts` (schema/display).

### Implementation notes

_(empty until Stage 4 is actually done)_

---

## Stage 5 — Subscription CRUD via HTTP + background sync task + web UI

**Smithy** (new `model/src/main/smithy/calendar_subscription.smithy`, registered in `model/smithy-build.json` same as the others): plain operations (not a Smithy `resource`, matching `Project`'s/`Team`'s own precedent, since there's no natural single-identifier CRUD shape beyond "list by project"):

```
CreateCalendarSubscription  POST /projects/{projectId}/calendar-subscriptions      { icalUrl } -> { id }
ListCalendarSubscriptions   GET  /projects/{projectId}/calendar-subscriptions      -> [{ id, icalUrl, createdByUserId, createdAt, lastSyncedAt, lastSyncError }]
DeleteCalendarSubscription  DELETE /projects/{projectId}/calendar-subscriptions/{id}
```

No `Update` operation — changing the URL is delete-and-recreate (simpler; a subscription's identity/history isn't meaningful enough to warrant edit-in-place). `task codegen`, then wire handlers in `src/main.rs`.

**Service layer** (`src/service/calendar_subscriptions.rs`): `create_calendar_subscription`/`list_calendar_subscriptions`/`delete_calendar_subscription`, gated via `require_project_admin` for create/delete (per the user's explicit "only project admin" requirement) and `require_project_member` for list (any project member can see what's subscribed, matching how project membership already gates read access elsewhere). `delete_calendar_subscription` also deletes every `Item` with that `calendar_subscription_id` (the cascade decision from Context above) — do this in the service function, not the repo, so it goes through normal item-delete bookkeeping if any exists (e.g. if delete ever gains side effects later, this stays correct for free). `create_calendar_subscription` triggers an immediate `calendar_sync::sync_subscription` call right after insert (synchronously, awaited in the handler) so the admin sees events populate immediately rather than waiting up to 15 minutes.

**Background task** (`src/main.rs`, near where `create_pool()`/other startup wiring happens): this is a genuinely new pattern for this codebase — there's no existing `tokio::spawn`-based background loop anywhere in `src/main.rs` today, so this is worth a comment explaining why it's here. After building the repo `Arc`s used elsewhere for `server::Extension`, spawn:

```rust
let sync_repos = (item_repo.clone(), calendar_repo.clone());
tokio::spawn(async move {
    let (item_repo, calendar_repo) = sync_repos;
    loop {
        tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        match calendar_repo.list_all().await {
            Ok(subs) => for sub in subs {
                if let Err(e) = calendar_sync::sync_subscription(&sub, item_repo.as_ref(), calendar_repo.as_ref()).await {
                    tracing::warn!(subscription_id = %sub.id, error = ?e, "calendar sync failed");
                }
            },
            Err(e) => tracing::error!(error = ?e, "failed to list calendar subscriptions for sync sweep"),
        }
    }
});
```

Sleep-first (rather than sync-immediately-then-sleep) is deliberate: `create_calendar_subscription`'s own inline sync already covers "just added," so the periodic sweep's first useful run is naturally ~15 minutes after startup, avoiding a startup-time thundering-herd fetch against every subscription across every project.

**Web UI** (new `src/web_ui/project_calendar_subscriptions.rs`, `templates/project_calendar_subscriptions/*.html`): `/projects/:project_id/calendar-subscriptions` — list of subscriptions (masked/truncated URL, last-synced time, error if any), an add form (iCal URL input, admin-only — hidden/403 for non-admins via `is_project_admin`, mirroring how points/assignment fields are gated elsewhere), delete button per row (admin-only, with the cascade-delete warning made explicit in a confirm dialog since it removes imported events too). Link to this screen from the Events screen (`project_events/`) header, visible only to admins (`is_project_admin`), e.g. "Manage Google Calendars."

**Files touched:** `model/src/main/smithy/calendar_subscription.smithy` (new), `model/smithy-build.json`, `src/main.rs` (handlers + background task spawn), `src/service/calendar_subscriptions.rs` (new), `src/web_ui/mod.rs` (`pub mod project_calendar_subscriptions;`), `src/web_ui/project_calendar_subscriptions.rs` (new), `templates/project_calendar_subscriptions/*.html` (new), `templates/project_events/*.html` (admin-only nav link), `src/json_api/mod.rs`.

### Implementation notes

_(empty until Stage 5 is actually done)_

---

## Stage 6 — CLI + MCP parity

Per this repo's own touch-point checklist convention (new operations must reach `prl` and the MCP server too):

- `prl projects calendar add --project <id> --url <ical-url>` / `prl projects calendar list --project <id>` / `prl projects calendar remove --project <id> <subscription-id>` (`todo-cli/src/projects.rs`, mirroring `prl projects members`/`set-role`'s existing shape).
- MCP tools `create_calendar_subscription`/`list_calendar_subscriptions`/`delete_calendar_subscription` (`mcp-server/src/index.ts`), `projectId`-required like the other project-scoped tools.
- `docs/prl-user-guide.md` documents the new subcommands.

**Files touched:** `todo-cli/src/projects.rs`, `todo-cli/src/main.rs` (subcommand wiring), `mcp-server/src/index.ts`, `docs/prl-user-guide.md`.

### Implementation notes

_(empty until Stage 6 is actually done)_

---

## Stage 7 — Recurring events (RRULE) expansion

This is the piece `family-board` never had to solve (it doesn't expand recurrence at all) and is the most novel part of this whole plan — isolated as its own stage deliberately, same reasoning `project-abstraction-plan.md`'s Stage A4 gave for isolating its highest-risk/most-novel piece alone.

**New dependency**: the `rrule` crate (RFC 5545 `RRULE` expansion, `chrono`-based) — confirm current crates.io version and API shape at implementation time; this is a less universally-used crate than `ical`/`chrono-tz` so its API surface should be checked fresh rather than assumed from memory.

**What changes in `calendar_sync.rs`:**

- `ParsedIcalEvent` gains the raw `RRULE` string (when present), plus the VEVENT's own `EXDATE` list and — separately — any **override** VEVENT blocks in the same feed that share the same `UID` and carry a `RECURRENCE-ID` (RFC 5545's mechanism for "this one occurrence of the series was moved/renamed/cancelled").
- For each RRULE-bearing master event: expand occurrences within the bounded window (`now - 7d` to `now + 180d`, per the Context section's constants) via the `rrule` crate, skipping any timestamp present in `EXDATE`.
- Each generated occurrence gets a **synthetic, deterministic** `google_event_id`: `"{uid}::{occurrence_start_rfc3339}"`. Deterministic on the *original, unmodified* RRULE-computed timestamp (not a display timestamp that could shift) is what makes this stable across syncs — the same occurrence produces the same id every time, so Stage 3's existing create/update/delete-diff machinery (keyed on `google_event_id`) works completely unchanged for expanded occurrences; no diff-logic rewrite needed, only a richer id-generation and a richer "what does `parsed` contain" step upstream of it.
- A `RECURRENCE-ID`-bearing override VEVENT is matched to the expanded slot whose original (pre-override) timestamp equals the override's `RECURRENCE-ID` value, and its fields (summary/start/end/etc. — the override can move the occurrence's own time) replace that slot's generated fields *before* the diff step runs, using the **same synthetic id** as the slot it's overriding (so it updates the existing imported item in place rather than creating a duplicate). An override with `STATUS:CANCELLED` removes that one slot from the parsed set entirely (equivalent to an implicit `EXDATE` for that occurrence) — it'll be diffed away as deleted if it was previously imported.
- **Window sliding**: because the window is relative to "now," a previously-imported occurrence that's aged out the back of the window (`start < now - 7d`) needs to stop being tracked without looking like a deletion the user should worry about — same code path as any other diff-delete (it's gone from the freshly-parsed set), just worth calling out that this is *expected, routine* churn for a recurring series, not a sign of something wrong. No special-casing needed in the diff logic itself, just worth a code comment so it isn't "fixed" by mistake later.

**Files touched:** `Cargo.toml` (`rrule`), `src/service/calendar_sync.rs` (expansion logic, override/EXDATE handling, expanded fixture `.ics` tests — a weekly recurring event, one with a `RECURRENCE-ID` override, one with an `EXDATE`, one cancelled-occurrence override).

**Verification**: this stage is worth a live smoke test against a real Google Calendar recurring event (e.g. the user's own weekly-recurring test event) in addition to fixture-based unit tests, given how easy RRULE/timezone interactions are to get subtly wrong.

### Implementation notes

_(empty until Stage 7 is actually done)_
