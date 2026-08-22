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

**"Independently landable" describes the code, not a standing instruction to `git commit`.** Per this repo's global CLAUDE.md/git policy, commits only happen when the user explicitly asks — finishing a stage does not imply consent to commit it. Each stage being self-contained just means it's *safe* to commit on its own (and to do so without waiting for later stages) once the user actually asks.

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

Done as planned, no deviations from the design above. Concretely:

- `src/storage/migrations/add_calendar_subscriptions.rs` — version **25**, name
  `AddCalendarSubscriptions`. Registered in `all_migrations()` in
  `src/storage/migrations/mod.rs` (mod + use + push, alphabetical-ish position
  matching the existing list's rough ordering).
- Table + both `items` columns + all three new indexes exactly as specified, including
  the partial unique index `idx_items_calsub_google_event_id` on
  `(calendar_subscription_id, google_event_id) WHERE calendar_subscription_id IS NOT
  NULL`.
- Followed `AddProjects`'s precedent exactly on the index-ordering question (this
  plan's own §Stage 1 already flagged this correctly, confirmed against the real
  `add_projects.rs`/`add_item_series_id.rs` source before writing): a brand-new table
  (`calendar_subscriptions`) is safe to create — table *and* its index — directly in
  both the migration and the `create_pool()` baseline, since a `CREATE TABLE IF NOT
  EXISTS` is a no-op either way. An index on a column added to an *existing* table
  (`items.calendar_subscription_id`, `items.google_event_id`) must only ever be
  created inside the migration itself, never the baseline — baseline `CREATE INDEX`
  statements run unconditionally before `run_migrations()`, so a baseline index on a
  not-yet-existing column breaks any DB that predates the migration. Both new `items`
  indexes therefore live only in `add_calendar_subscriptions.rs::up()`, not in
  `create_pool()`. The two new `items` *columns* themselves (not their indexes) were
  still added to the baseline `CREATE TABLE items` in `src/storage/sqlite/mod.rs`, so a
  fresh DB is correct without ever touching the migration, per the standard "Adding a
  DB column" workflow in CLAUDE.md.
- `src/storage/migrations/mod.rs`'s three `#[cfg(test)]` fixtures/assertions updated:
  `current_schema_pool()` now includes `google_event_id`/`calendar_subscription_id` on
  its `items` table (it does **not** include a `calendar_subscriptions` table itself —
  that's consistent with this fixture's existing pattern, e.g. it also has no
  `item_series_rotation_members` table even though that migration predates this one;
  the fixture isn't a strict mirror of the true current schema, migrations for
  brand-new tables just create them fresh when run against it, and the
  no-op/idempotency tests only assert on `_migrations` row count, not on absence of
  schema change). All three `assert_eq!(applied_count, 24)` bumped to `25`.
- New migration file's own tests (3): create-table-and-columns + idempotency (run
  `up()` three times), plus one exercising the partial unique index directly at the
  SQL level (`rejects_double_importing_the_same_google_event_into_the_same_subscription`)
  — confirms both that a duplicate `(calendar_subscription_id, google_event_id)` pair
  is rejected and that ordinary `NULL`-subscription items are never constrained by it.
- **Verified**: `cargo test` — 411 passed, 0 failed (up from 408 pre-stage; +3 new
  migration tests). `task check` — clean, no new warnings introduced (only
  pre-existing unrelated dead-code warnings elsewhere in the crate).
- Nothing discovered that changes any later stage's assumptions. Stage 2's own note
  about the Stage 2/3/4 field-plumbing ordering dependency (see that section above)
  still stands as originally written — this stage only added the raw columns, nothing
  in the domain `Item` struct or any repo method reads/writes them yet.

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

Done as planned, with a couple of small signature deviations from the sketch above
(neither changes behavior, both just match this codebase's actual conventions more
closely than the plan's pseudocode did):

- `src/domain/calendar_subscription.rs` (new) — `CalendarSubscription` struct exactly
  as specified.
- `src/domain/mod.rs` — `pub mod calendar_subscription;` added.
- `src/storage/sqlite/mod.rs`:
  - `CalendarSubscriptionRepo` trait added alongside `ProjectRepo`/before
    `ActivityLogRepo`, `#[cfg_attr(test, mockall::automock)]` like every other repo
    trait. **Deviation**: none of its methods needed the explicit `<'a>` lifetime
    dance `ProjectRepo::create` required — that fix was specifically for a param
    wrapped in `Option<&'a str>` (elision doesn't reach inside `Option`); this trait's
    `create` takes three plain `&str`s (no `Option`), which elides fine on its own,
    matching `ItemRepo::get`'s two-plain-`&str`-params precedent instead.
  - `record_sync_result`'s `error` param stayed `Option<String>` (owned), exactly as
    planned — no lifetime issue there either since it's owned, not borrowed.
  - `ItemRepo::list_by_calendar_subscription` added exactly as planned, with a doc
    comment flagging that `calendar_subscription_id`/`google_event_id` aren't domain
    `Item` fields yet (Stage 4) — this method's `WHERE` clause is the only place either
    raw DB column is touched until then.
  - `row_to_calendar_subscription` helper added alongside `row_to_activity_log_entry`,
    same `DateTime::from_timestamp`-conversion pattern.
- `src/storage/sqlite/calendar_subscriptions.rs` (new) — `SqliteCalendarSubscriptionRepo`,
  following `projects.rs`'s query-building conventions (`SELECT` constant + `format!`,
  `db_err`/`not_found` helpers). 8 unit tests: create/get round-trip, get-missing,
  list-by-project (scoping + ordering), list-all (cross-project), delete + delete-missing,
  record-sync-result (both the error-recorded and success-clears-error cases) +
  record-sync-result-missing.
- `src/storage/sqlite/items.rs` — `list_by_calendar_subscription` implemented following
  `list_by_source_event`'s exact shape (same `ITEM_SELECT` constant, same
  `ORDER BY COALESCE(due_date, ...)`). Its two new tests seed
  `calendar_subscription_id` via a raw `UPDATE items SET ...` (not through
  `repo.create`), since the domain `Item` struct has no such field yet — Stage 4 will
  make this natural; until then this is the only way to exercise the column at the
  repo-test level. The test-only in-memory `items` table in this file's `test_pool()`
  gained `google_event_id`/`calendar_subscription_id` columns (both nullable, matching
  the real schema) so those raw-SQL test inserts have somewhere to write.
- **Not wired anywhere**: no `Arc<dyn CalendarSubscriptionRepo>` was added to
  `main.rs`'s `server::Extension` wiring — per the plan, Stage 2 stays unreachable from
  any service/handler code. `cargo check` shows the expected new "never used"
  warnings for the whole new trait/impl/struct (same shape as pre-existing
  `UserRepo::create`/`delete` warnings), nothing unexpected.
- **Verified**: `cargo test` — 421 passed, 0 failed (up from Stage 1's 411, +10: 8 new
  `calendar_subscriptions.rs` tests + 2 new `items.rs` tests). `cargo check` — clean,
  no new warnings beyond the expected "never used" ones noted above.
- Nothing discovered that changes Stage 3/4/5's assumptions. The Stage 3/4 ordering
  dependency flagged in this stage's own planning section above (Stage 3's sync writes
  need Stage 4's domain-struct fields to actually persist `google_event_id`/
  `calendar_subscription_id`) still stands, untouched by this stage — Stage 2 only
  added the repo methods/trait, not the domain fields themselves.

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

Done as planned, with one deliberate scope merge and a couple of structural deviations:

- **Merged in the domain-struct-only slice of Stage 4**, exactly as this plan's own
  Stage 2 note suggested, since Stage 3's writes are meaningless without it: added
  `google_event_id`/`calendar_subscription_id: Option<String>` to the domain `Item`
  struct (`src/domain/item.rs`), following `series_id`'s precedent exactly (doc comment
  says the same thing — set once at creation by the sync path only, carried forward
  unchanged on every update, `None` for non-imported items). Wired both columns through
  every site that already carries `series_id` in `src/storage/sqlite/items.rs`
  (`ITEM_SELECT`, `create`'s INSERT, both `update`/`update_by_project` UPDATEs) and
  `row_to_item` in `src/storage/sqlite/mod.rs`. **Not done** (correctly out of scope
  for this merge, still real Stage 4 work): no Smithy fields, no service-layer
  read-only guard, no web UI badge/hidden-Edit — an imported item is fully persistable
  and readable via the domain/storage layer now, but still completely unreachable from
  any HTTP request, exactly as intended until Stage 4/5 land.
- `src/storage/sqlite/items.rs`'s `list_by_calendar_subscription_scopes_to_that_subscription`
  test (added in Stage 2 as a raw-SQL `UPDATE` workaround, since the field didn't exist
  yet) was simplified to set the now-real domain fields directly through `repo.create`.
  Added a new `google_event_id_and_calendar_subscription_id_round_trip_through_create_and_update`
  test mirroring `series_id_round_trips_through_create_and_update`'s exact shape.
- `ParsedIcalEvent`/`CalendarSyncError`/`SyncSummary`/`parse_ical`/`fetch_ical`/
  `sync_subscription` all match the plan's sketch exactly, name-for-name. One addition
  not in the sketch: `CalendarSyncError` gained a second variant, `Repo(String)` — the
  plan's sketch only anticipated fetch failures (`Fetch(String)`, "non-2xx, timeout,
  etc."), but `sync_subscription` also has to propagate a storage-layer failure
  (`list_by_calendar_subscription`/`create`/`update_by_project`/`delete`/
  `record_sync_result` are all fallible). `RepoError` has no `Display`/`Error` impl, so
  `repo_err()` formats it via `{:?}` into `CalendarSyncError::Repo`. Both variants get
  `record_sync_result(..., Some(error.to_string()))` on failure via a shared
  `Display`/`Error` impl on `CalendarSyncError` itself.
- **Diff logic split into `sync_subscription` (fetch + bookkeeping) and a private
  `run_diff` (the actual create/update/delete diff)**, not in the plan's sketch but a
  natural factoring once `record_sync_result`'s success/failure bookkeeping needed to
  wrap the diff step without duplicating that bookkeeping for every early-return. `run_diff` takes
  `importable: &[ParsedIcalEvent]` (already RRULE-partitioned and window-filtered by its
  caller) and returns a plain `(created, updated, deleted)` tuple; `sync_subscription`
  computes `skipped_recurring` itself and assembles the final `SyncSummary`.
- **Window filtering lives in `sync_subscription`, not `parse_ical`** — a deliberate
  read of the plan's diff-logic bullet list (which only mentions RRULE-partitioning as
  already done to `parsed` before the diff, not window filtering) plus a practical
  concern: keeping `parse_ical` free of any `Utc::now()`-relative behavior makes its own
  unit tests fully deterministic against fixture text with fixed dates, rather than
  needing a controllable-clock parameter threaded through just for testing. A private
  `within_import_window(start, now)` helper (constants `IMPORT_WINDOW_PAST_DAYS = 30`,
  `IMPORT_WINDOW_FUTURE_DAYS = 365`, exactly as specified in the Context section) is
  applied to the RRULE-partitioned non-recurring set before it reaches `run_diff`.
  RRULE-bearing events are *not* window-filtered before being counted into
  `skipped_recurring` — they're never imported regardless of window, so filtering them
  would only make the skipped-count wrong.
- **TZID resolution matches the plan's spec exactly**: `chrono_tz::Tz::from_str(tzid)`
  resolves the param, `Tz::from_local_datetime` localizes the naive value, falls back to
  literal UTC when the value ends in `Z` or has no usable `TZID`. One judgment call the
  plan didn't specify: `LocalResult::Ambiguous` (a local time that maps to two UTC
  instants during a fall-back DST transition) picks the earlier instant — arbitrary but
  harmless, and RFC 5545 doesn't disambiguate this case either, noted in a code comment
  so it isn't mistaken for an oversight later.
- **All-day `DTEND` default**: the plan's "end computed as start + 1 day if DTEND gives
  an exclusive end date" was read as "when DTEND is absent for an all-day event, default
  it to start + 1 day" (RFC 5545's own default for a DTEND-less all-day VEVENT) rather
  than "always recompute end as start + 1 day" — when DTEND *is* present it's parsed and
  used as-is. Covered by `all_day_event_with_no_dtend_defaults_to_one_day` plus
  `parses_an_all_day_event` (DTEND present) as two separate cases.
- **The `google_event_id`-keyed diff, create/update/delete counts, and
  `STATUS:CANCELLED` filtering** all match the plan exactly. `event_differs_from_item`
  compares `summary`/`description`/`scheduled_date`/`scheduled_end_date`/
  `has_scheduled_time` — a no-op update is skipped, confirmed by
  `no_op_when_nothing_changed`, which sets up a `MockItemRepo` with **no**
  `expect_update_by_project`/`expect_create` at all (a call to either panics the mock,
  which is the assertion).
- **Not abstracted behind a trait**: `fetch_ical` calls `reqwest::get` directly, so
  `sync_subscription`'s own unit tests can't exercise the fetch step without a real HTTP
  call. Per the plan's own verification note ("unit tests against fixture `.ics` text"),
  tests instead call the private `run_diff` directly — documented inline in the first
  diff test (`creates_a_new_event_not_previously_imported`) so this isn't rediscovered
  as a gap later. `fetch_ical`/`sync_subscription`'s fetch-failure path (recording
  `last_sync_error` without touching existing items) is therefore covered by code
  inspection only, not a unit test — worth a live smoke test once Stage 5 makes this
  reachable via a real subscription.
- 11 new tests: 6 pure `parse_ical` fixture tests (plain timed UTC event, all-day event
  with DTEND, all-day event without DTEND, TZID-bearing non-UTC event, CANCELLED event,
  RRULE-bearing event flagged-not-expanded) + 4 `run_diff` tests against `MockItemRepo`
  (create, update, no-op, delete) + 1 `within_import_window` boundary test.
- **Verified**: `cargo test` — 433 passed, 0 failed (421 pre-stage + 11 new
  `calendar_sync` tests + 1 new `items.rs` round-trip test, replacing the raw-SQL Stage 2
  stopgap test 1-for-1 so the net is +1 there). `cargo check` — clean; the only new
  warnings are the expected "never used" ones for this whole module (nothing in Stage 3
  is called from anywhere reachable yet, exactly as planned) plus `CalendarSubscription`
  finally losing its earlier "never constructed" warning now that Stage 3's own tests
  construct one.
- Nothing discovered that changes Stage 4 (the read-only-enforcement/Smithy/web-UI half
  still to do) or Stage 5's assumptions. Stage 5's `create_calendar_subscription`
  handler can call `calendar_sync::sync_subscription(&sub, item_repo.as_ref(),
  calendar_repo.as_ref())` exactly as sketched in that stage's own section — the
  signature landed unchanged from the plan.

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

Done as planned. The domain-struct/storage-row-mapping half was already merged into Stage 3
(see that section's own note), so this stage was Smithy + service-layer guards + web UI only:

- **Smithy**: `googleEventId`/`calendarSubscriptionId` added to `ProjectItem`'s
  `properties`, referenced only in `GetProjectItem`'s `output` and `ProjectItemSummary` —
  exactly as planned, **not** referenced in `CreateProjectItem`'s or `UpdateProjectItem`'s
  `input`, so no client can ever set them via a request body. `task codegen` run;
  `json_api::project_items::get_project_item`/`list_project_items` updated to populate the two
  new output fields from the domain `Item` (`google_event_id`/`calendar_subscription_id`,
  already present on the struct since Stage 3).
- **Service-layer guard**: reused `ItemError::Invalid` rather than adding a new
  `ImportedItemReadOnly` variant (the plan flagged this as the two options — went with the
  `TEMPLATE`-rejection precedent's existing shape, no new variant needed). Placed directly in
  `service::items::update_item`/`delete_item` and `service::team_items::update_team_item`/
  `delete_team_item` — the functions that actually fetch+mutate — rather than adding a
  redundant second fetch+check in `service::project_items::update_project_item`/
  `delete_project_item`. Those two already delegate straight into the guarded functions and
  propagate their `Result` via `?`, so the rejection surfaces correctly through the
  `project_items` dispatch layer with no changes needed there at all — confirmed by every
  existing `project_items` test still passing unmodified (mock items default
  `google_event_id: None`, so the new guard is a no-op for all of them). Each guard sits right
  after that function's existing `current`/`repo.get(...)`-style fetch, before any other
  validation, mirroring where the `TEMPLATE`-rejection check already sits in `create_item`.
  4 new unit tests (one edit + one delete rejection per module) confirm the guard fires;
  `cargo test` — 437 passed, 0 failed (433 pre-stage + 4).
- **Not needed**: a defensive strip/reject in `project_items::duplicate_project_item` (raised
  while planning this stage, since a naive clone would copy `google_event_id`/
  `calendar_subscription_id` onto the duplicate and collide with the Stage 1 partial unique
  index). Turned out to be moot — `ProjectEventRow::from_item` already hardcodes
  `duplicate_url: None` for every Event (Events never exposed a duplicate action in the UI to
  begin with — only Tasks have a `/tasks/:id/duplicate` route registered in `main.rs`), and
  since imported items are always Events (Context section above), `duplicate_project_item` is
  never actually reachable for one. Left `duplicate_project_item` itself unchanged.
- **Web UI**: added `is_imported: bool` to the shared `web_ui::components::row::Row` (used by
  `ProjectTaskRow`/`ProjectEventRow`/`ProjectSimpleItemRow` — the only three call sites, both
  found and updated; `main_dashboard.rs`/`project_dashboard.rs` have their own unrelated
  `MainDashboardRow`/`ProjectDashboardRow` structs, not `Row`). `ProjectEventRow::from_item`
  sets it from `item.google_event_id.is_some()` and also nulls out `reschedule_url` for an
  imported row (would otherwise open a dialog whose save PUT the new guard rejects);
  `ProjectTaskRow`/`ProjectSimpleItemRow` hardcode `false` (imported items are always Events,
  per the Context section, so this is provably always correct for them, not just a stopgap).
  `templates/components/row.html` shows a small "📅 Google Calendar" badge and hides the
  delete button when `is_imported` — reschedule/duplicate/assign buttons already naturally
  disappear too since their `Option<String>` fields are `None` for an imported row. The
  Events calendar-view day panel (`day_list_rows` in `project_events/mod.rs`) reuses this same
  `ProjectEventRow::from_item`, so it's covered for free — no separate change needed there.
  `ProjectEventDetailPageTemplate` gained the same `is_imported` field (set from
  `item.google_event_id.is_some()` at both its call sites in `handlers.rs`); the detail page
  template hides the "Edit" link and shows the same badge text instead. Did **not** block the
  `GET .../:id/edit` route itself for an imported item (only hid the link to it) — matches this
  app's existing precedent for the analogous `complete == true` case (checked
  `project_tasks/handlers.rs`: no route-level guard exists there either, only the link is
  hidden) — the PUT-side service guard is what actually enforces read-only either way.
  Child add-form under an imported Event (`.../:id/children`) was deliberately left untouched:
  a manually-added child task is never itself imported (`google_event_id` stays `None` on it),
  so it's fully editable/deletable like any other item — only the imported Event row itself is
  locked.
- **CLI/MCP display-only surfacing**: `prl items get` gained a `gcal event: <id or ->` line;
  `prl items list`'s row now appends a `[gcal]` suffix when `google_event_id` is set (mirroring
  the existing `▸`-for-has-children suffix convention). `prl items update`/`create_item` needed
  no change, exactly as the plan predicted (the Smithy input shapes simply don't carry the
  field). MCP: no output-schema change needed — `list_items`/`get_item` return the API's raw
  JSON pass-through, so `googleEventId`/`calendarSubscriptionId` already flow through
  automatically once the server started returning them; only the four tool **descriptions**
  (`list_items`, `get_item`, `update_item`, `delete_item`) were updated to document the
  read-only behavior so a caller doesn't have to discover the rejection by trial and error.
  `npm run build` run to confirm the TS still compiles (it's description-string-only, so this
  was never really in doubt, but confirmed anyway).
- **Verified**: `cargo test` (437 passed), `cargo check`/`task check` (only the same
  pre-existing Stage 2/3 dead-code warnings, no new ones), `task web-styles` (picked up the new
  Tailwind classes in `row.html`/`detail_page.html` with no errors), `cd todo-cli && cargo
  check` (clean), `cd mcp-server && npm run build` (clean). No live/browser smoke test done
  this stage — nothing built here is reachable via HTTP yet (`CreateCalendarSubscription`
  doesn't exist until Stage 5), so there's no way to actually produce an imported item to click
  through; worth a real click-through once Stage 5 lands.
- Nothing discovered that changes Stage 5/6's assumptions. Stage 5's own sync-engine writes
  (`calendar_sync::sync_subscription`, from Stage 3) go straight through
  `item_repo.create`/`update`/`delete` rather than the `service::items`/`team_items` functions
  guarded in this stage — confirmed by re-reading Stage 3's own note on this — so the new
  read-only guards in this stage never block the sync's own writes, only end-user edit/delete
  attempts. Stage 5 can wire `CreateCalendarSubscription`/background sync/web UI management
  screen exactly as sketched with no changes needed to this stage's work.

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

Done as planned, with a few additions beyond the original sketch (a user-requested UI
addition, one genuine pre-existing bug caught by the live smoke test, and one deviation
in how the info dialog is wired):

- **Smithy**: `model/src/main/smithy/calendar_subscription.smithy` (new) — exactly the
  three plain operations sketched (`CreateCalendarSubscription`/
  `ListCalendarSubscriptions`/`DeleteCalendarSubscription`), no `Update`. Followed
  `ItemSeries`'s precedent on the `@notProperty`/`@httpLabel` placement question the plan
  didn't spell out in full: a lone `@httpLabel projectId` never gets `@notProperty`, but a
  *second* `@httpLabel` identifier (`id`, in `DeleteCalendarSubscription`) does — copied
  directly from `GetItemSeries`'s exact shape rather than re-deriving it. Registered in
  `service.smithy`'s top-level `operations: [...]` list, right after the `ItemSeries` ops.
  `task codegen` run clean.
- **Service layer** (`src/service/calendar_subscriptions.rs`, new): `create_calendar_subscription`/
  `list_calendar_subscriptions`/`delete_calendar_subscription` match the plan's gating
  exactly (`require_project_admin` for create/delete, `require_project_member` for list).
  Added one function not in the plan's sketch, `sync_all_subscriptions(calendar_repo,
  item_repo)` — a thin wrapper around the plan's inline background-sweep sketch
  (`list_all` + loop + per-subscription `sync_subscription` + `tracing::warn!` on
  failure), pulled out of `main.rs` and into the service layer purely so it's unit-testable
  and `main.rs`'s `tokio::spawn` body stays a one-liner. `create_calendar_subscription`'s
  immediate post-create sync matches the plan exactly (awaited inline, failure logged but
  never propagated — `sync_subscription` already records the failure on the row via
  `record_sync_result`). `delete_calendar_subscription`'s cascade also matches the plan:
  done in the service function via `list_by_calendar_subscription` + per-item `delete`,
  not pushed into the repo layer. 7 new unit tests (admin/member gating × 3 operations,
  cross-project subscription-id rejection on delete, cascade-deletes-every-imported-item).
  One test-hermeticity note: the "create succeeds even though sync fails" test uses the
  literal string `"not a valid url"` as the iCal URL rather than a syntactically-valid but
  unreachable URL — `reqwest::get` fails at URL-parse time for that, before any real
  network I/O, keeping the test from depending on network access.
- **JSON API** (`src/json_api/calendar_subscriptions.rs`, new): three handlers following
  `item_series.rs`'s exact shape (`server::Extension` params, `to_msg`/`to_summary`
  helpers). Registered in `src/json_api/mod.rs`.
- **`src/main.rs`**: `SqliteCalendarSubscriptionRepo` constructed alongside the other
  repos; `Extension(calendar_repo)` layered onto the smithy `api` service and both
  auth-mode branches' `web_router` (caddy and internal), matching every other repo's
  wiring exactly. Background sync task: a `tokio::spawn`ed `loop { sleep(15 min); sync_all_subscriptions(...).await }`,
  sleep-first as the plan specified (`create_calendar_subscription`'s own inline sync
  already covers "just added"). This is genuinely the first `tokio::spawn`-based
  background loop in this codebase — flagged with a comment at the spawn site, per the
  plan's own note. Three new web routes: `GET/POST /projects/:project_id/calendar-subscriptions`,
  `DELETE /projects/:project_id/calendar-subscriptions/:subscription_id`, and one not in
  the original plan — `GET /calendar-subscriptions/ical-help` (see the info-dialog note
  below).
- **Web UI** (`src/web_ui/project_calendar_subscriptions.rs`, new;
  `templates/project_calendar_subscriptions/*.html`, new): single-file module following
  `project_activity.rs`'s precedent (small screen, no dedicated subdirectory of
  handlers.rs/templates.rs) rather than the larger per-type screens' folder structure.
  `project_calendar_subscriptions_page`/`create_project_calendar_subscription_form`/
  `delete_project_calendar_subscription_form` share one `render_page` helper, matching
  `project_activity.rs`'s `render_activity_page` reuse pattern. List is member-readable
  (`list_calendar_subscriptions` only requires membership); the add form and each row's
  delete button are conditionally rendered on `is_admin` (computed via
  `is_project_admin`, matching the plan). Delete confirms via `hx-confirm` with the
  cascade warning spelled out, exactly as specified. "Manage Google Calendars" link added
  to `project_events/list_page.html`'s header, admin-only (`ProjectEventsListPageTemplate`
  gained an `is_admin` field, computed in `project_events_page`).
- **User-requested addition, mid-stage**: an info icon (ⓘ) next to the iCal URL field
  opening a small dialog with step-by-step instructions for finding a calendar's "Secret
  address in iCal format" in Google Calendar's own UI — not in the original plan, added
  after the user asked for it while this stage was in progress. **Deviation from the
  first draft**: the first pass embedded a self-contained `<dialog>` directly in
  `page.html`, opened via a plain `onclick="...showModal()"`. That collided with
  `base.html`'s existing `htmx:afterSwap` listener (added for `#action-dialog`/
  `#error-dialog`), which auto-opens *any* `<dialog>` element found inside a swapped
  target — since this screen's own page content is swapped into `#page` on every boosted
  navigation, the dialog was popping open on every page load, not just on click (caught
  live, not by any unit test — Askama/`cargo test` have no way to exercise client-side
  JS). Fixed by following the established `#action-dialog` convention instead
  (`templates/components/reschedule_dialog.html`'s exact shape): the instructions moved to
  their own fragment template (`templates/project_calendar_subscriptions/ical_help_dialog.html`,
  no `<dialog>` wrapper of its own) served by a new static handler
  (`ical_help_dialog_fragment`, `GET /web/calendar-subscriptions/ical-help`, no project
  scoping needed since the content never varies), and the info button now does
  `hx-get="..." hx-target="#action-dialog" hx-select="unset" hx-swap="innerHTML"` like
  every other row-action dialog trigger in this app. Confirmed live: closed on normal page
  load/navigation, opens only on click, closes via its own "Got it" button.
- **Pre-existing bug caught by the live smoke test, fixed as part of this stage**:
  `ItemRepo::list_due`/`list_due_by_project` (`src/storage/sqlite/items.rs`) build their
  own explicit `SELECT` column lists rather than reusing the shared `ITEM_SELECT`
  constant every other query site uses. Stage 3's merged-in domain-field addition (see
  that stage's own implementation notes) added `google_event_id`/`calendar_subscription_id`
  to `ITEM_SELECT` and every site built on it, but missed these two — `row_to_item`
  unconditionally does `row.get("google_event_id")`, which panics with `ColumnNotFound`
  the instant either function runs against a database with at least one due item. No
  prior test exercised either function against a real row (`list_due`/`list_due_by_project`
  had zero dedicated unit tests before this), so it went unnoticed through Stages 3/4's
  own `cargo test` runs, which only ever ran against tiny hand-built fixtures with no due
  items in the relevant paths — it surfaced immediately on this stage's live smoke test
  (see below), which ran the real server against a full copy of the project's actual
  production data and hit the dashboard route, which calls `list_due_by_project`. Fixed
  by adding `items.google_event_id, items.calendar_subscription_id` to both queries'
  column lists. Added 2 regression tests (`list_due_by_project_does_not_panic_and_round_trips_google_event_id`,
  `list_due_does_not_panic_and_round_trips_google_event_id`) that create a due item with
  `google_event_id` set and assert both functions return it correctly rather than
  panicking — the gap this bug slipped through.
- **Live smoke test** (beyond the usual `cargo test`/`task check`): ran the actual server
  (`caddy` auth mode, `TODO_DEV_EMAIL` bypass) against a scratch copy of the project's
  real production database (never the real file — copied to a session-local scratch
  path, deleted afterward) and drove it with Playwright. Confirmed, as the admin
  (`whlapinel@gmail.com`, an actual admin on the real "Lapinel Family" project): the
  "Manage Google Calendars" link appears on the Events header; the calendar-subscriptions
  page loads; the info dialog opens on click and stays closed otherwise (see above);
  subscribing with a syntactically-valid but nonexistent iCal URL succeeds (the row
  appears immediately with a real "last sync failed: failed to fetch calendar feed:
  unexpected status 404 Not Found" — confirming `fetch_ical`'s real-network path, never
  actually unit-tested per Stage 3's own notes, works correctly end-to-end); unsubscribing
  prompts the cascade-warning `hx-confirm` dialog and, once accepted, removes the row.
  Also restarted the server as a different real project member (a `member`-role, not
  `admin`-role, user on the same project) and confirmed the "Manage Google Calendars" link
  is absent from the Events header and the add-form is absent from the
  calendar-subscriptions page when visited directly (list itself still renders, empty, per
  the plan's member-readable design). This also incidentally confirmed migration 25 applies
  cleanly to the real production schema (which had never run past migration 24 before this
  stage), not just the test-fixture schema.
- **Verified**: `cargo test` — 446 passed, 0 failed (437 pre-stage + 7 new
  `calendar_subscriptions` service tests + 2 new `list_due`/`list_due_by_project`
  regression tests — see the bug note above). `task check`/`cargo check` — clean, same 13
  pre-existing warnings as every prior stage, none new. `task web-styles` — picked up the
  new Tailwind classes with no errors. Live smoke test as described above.
- Nothing discovered that changes Stage 6's assumptions. `prl`/MCP parity (Stage 6) can
  wire straight onto `CreateCalendarSubscription`/`ListCalendarSubscriptions`/
  `DeleteCalendarSubscription` exactly as generated — no Smithy shape changed since this
  stage landed them.

---

## Stage 6 — CLI + MCP parity

Per this repo's own touch-point checklist convention (new operations must reach `prl` and the MCP server too):

- `prl projects calendar add --project <id> --url <ical-url>` / `prl projects calendar list --project <id>` / `prl projects calendar remove --project <id> <subscription-id>` (`todo-cli/src/projects.rs`, mirroring `prl projects members`/`set-role`'s existing shape).
- MCP tools `create_calendar_subscription`/`list_calendar_subscriptions`/`delete_calendar_subscription` (`mcp-server/src/index.ts`), `projectId`-required like the other project-scoped tools.
- `docs/prl-user-guide.md` documents the new subcommands.

**Files touched:** `todo-cli/src/projects.rs`, `todo-cli/src/main.rs` (subcommand wiring), `mcp-server/src/index.ts`, `docs/prl-user-guide.md`.

### Implementation notes

Done as planned, with one naming deviation from the plan's literal CLI syntax sketch:

- **CLI naming deviation**: the plan's own prose sketch (`prl projects calendar add
  --project <id> --url <ical-url>`) implied a nested `calendar` sub-subcommand group,
  but no command anywhere in `todo-cli` actually nests two levels deep (`Cli` →
  `Command` → an `X Command` enum is the only nesting pattern in use — confirmed by
  grepping every `#[derive(Subcommand)]`/`#[command(subcommand)]` site before writing
  this). Introducing a second nesting level for three commands would have been a new,
  one-off pattern rather than following precedent, so these landed as three flat
  `ProjectsCommand` variants instead, matching `AttachTeam`/`DetachTeam`/`SetRole`'s
  existing compound-PascalCase-name shape (`CalendarAdd`/`CalendarList`/
  `CalendarRemove`, clap-kebab-cased to `calendar-add`/`calendar-list`/
  `calendar-remove`). Positional args (`project_id` first, matching every other
  `ProjectsCommand` variant) rather than the plan's sketched `--project`/`--url`
  flags, for the same reason — no existing `ProjectsCommand` variant uses flags for
  its own scoping id.
- `todo-cli/src/projects.rs`: `CalendarAdd { project_id, url }` →
  `create_calendar_subscription().project_id(..).ical_url(..).send()`, printing the
  new subscription's id. `CalendarList { project_id }` →
  `list_calendar_subscriptions()`, printed as a table (id / last-synced date via the
  existing `fmt_date_opt` helper / last error or `-` / url) — reused `fmt_date_opt`
  rather than adding a new formatter, since `CalendarSubscriptionSummary::last_synced_at()`
  is already the same `Option<&aws_smithy_types::DateTime>` shape every other
  `fmt_date_opt` call site takes. `CalendarRemove { project_id, subscription_id }` →
  `delete_calendar_subscription().project_id(..).id(..).send()`.
- **MCP** (`mcp-server/src/index.ts`): `create_calendar_subscription`/
  `list_calendar_subscriptions`/`delete_calendar_subscription` tools added right after
  `detach_team_from_project` in both the tool-definition list and the switch statement,
  `projectId`-required on all three (matching every other project-scoped tool) plus
  `icalUrl`/`subscriptionId` required on create/delete respectively. Route directly onto
  the Stage 5 REST paths (`POST`/`GET`/`DELETE /projects/:projectId/calendar-subscriptions[/:id]`)
  via the existing `api()` helper, no new plumbing needed. Descriptions call out the
  read-only-import and ~15-minute background-resync behavior so a caller doesn't have to
  discover either by trial and error.
- `docs/prl-user-guide.md`: new "Google Calendar subscriptions" subsection under
  "## Projects", documenting `calendar-add`/`calendar-list`/`calendar-remove` and the
  read-only/cascade-delete behavior, matching the section's existing prose style.
- **Verified**: `cargo test` (root crate) — 446 passed, 0 failed, unchanged from Stage
  5 (this stage touched no Rust service/storage code, only the CLI crate and the
  Node/TS MCP server). `cargo check` (root crate) — clean, same 13 pre-existing
  warnings as every prior stage. `cd todo-cli && cargo check` — clean. `cd mcp-server
  && npm run build` — clean. `cargo run --bin prl -- projects --help` confirmed the
  three new subcommands appear with their descriptions.
- Nothing discovered that changes Stage 7's assumptions — Stage 7 only touches
  `src/service/calendar_sync.rs`, untouched by this stage.

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

Done as planned, with the API details confirmed against this repo's own already-vendored
`rrule = "0.14.0"` (already a dependency, already used by `src/domain/recurrence.rs` for
the English-phrase recurrence parser's own expansion — no new crate added, contrary to
the plan's expectation that this would need a fresh dependency) rather than assumed from
memory or docs.rs summaries — the crate's real source was read directly from
`~/.cargo/registry/src/.../rrule-0.14.0/` to confirm `RRuleSet`'s exact builder shape
(`RRuleSet::new(dt_start).rrule(validated).exdate(...).after(...).before(...).all(limit)`)
and `rrule::Tz`'s `From<chrono_tz::Tz>` impl, before writing any expansion code:

- **`ParsedIcalEvent` gained five fields**, exactly as sketched: `rrule: Option<String>`
  (raw `RRULE` value), `exdates: Vec<DateTime<Utc>>`, `tz: chrono_tz::Tz` (the zone
  `DTSTART` resolved in — needed so expansion respects local wall-clock time across DST,
  not a fixed UTC offset), `recurrence_id: Option<DateTime<Utc>>` (`Some` only on a
  `RECURRENCE-ID`-bearing override VEVENT, holding that occurrence's *original* unmodified
  timestamp), and `cancelled: bool` (`true` only for a cancelled *override* — a plain
  cancelled non-override event is still dropped entirely at parse time, unchanged from
  Stage 3).
- **`parse_dt_property` refactored into a shared `parse_dt_value(value, params)`** so the
  same TZID-aware parsing logic (unchanged from Stage 3) is reusable for `DTSTART`/`DTEND`/
  `RECURRENCE-ID` (single value) and `EXDATE` (a property whose value can be a
  comma-separated list of instants sharing one set of params — RFC 5545 permits this
  and real feeds use it). Return type widened from `(DateTime<Utc>, bool)` to
  `(DateTime<Utc>, bool, chrono_tz::Tz)`, the third element being the resolved zone
  (`Tz::UTC` for an all-day date, a `Z`-suffixed value, or a `TZID`-less/unresolvable
  local value — matching Stage 3's existing UTC-fallback precedent exactly).
- **`parse_vevent`'s cancellation check narrowed**: `STATUS:CANCELLED` still drops a
  plain/master event entirely (`recurrence_id.is_none()`, unchanged behavior — confirmed
  by the pre-existing `skips_a_cancelled_event` test still passing unmodified), but a
  cancelled *override* (`RECURRENCE-ID` present) is now parsed fully with `cancelled: true`
  rather than filtered out — `sync_subscription` needs that data to know which slot to
  drop, the same way an `EXDATE` does.
- **`expand_master_event(master, overrides, now) -> Result<Vec<ParsedIcalEvent>, String>`**
  (new, `src/service/calendar_sync.rs`): builds an `RRuleSet` from `master.rrule` parsed
  via `rrule::RRule<Unvalidated>::from_str` + `.build(dt_start)` (confirmed this one-step
  `build` — validate-and-construct-the-set together — exists on `RRule<Unvalidated>`
  directly, simpler than the plan's sketched two-step `validate` + `RRuleSet::new(...).rrule(...)`),
  adds every `master.exdates` entry via `.exdate(...)`, then queries
  `.after(window_start).before(window_end).all(RECURRING_MAX_OCCURRENCES)` where the
  window is `now - 7d` to `now + 180d` (`RECURRING_WINDOW_PAST_DAYS`/
  `RECURRING_WINDOW_FUTURE_DAYS`, exactly the constants the Context section specified).
  Each returned occurrence's *original* timestamp becomes
  `"{uid}::{original_start.to_rfc3339()}"`; if an override matches that original timestamp
  in `overrides` its fields replace the generated ones (same synthetic id) unless
  `ov.cancelled`, in which case the slot is dropped (`filter_map` returning `None`) —
  exactly the plan's override/cancellation semantics. `RECURRING_MAX_OCCURRENCES = 500` is
  a generous safety cap (not expected to bind — the ~187-day window holds ~187 occurrences
  even at daily frequency) with a `tracing::warn!` if `RRuleResult::limited` ever fires.
  `Err(String)` only for an unparseable/invalid `RRULE`; the caller counts this into
  `SyncSummary::skipped_recurring` (redefined from Stage 3's "every recurring event,
  unconditionally" to "recurring events whose RRULE failed to expand") rather than
  failing the whole sync.
- **`sync_subscription` partitions `parse_ical`'s flat output three ways** — a
  `recurrence_id.is_some()` VEVENT into an `overrides: HashMap<(uid, recurrence_id),
  ParsedIcalEvent>`, an `has_rrule` VEVENT into `masters`, everything else into
  `non_recurring` — then calls `expand_master_event` per master and appends every
  expansion's output straight into `importable` alongside the (still separately
  window-filtered) non-recurring events. **No changes to `run_diff` at all** — the plan's
  central claim that expanded occurrences' deterministic synthetic ids let the existing
  `google_event_id`-keyed diff handle create/update/delete unchanged held exactly as
  predicted, confirmed by `run_diff_creates_items_for_each_expanded_recurring_occurrence`
  (a `FREQ=WEEKLY;COUNT=3` master's 3 expanded occurrences flow through `run_diff`
  unmodified and produce 3 plain creates).
- **Window sliding needed no special-casing**, per the plan's own prediction: an aged-out
  occurrence simply isn't regenerated by the next sync's `expand_master_event` call, so it
  falls out of `importable` and is deleted via the ordinary diff-delete path — nothing
  added to `run_diff` to recognize this as "expected churn" versus a real deletion, since
  from the diff's perspective they're identical and that's fine (a code comment on the two
  window constants notes this instead of touching the diff logic).
- **9 new tests**, all against the pure `expand_master_event`/`parse_ical` functions (no
  network, following Stage 3's own precedent of not abstracting `fetch_ical` behind a
  trait): weekly expansion within the window (asserts count, exact 7-day spacing, synthetic
  id format, and every occurrence falling inside `[now-7d, now+180d]` rather than a
  brittle magic-number count), `EXDATE` exclusion, a `RECURRENCE-ID` override (moved
  time + renamed summary, matched and re-keyed under the *original* timestamp), a
  cancelled-override slot drop, an unparseable-`RRULE` error case, plus 4 pure `parse_ical`
  fixture tests (`RRULE` value capture, multi-value comma-separated `EXDATE` + a
  `RECURRENCE-ID` override VEVENT parsed together from one feed, and a cancelled override
  surviving parse with `cancelled: true`) — and the one `run_diff` integration test above.
- **The DST correctness case the plan flagged as the highest-risk part of this whole
  stage got its own dedicated test**
  (`expand_master_event_respects_local_wall_clock_across_a_dst_transition`): a weekly 9am
  `America/New_York` master spanning the 2026-11-01 fall-back transition is asserted to
  stay pinned to 9am *local* time on both sides (13:00 UTC before the transition during
  EDT, 14:00 UTC after during EST) rather than drifting to a fixed UTC offset — this is
  exactly the bug class `family-board`'s own non-recurring-only `parse_ical_dt` never had
  to face, and confirms `rrule::Tz`'s `chrono::TimeZone` impl normalizes correctly across
  the transition without any extra handling needed in this module's own code.
- **Verified**: `cargo test` — 455 passed, 0 failed (446 pre-stage + 9 new). `task check`/
  `cargo check` — clean, the same 13 pre-existing warnings as every prior stage, none new.
  No live smoke test against a real Google Calendar recurring event was done this stage
  (the plan's own verification note suggested one) — flagging this as a worthwhile
  follow-up before fully trusting this against production feeds, since fixture-`.ics`
  coverage, however thorough, can't rule out a real calendar exporting an `RRULE`/
  `EXDATE`/`RECURRENCE-ID` shape these tests didn't anticipate.
- Nothing else discovered that changes any downstream assumption — this was the last
  planned stage; the whole `docs/google-calendar-import-plan.md` feature is now
  code-complete pending that live smoke test.
