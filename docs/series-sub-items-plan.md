# Task series sub-items (replacing `template_item_id`)

Status: **Stages 1–2 complete** (2026-08-26). Stages 3–4 not started. Written
2026-08-26 after a design discussion in this repo. Supersedes Stage 10 gap 3 of
`docs/archived/recurring-events-virtual-occurrences-rough-plan.md`
(the `template_item_id`/`copy_template_children`-at-materialize-time mechanism) before that
mechanism ever shipped to real usage — confirmed with the user that nothing depends on it,
so this plan removes it outright rather than migrating existing data.

## Stage 1 completion notes (2026-08-26) — read this before starting Stage 2

Stage 1 landed with a few deliberate deviations from the "Design"/"Staging" sections below,
made mid-implementation once the actual blast radius became clear. **The Design/Staging
sections below are the original pre-implementation plan and are now partly stale** — this
section is the authoritative record of what's actually true on disk; treat the rest of the
doc as background/rationale, not as a description of current state.

1. **`recurrence`/`anchor_date` were deliberately *not* made `Option` in Stage 1**, contrary
   to the Domain/Smithy sections below. `ItemSeries.recurrence: String` and
   `anchor_date: DateTime<Utc>` are still required, non-`Option`, exactly as before — as are
   the corresponding Smithy fields (`@required` untouched on all four operations) and the DB
   columns (still `NOT NULL`, no migration rebuild). Reasoning found mid-implementation: making
   these `Option` ripples into ~40+ call sites across `src/service/item_series.rs` alone
   (every `recurrence::parse(&series.recurrence)`/`series.anchor_date` read, in both
   production code and its ~150 unit tests), and nothing actually *needs* `None` there until a
   child series can exist — which Stage 2 is what introduces. Bundling the `Option`-ification
   with Stage 2's `validate_series_parent`/`validate_series_recurrence_required`/
   `resolve_current_occurrence_date` work (where `None` first becomes meaningful) keeps each
   stage's diff coherent instead of a schema stage doing a behavior-adjacent, high-churn change
   for a case that can't occur yet.
   - **This means Stage 2 must do the `recurrence`/`anchor_date` Option-ification work** that
     the Design section below describes (domain struct, sqlite repo, Smithy `@required` drop +
     `task codegen`, `json_api/item_series.rs`, and — the wide part — every
     `recurrence::parse(&series.recurrence)`/`.anchor_date` read site in
     `src/service/item_series.rs`, both production and tests) **as part of Stage 2**, not
     something already done. Grep fresh at that time; the plan's original list of call sites is
     illustrative, not exhaustive, and will have drifted further since 2026-08-26.
   - The nullability migration (relaxing `recurrence`/`anchor_date` to nullable via the
     rebuild-and-copy shape `ActivityLogTeamIdNullable` established) is **not yet written** —
     do it as part of Stage 2, most likely as its own migration (version 30+) rather than
     folded into `AddItemSeriesChildren` (version 29, already shipped and immutable now that
     it's applied — see migration-system conventions in CLAUDE.md: migrations are append-only,
     never edited after the fact once merged).
2. **The `template_item_id`-linked-template copy-on-materialize behavior was removed in Stage
   1, not deferred to Stage 2 as the Design section's `get_or_materialize_occurrence` bullet
   originally described.** Once `ItemSeries.template_item_id` no longer exists as a field
   (confirmed clean removal, no back-compat shim), the `if let Some(template_id) =
   &series.template_item_id { copy_template_children(...) }` block in
   `get_or_materialize_occurrence` (`src/service/item_series.rs`) could not compile, so it was
   deleted immediately along with the field. **Practical effect**: any series that already had
   `template_item_id` set before this commit stops copying template children onto newly
   materialized occurrences as of this commit, not as of some later stage — there was no way to
   defer this given the "clean removal" instruction. Confirmed acceptable since nothing in
   current data relies on it.
3. **Stage 1 additionally touched `src/web_ui/project_item_series/` (handlers.rs, templates.rs)
   and three `templates/project_item_series/*.html` files** — outside the Design section's
   "Web UI" scope, which the Staging section assigns entirely to Stage 3. This was
   unavoidable: `src/web_ui/` is compiled as part of the same crate as everything else (`cargo
   build`/`cargo test` at repo root builds the whole crate in one pass — see CLAUDE.md's
   Architecture section), so removing `ItemSeries.template_item_id` forced removing every
   reference to it, including the "Children template" `<select>` and its Rust plumbing
   (`CreateItemSeriesForm.template_item_id`, `NewProjectItemSeriesPageTemplate.templates`,
   `EditProjectItemSeriesPageTemplate.templates`/`template_item_id`,
   `ProjectItemSeriesRow.template_name`). **No replacement UI was added** — the series
   create/edit forms currently have no field for `templateItemId` (removed) and no field for
   `parentSeriesId`/`dueOffsetDays` either; both `CreateItemSeriesParams`/
   `UpdateItemSeriesParams` are constructed with `parent_series_id: None, due_offset_days:
   None` at both web UI call sites. That real UI work (parent-series `<select>`, offset input,
   indented sub-item list rows) is still entirely Stage 3's to do, per the Design section
   below.
   - **Known regression from this, until Stage 3 lands**: `update_project_item_series_form`
     unconditionally writes `parent_series_id: None, due_offset_days: None` on every edit
     (matching this codebase's existing round-trip convention — see the `basis`/
     `assigned_to_user_id` comments in that same struct literal). Since nothing can set these
     two fields to non-`None` via the web UI yet, this is currently a no-op in practice, not an
     active bug — but it means editing a series that got `parentSeriesId`/`dueOffsetDays` set
     via the JSON API/CLI directly would silently clear them back to `None` if edited through
     the web UI before Stage 3 ships the real form fields. Worth a mental note, not a blocker.
4. **`create_series`/`update_series` lost their `repo: &Arc<dyn ItemRepo>` parameter
   entirely** (not mentioned in the Design section, which only sketched adding/removing
   validation calls). It was solely used by the now-deleted `validate_series_template_item`;
   once that function was deleted, `repo` was dead in both functions and in every caller
   (`json_api/item_series.rs`, `web_ui/project_item_series/handlers.rs`), so it was removed
   for real rather than left as an unused/underscore-prefixed parameter. **Stage 2 will need to
   re-add an `ItemSeriesRepo`-typed parameter (not `ItemRepo`) to at least one of these** for
   `validate_series_parent` (needs to look up the candidate parent series by id) — check the
   current signature before assuming `repo`'s absence is permanent scaffolding to restore
   as-is.
5. Five now-meaningless unit tests were deleted outright rather than adapted (they tested the
   `template_item_id` validation/materialization behavior that no longer exists):
   `create_series_rejects_template_item_id_on_an_event_series`,
   `create_series_rejects_a_non_template_item_as_the_template`,
   `create_series_rejects_a_template_item_from_another_project`,
   `create_series_accepts_a_valid_template_item_id`,
   `update_series_accepts_a_valid_template_item_id`, and
   `materializes_a_task_and_copies_the_linked_templates_children`. Their test-only helpers
   (`no_template_repo`, `template_item`) were deleted too since nothing else used them. Stage 2
   should add fresh tests for `validate_series_parent`/`validate_series_recurrence_required`/
   `validate_series_offset` instead of trying to resurrect these.
6. Verification performed: `cargo build`, `cargo test` (497 passed, 0 failed), `cargo fmt`
   (plain, repo-root, no path args). `task codegen` was run once, successfully, after editing
   `item_series.smithy`. **Not verified**: live-in-browser behavior (per CLAUDE.md's
   no-Playwright convention) — the create/edit series forms were only checked by reading the
   template/handler diff, not by loading them in a browser. `todo-cli`/`mcp-server` were **not
   touched** and are now stale against the Smithy model (`todo-cli/src/series.rs` still
   references `--template`/`template_item_id()` against the regenerated `todo-client`, which no
   longer has that field — `cd todo-cli && cargo build` will fail until Stage 4). This is
   expected per the Staging section's original stage assignment (CLI/MCP is Stage 4), not a
   regression introduced early.

## Stage 2 completion notes (2026-08-26) — read this before starting Stage 3

Stage 2 did the deferred `recurrence`/`anchor_date` Option-ification (domain, sqlite repo,
Smithy `@required` drop + `task codegen`, `json_api/item_series.rs`, and every
`recurrence::parse`/`.anchor_date` call site) that Stage 1's completion notes pushed here,
plus the full service-layer design below. A few deviations from the pre-implementation
Design section, found mid-implementation:

1. **Migration is version 30**, `src/storage/migrations/item_series_children_relax_recurrence.rs`
   (`ItemSeriesChildrenRelaxRecurrence`) — a separate migration from `AddItemSeriesChildren`
   (version 29, already shipped), exactly as the Design section recommended. Uses the
   `ActivityLogTeamIdNullable` rebuild-and-copy shape.
2. **`current_occurrence_date` keeps its original `&ItemSeries` signature** (not refactored to
   take a pre-resolved `anchor_date: DateTime<Utc>` parameter) — it reads
   `series.anchor_date.expect(...)` on the `None`-cursor branch, with a doc comment stating the
   invariant. Every caller is now funneled through a new `recurrence_source`/
   `resolve_current_occurrence_date` pair (see below) that resolves to a *root* series first, so
   this `expect` is unreachable in practice; a bare `.unwrap_or_default()` was rejected as
   silently hiding a real invariant violation instead of surfacing it.
3. **New private helper `recurrence_source(series_repo, series)`** — not in the Design section —
   factors out "the series whose recurrence/anchor actually govern cadence: `series` itself for a
   root series, or its parent for a child" (decision 2). `resolve_current_occurrence_date` is
   built on top of it, and it's also reused by `resolve_occurrence_assignee`/
   `current_series_assignee` for a rotating child series' turn-index calculation (not called out
   in the Design section, which only listed cadence/current-occurrence call sites, but rotation
   index math has the exact same "which series' rule do I measure from" question).
4. **Cursor machinery (`advance_cursor`/`retreat_cursor`/`clear_cursor`, and the
   `require_current_occurrence`/`require_cursor_occurrence` ordering guards) now also checks
   `series.parent_series_id.is_none()`, everywhere it already checked
   `series.item_type == ItemKind::Task`** (`skip_occurrence`, `unskip_occurrence`,
   `record_task_completion`, `record_task_uncompletion`, and the two `require_*` guards). This
   wasn't spelled out in the Design section beyond "`cursor_date` is simply never written for a
   child series" — treating a child exactly like an Event-typed series for every cursor-related
   codepath (no ordering enforcement, no cursor writes) was the concrete decision made to realize
   that. Settling a child's own occurrence out of order is therefore unguarded, the same as it
   already is for an Event series today — considered acceptable since nothing shared (a cursor)
   is at risk, and revisiting it only means adding a check, not a schema change.
5. **`is_due_date_basis(series)` was repointed to `series.item_type == ItemKind::Task`** instead
   of being deleted — the Design section's materialization rule ("Task always due_date, Event
   always scheduled_date") made the old `basis: "DUE_DATE"`-reading body dead, but the function
   itself is also called directly from `project_tasks`/`project_events` templates to decide a
   still-*virtual* occurrence's display (💀 vs 📅 icon, overdue styling) — repointing its body
   preserves every one of those call sites' correctness for free, so nothing in `web_ui` needed
   touching for this part.
6. **`validate_completable` gained an `ItemRepo` parameter** (`repo: &Arc<dyn ItemRepo>`, first
   argument) — needed to fetch a materialized child occurrence's `Item.complete` flag for the new
   completion guard (`require_no_incomplete_materialized_child_occurrence`). Its one caller,
   `service::project_items::update_project_item`, already had `repo` in scope.
7. **Rendering's `display_date` field was added to `ProjectOccurrence` and correctly populated
   (service layer), but no `web_ui` template/handler was switched from `occurrence_date` to
   `display_date`** — per the Staging section, that switch is explicitly Stage 3's job
   ("Web UI... switched to `display_date`"), and since no UI can create a child series yet
   (Stage 1/2 note: `parentSeriesId`/`dueOffsetDays` have no form field), `occurrence_date` and
   `display_date` are identical for every series reachable through the UI today — so this is a
   no-op in practice, not a latent bug, until Stage 3 ships the parent-series picker.
8. **`web_ui/project_item_series/` compile fixes were minimal, mirroring Stage 1's precedent**:
   the create/update handlers now send `recurrence: Some(...)`/`anchor_date: Some(...)` (always
   creating a root series, since there's still no parent-series `<select>`), and the
   list/edit-page display code falls back to `unwrap_or_default()`/`Utc::now()` for the
   (currently unreachable via the UI) case of a child series' `None` recurrence/anchor_date
   reaching a display path built for a root series. Real child-aware UI is still entirely
   Stage 3's work.
9. Test coverage added for the new pieces: `validate_series_parent` (parent must be TASK, same
   project, itself root, no self-reference), `validate_series_recurrence_required` (root requires
   both fields, child rejects either), `validate_series_offset` (offset only valid on a child),
   the `basis: DUE_DATE` outright rejection (replacing the old
   `create_series_allows_due_date_basis_on_any_recurrence_pattern_for_a_task_series` test, which
   asserted the now-retired opposite behavior), the completion guard (blocks on a materialized+
   incomplete child occurrence, allows through a still-virtual one), the delete cascade (child
   series deleted before parent), and the rendering second pass (one synthesized child
   `ProjectOccurrence` per parent cycle, with `occurrence_date` as the lookup identity and
   `display_date` offset-shifted). Not exhaustive — e.g. `duplicate_series`'s child-duplication
   path and the rotating-child-assignee path have no dedicated new test, verified by code reading
   only.
10. Verification performed: `task codegen` (twice — first pass caught that only
    `ItemSeriesSummary`'s `@required` had been dropped, not the three operation blocks', fixed
    and re-run), `cargo build`, `cargo test` (513 passed, 0 failed, up from 497), `cargo fmt`
    (plain, repo-root, no path args). **Not verified**: live-in-browser behavior (per CLAUDE.md's
    no-Playwright convention). `todo-cli` was already broken since Stage 1
    (`template_item_id`/`--template`); this stage adds new, distinct breakage from the same root
    cause category — `recurrence`/`anchorDate` becoming optional on the generated client types
    (`Option<&str>` no longer implements `Display` where `todo-cli` prints them directly) — on
    top of the still-unresolved `template_item_id` breakage. Both are Stage 4's to fix in one
    pass; `mcp-server` (TypeScript, separate build) was not touched or built, per the Staging
    section.

## Context

`docs/issues_and_features.md`'s first entry raised a real problem with `ItemSeries.template_item_id`:
a template's children only get copied onto an occurrence at *materialization* time (typically
when the occurrence is completed, near its due date), so a lead-time child with a large negative
`due_offset_days` (e.g. "order supplies", offset -30) never shows up in the calendar until far
too late to be useful — the whole point of a lead-time reminder is to see it in advance.

Two fixes were considered and rejected in favor of a third:

- Eagerly materializing all *current* occurrences on read — rejected because a long lead time
  (offset -60) could materialize an occurrence, and therefore its children, well before the
  parent's own due date, producing children (and a parent!) that read as already-overdue at
  birth.
- Eagerly materializing the *next* occurrence as soon as the current one settles — a band-aid
  that only shifts the same problem earlier by one cycle, and adds a special case gated on
  "does this series have a template" rather than fixing the underlying model.
- **Letting a task series have sub-items directly, each one itself a series** (this plan) — reuses
  the virtual/materialize/skip/complete machinery `docs/archived/unify-virtual-materialized-occurrences-plan.md`
  already built for top-level occurrences, recursively, instead of inventing a new
  "virtual child of an occurrence" concept. A lead-time child is a first-class `ItemSeries` occurrence
  with its own lifecycle, so it's virtual (and thus calendar-visible) exactly as far in advance as
  any other series occurrence already is, and can be completed independently and early without
  forcing the parent to materialize.

### Decisions confirmed with the user (2026-08-26)

1. A series (`ItemSeries`) gains a `parent_series_id`, mirroring `Item.parent_item_id`. Only a
   `Task`-typed series may have `parent_series_id` set (be a child) — `Event`-typed series can
   never be children, matching `Event` items never being auto-trigger targets either.
2. A child series has **no independent recurrence of its own** — it has no `recurrence`/
   `anchor_date`, and no `cursor_date`. Its "current occurrence" is always exactly its parent's
   current occurrence (`item_series::current_occurrence_date` on the parent) — one cycle, shared
   identity, not two cursors that could drift apart.
3. **One level of nesting only** — a child series cannot itself have `parent_series_id` set on
   one of *its* children. Not enforced by the schema shape (it's a plain nullable self-reference,
   same as `Item.parent_item_id`), but rejected at the service layer the same way `Item`'s
   "children cannot themselves have children" isn't actually a rule today for items, but *is*
   being deliberately introduced for series, per "no premature abstraction until something needs
   it."
4. `template_item_id` is removed outright — **no migration/backfill**, since the user confirmed
   nothing in current data relies on it.
5. (Raised independently in the same conversation, applies to *every* Task series, not just ones
   with children) **Task series lose the "materialize onto `scheduled_date`" option entirely** —
   a Task series always materializes onto `due_date` going forward. This retires the `basis:
   "DUE_DATE"` toggle (Stage 10 gap 1's `is_due_date_basis`) by making its behavior the only
   behavior for `Task`-typed series; `Event`-typed series are unaffected and keep materializing
   onto `scheduled_date` as today. **This is a behavior change for any existing Task series that
   is not already `basis: "DUE_DATE"`** — its future (not historical) materializations switch
   from `scheduled_date` to `due_date`. Flagging this plainly since it's a step further than "new
   series only," even though it follows directly from what was asked ("I want to eliminate the
   concept of schedule in a task series — they should only have due dates").

### Two behavioral questions this plan resolves by inference, not by an explicit prior answer

These follow naturally from "just like with tasks themselves" but weren't asked-and-confirmed
verbatim, so they're called out here rather than buried in the stage text below:

- **Completion guard**: per CLAUDE.md's Completion-transition guards, completing an item is
  rejected while it has an incomplete *materialized* direct child; a virtual, never-materialized
  child doesn't count (an item with zero children can complete freely). This plan applies the
  identical rule one level up: completing a series' current occurrence is rejected if any child
  series' occurrence for that same cycle is materialized-and-incomplete, but a still-virtual
  child occurrence never blocks it. The alternative (forcing every child to materialize before
  the parent can complete) would reintroduce exactly the "forced early materialization" problem
  this plan exists to remove, so this is the only reading that's actually consistent with the
  stated goal.
- **Delete/duplicate cascade**: deleting or duplicating a series with children cascades to the
  children (children are structural — a lead-time sub-item series has no independent meaning
  without its parent), unlike `DeleteItemSeries`'s existing orphan-not-cascade treatment of
  *materialized items*, which stay standalone on purpose (they're independent artifacts with
  their own completion/points history). A child *series* is not a materialized artifact, so the
  precedent that applies here is closer to "deleting a template deletes its children" than
  "deleting a series orphans its materialized items."

## Design

### Schema (migration version 29, `src/storage/migrations/add_item_series_children.rs`)

```sql
ALTER TABLE item_series ADD COLUMN parent_series_id TEXT;      -- guarded, column_exists
ALTER TABLE item_series ADD COLUMN due_offset_days INTEGER;    -- guarded, column_exists
ALTER TABLE item_series DROP COLUMN template_item_id;          -- guarded, inverse column_exists
                                                                -- (SQLite 3.35+ DROP COLUMN,
                                                                -- same precedent as
                                                                -- DropTeamMemberPoints)
```

`recurrence`/`anchor_date` also need to go from `NOT NULL` to nullable (required only for a
series with no parent — see Domain below). SQLite has no `ALTER COLUMN` to drop a `NOT NULL`
constraint, so this needs the rebuild-and-copy shape `ActivityLogTeamIdNullable` already
established: create `item_series_new` with the relaxed schema, `INSERT INTO ... SELECT` every
row across, `DROP TABLE item_series`, `RENAME TO item_series`, then recreate
`idx_item_series_project_id` (indexes don't survive a `DROP TABLE`). Do this in the *same*
migration as the two column adds/one drop above rather than a separate one — it's one
conceptual change to the table's shape.

Update the baseline `CREATE TABLE IF NOT EXISTS item_series` in `create_pool()`
(`src/storage/sqlite/mod.rs`) to match the final shape directly (nullable `recurrence`/
`anchor_date`, `parent_series_id TEXT`, `due_offset_days INTEGER`, no `template_item_id`) — a
fresh DB never touches the migration above.

No new index needed for `parent_series_id` in v1 — per-project series counts are small, and
every "find this series' children" lookup filters an already-fetched `list_series_for_project`
result in memory rather than issuing a new indexed query.

### Domain (`src/domain/item_series.rs`)

- Remove `template_item_id: Option<String>` and its doc comment.
- `recurrence: String` → `recurrence: Option<String>`; `anchor_date: DateTime<Utc>` →
  `anchor_date: Option<DateTime<Utc>>`. `None` for a child series (meaningless field, same
  "meaningless for this kind, not structurally forbidden" precedent `cursor_date` already
  follows for `Event` series).
- Add `parent_series_id: Option<String>` — `Some` only for a `Task`-typed series that is itself
  a child (validated at the service layer, not structurally).
- Add `due_offset_days: Option<i32>` — `Some` only when `parent_series_id.is_some()`, mirroring
  `Item.due_offset_days`'s "only meaningful on a child" convention exactly.
- `cursor_date` is simply never written for a child series (`ItemSeriesRepo::advance_cursor`
  never called on one) — no schema/type change needed, just a service-layer invariant to
  document on the field.
- `basis`'s doc comment: drop the `"DUE_DATE"` paragraph; `Some("COMPLETION")` remains the only
  non-default value, meaning goes back to being one axis (advance timing) instead of two.

This is the widest-blast-radius mechanical change in the plan — `recurrence`/`anchor_date`
becoming `Option` ripples into every direct reader. Known call sites to update (found via
`grep -n "\.recurrence\b\|\.anchor_date\b" src/service/item_series.rs src/storage/sqlite/item_series.rs src/json_api/item_series.rs src/web_ui/project_item_series/*.rs todo-cli/src/series.rs`
before starting — do this grep fresh at implementation time rather than trusting this list,
since it will drift):

- `current_occurrence_date` (takes a pre-parsed `RecurrenceRule`, unaffected directly, but every
  caller that parses `series.recurrence` needs an `.ok_or(...)` first)
- `list_occurrence_states_for_project`'s `recurrence::parse(&series.recurrence)`
- `validate_series_basis`'s re-parse of `recurrence`
- `get_or_materialize_occurrence` (doesn't parse recurrence itself, but constructs `ItemSeries`
  copies in tests)
- `duplicate_series` (plain field clone, unaffected)
- `src/storage/sqlite/item_series.rs`'s row mapping / INSERT / UPDATE column bindings
- `src/json_api/item_series.rs`'s Smithy↔domain conversion (see Smithy section)
- `todo-cli/src/series.rs`'s display/parsing (`fmt_date(out.anchor_date())`, etc.) and
  `mcp-server/src/index.ts`'s response shaping

### Smithy (`model/src/main/smithy/item_series.smithy`)

In `ItemSeriesSummary`, `CreateItemSeries`'s input, `GetItemSeries`'s output, and
`UpdateItemSeries`'s input (all four currently repeat the same field list — keep them in sync
as today):

- Remove `templateItemId: String`.
- Add `parentSeriesId: String` (optional, `@notProperty`) and `dueOffsetDays: Integer`
  (optional, `@notProperty`).
- Drop `@required` from `recurrence`/`anchorDate` on `CreateItemSeries`'s input and
  `UpdateItemSeries`'s input (they stay present on `GetItemSeries`'s output and
  `ItemSeriesSummary` as plain optional fields too, so a child series' `GetItemSeries` simply
  omits them on the wire). Required-only-for-a-root-series becomes a service-layer check
  (`validate_series_recurrence_required`, see below) — this is exactly the precedent
  `event_type`'s cross-field validity already follows (CLAUDE.md: "every other finite-valued
  field... is compared by literal at the Rust layer, with no Smithy-level validation";
  `ItemType` is the one deliberate exception, and this isn't it).
- `itemType` stays `@required` on all four — unaffected.

Run `task codegen` after editing, then fix the resulting Rust compile errors in
`src/json_api/item_series.rs` (the Smithy↔domain conversion) and `src/main.rs` if the handler
signatures shift.

### Service layer (`src/service/item_series.rs`)

**Validation** — replace `validate_series_template_item` with two new checks, called from both
`create_series` and `update_series` exactly where `validate_series_template_item` is called
today:

```rust
/// Only a TASK series may be a child (decision 1); a child's parent must exist, live in the
/// same project, itself be a TASK series, and itself have no parent (decision 3 — one level
/// only). Self-reference (only reachable via update_series) is rejected too.
async fn validate_series_parent(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    own_id: Option<&str>, // None on create
    project_id: &str,
    item_type: ItemKind,
    parent_series_id: &Option<String>,
) -> Result<(), ItemError> { ... }

/// A root series (no parent) must define its own cadence; a child series must not — its cycle
/// is entirely inherited (decision 2).
fn validate_series_recurrence_required(
    parent_series_id: &Option<String>,
    recurrence: &Option<String>,
    anchor_date: &Option<DateTime<Utc>>,
) -> Result<(), ItemError> { ... }

/// due_offset_days is only meaningful on a child (mirrors Item::validate's own
/// due_offset_days-only-on-a-child rule).
fn validate_series_offset(
    parent_series_id: &Option<String>,
    due_offset_days: Option<i32>,
) -> Result<(), ItemError> { ... }
```

`validate_series_basis`: delete the `Some("DUE_DATE")` arm's "accept" branch — reject it
outright (`ItemError::Invalid("basis: DUE_DATE is no longer supported — task series always
materialize onto due_date")`) so a stale client can't silently write a now-meaningless value.

**Cursor/current-occurrence resolution** — a child has no cadence to resolve on its own, so
every caller that today does `recurrence::parse(&series.recurrence)` +
`current_occurrence_date(series, &rule, tz)` for a series that *might* be a child needs to go
through a new wrapper instead:

```rust
/// Resolves "the current occurrence date" for any series, root or child. A child delegates
/// entirely to its parent (decision 2) — one level of recursion, never more (decision 3, so no
/// loop-guard needed beyond what validate_series_parent already enforces at write time).
pub async fn resolve_current_occurrence_date(
    series_repo: &Arc<dyn ItemSeriesRepo>,
    series: &ItemSeries,
    tz_offset_minutes: i32,
) -> Result<DateTime<Utc>, ItemError> {
    let target = match &series.parent_series_id {
        Some(parent_id) => series_repo.get_series(parent_id).await?,
        None => series.clone(),
    };
    let rule = recurrence::parse(target.recurrence.as_deref().ok_or_else(|| {
        ItemError::Internal("root series missing recurrence".to_string())
    })?)
    .map_err(ItemError::Invalid)?;
    Ok(current_occurrence_date(&target, &rule, tz_offset_minutes))
}
```

Switch `list_occurrence_states_for_project`, `get_or_materialize_occurrence`,
`validate_completable`, `validate_uncompletable`, `record_task_completion`,
`record_task_uncompletion`, `skip_occurrence`, `unskip_occurrence` to call this instead of their
current inline parse-and-resolve wherever the series in question could be a child. The existing
plain `current_occurrence_date(series, rule, tz)` stays as the low-level primitive
`resolve_current_occurrence_date` itself calls — not removed, just no longer called directly by
these outer functions.

**Materialization** (`get_or_materialize_occurrence`):

- Drop the `is_due_date_basis(&series)` branch. New rule: `series.item_type == ItemKind::Task`
  → always `due_date`/`has_due_time`; `ItemKind::Event` → always `scheduled_date`/
  `has_scheduled_time` (unchanged for Events).
- For a child series, the `occurrence_date` parameter passed in by every caller *is the parent's
  cycle date* (see rendering below — a child's `ProjectOccurrence` identity is the parent's
  `occurrence_date`, not an independently computed one), and the item's actual `due_date` is
  `item::deadline_from_offset(occurrence_date, series.due_offset_days)` — reusing the exact
  domain helper item-level children already use, not a new one.
- `series_id: Some(series.id.clone())` unchanged — a materialized child occurrence's `series_id`
  points at the *child* series, not the parent, so `find_occurrence_by_item_id`/`skip_url_for_item`
  keep working unmodified for a materialized child item.
- Delete the `if let Some(template_id) = &series.template_item_id { copy_template_children(...) }`
  block entirely — no replacement needed, since a child series' own
  `get_or_materialize_occurrence` call (triggered independently, e.g. by completing it) is what
  creates its item now.

**Completion guard** (new, in `validate_completable`, gated by decision 5 above): before letting
a root Task series' current occurrence complete, fetch its children
(`series_repo.list_series_for_project(project_id)` filtered to
`parent_series_id == Some(series.id)` — small N, in-memory filter per the Schema section's
no-new-index note) and reject if any child's occurrence for this same cycle is
`OccurrenceState::Materialized` with an incomplete item. A `Virtual` child occurrence never
blocks. This mirrors `has_incomplete_children` (`src/service/items.rs`) closely enough that it's
worth checking whether that function's shape can be reused/generalized rather than
hand-duplicated — a small research step to do at implementation time, not a design fork.

**Rendering** (`list_occurrence_states_for_project`): after the existing per-series loop
produces `ProjectOccurrence`s for every *root* series (unchanged), add a second pass over every
series with `parent_series_id.is_some()`. For each, walk the `ProjectOccurrence`s already
produced for its parent within `[range_start, range_end]` (regardless of the parent's own
`OccurrenceState` — a child occurrence exists in lockstep with every parent cycle whether or not
the parent itself has settled) and synthesize one child `ProjectOccurrence` per parent cycle:

- `occurrence_date`: the *parent's* `occurrence_date` for that cycle — this is the identity/
  lookup key into the child series' own `item_occurrences` rows (`series_repo.get_occurrence
  (child.id, parent_occurrence_date)`), exactly the mechanism that lets a child be
  independently skipped/materialized on its own cycle-identity even though it shares dates with
  its parent.
- A new field on `ProjectOccurrence`, `display_date: DateTime<Utc>` (separate from
  `occurrence_date`, which stays the lookup identity) — for a root occurrence this always equals
  `occurrence_date`; for a child it's `item::deadline_from_offset(occurrence_date,
  due_offset_days)`, the date actually shown to the user and actually written to `due_date` on
  materialization. Every existing template/handler that currently reads `occurrence_date`
  directly for *display* needs to switch to `display_date` — `occurrence_date` itself stops
  being safe to render as-is once children exist. (Root occurrences are unaffected in practice
  since the two are equal there, but the rename makes the child case correct without a
  root-vs-child branch in every template.)
- `is_current`: mirrors the parent occurrence's own `is_current` — a child has no independent
  cadence to be "current" against (decision 2).
- Skip a child's synthesized row if `series_repo.get_occurrence` shows a state already recorded
  for it independently (materialized or exdate) — same `existing_by_ts`-style lookup the root
  loop already does, just against the child series' own `item_occurrences`.

**Delete/duplicate** (per the inferred decisions above): `delete_series` cascades to
`list_series_for_project` children (delete each child series — including its own
`item_occurrences` rows — before deleting the parent) rather than the orphan-not-cascade
treatment `DeleteItemSeries`'s doc comment specifies for *materialized items*. `duplicate_series`
likewise duplicates direct children (one level, per decision 3), re-pointing each copy's
`parent_series_id` at the new parent's id.

Critical files: `src/service/item_series.rs`, `src/domain/item_series.rs`,
`src/storage/sqlite/item_series.rs`, `src/storage/migrations/add_item_series_children.rs`,
`src/storage/sqlite/mod.rs`, `model/src/main/smithy/item_series.smithy`,
`src/json_api/item_series.rs`.

### Web UI (`src/web_ui/project_item_series/`, `templates/project_item_series/`)

- `new_page.html`/`edit_page.html`: remove the "Occurrence basis" schedule-vs-due-date `<select>`
  (basis now only ever toggles `COMPLETION` vs. default — rename the field's label from
  "Occurrence basis" to something reflecting it's purely about advance-timing now, e.g.
  "Advance timing") and remove the `templateItemId` `<select>` entirely. Add, conditionally shown
  only when creating/editing a `Task`-typed series: a "Parent series" `<select>` (populated from
  `list_series_for_project` filtered to root Task series in the same project, excluding the
  series being edited itself) and a "Due offset (days)" number input, shown only once a parent is
  picked — mirroring how `project_tasks`' own child-item form already conditionally shows its
  offset field. When a parent is selected, the recurrence/anchor-date fields should hide entirely
  (client-side, same `.completion-basis-field`/`.template-field` toggle pattern the form already
  uses for other conditional fields) since they're rejected server-side once a parent is set.
- `list_page.html`/`row.html`: a series list currently has no nesting concept — add an indented
  "sub-items" listing under each root Task series row (mirroring how `project_templates`' list
  shows a template's children indented under it), each with its own edit/delete affordance. New
  child series are created via the same `new_page.html` form (pre-selecting the parent from a
  "+ Add sub-item" link on the parent's row, the same UX shape `project_events/:id/children`'s
  "add sub-item" flow already establishes for items).
- Everywhere a `ProjectOccurrence` is rendered (`project_tasks/`, `project_events/`,
  `main_calendar.rs`, `project_calendar.rs`, `all_projects_tasks.rs`) — these already iterate
  `list_occurrence_states_for_project`'s output, so once that function emits child rows
  alongside root ones (see Rendering above), the main change needed per screen is switching
  date-rendering from `occurrence_date` to the new `display_date` field, and deciding whether a
  child's row renders indented under its parent in list views the same way a materialized item's
  own children already do, or as a flat peer row tagged with its parent's name — recommend
  indented-under-parent for visual consistency with how item-level children already render in
  every one of these same screens.

This is real, if mostly mechanical, work — budget it as its own stage rather than folding into
the service-layer stage above, since it touches many files across several screens and is easiest
to verify with a live smoke test (`task run`) once the service layer is solid, per this repo's
"don't use Playwright, verify by reading the diff + a careful smoke test" convention.

### CLI (`todo-cli/src/series.rs`) and MCP server (`mcp-server/src/index.ts`)

- `prl series create`/`update`: replace `--template <item-id>` with `--parent-series <id>` and
  `--offset <days>`; `--recurrence`/`--anchor` become optional flags, required only when
  `--parent-series` is absent (clap's `required_unless_present`, mirroring precedent elsewhere in
  `todo-cli` for mutually-conditional flags — check `todo-cli/src/items.rs` for an existing
  example of that pattern before inventing a new one). `--basis` loses its `"due-date"` value
  (`parse_series_basis_flag`'s match arm) — `schedule`/`completion` remain, but `schedule` is now
  the *only* materialization target for Events, and simply the "advance timing" choice for Tasks
  (no display-shape change on the item itself).
- `prl series get`: drop the `template:` line, add `parent:`/`offset:` lines.
- MCP `create_item_series`/`update_item_series` tool schemas: drop `templateItemId`, add
  `parentSeriesId`/`dueOffsetDays`; `basis` enum loses `"DUE_DATE"`; `recurrence`/`anchorDate`
  descriptions gain a note that they're required only when `parentSeriesId` is omitted. Rebuild
  via `npm run build` after editing `src/index.ts` (changes aren't live until this runs, per
  CLAUDE.md's touch-point checklist).
- `docs/prl-user-guide.md`: update the `series create`/`series update` command docs to match.

## Staging

Four independently-committable stages, in dependency order:

1. **Schema + domain + storage + Smithy** — migration, `ItemSeries` struct, sqlite repo,
   `.smithy` file + `task codegen`, fix resulting compile errors in `src/json_api/item_series.rs`.
   Nothing behavioral changes yet; `create_series`/`update_series` can temporarily reject
   `parentSeriesId`/`dueOffsetDays` with a not-yet-implemented error, or this stage can include
   just enough of the service-layer plumbing (new fields threaded through
   `CreateItemSeriesParams`/`UpdateItemSeriesParams`, still unvalidated) to make the round trip
   compile — whichever keeps this stage's diff cleanly separable from stage 2's actual behavior
   change. Recommend the latter (plumb but don't yet validate), since splitting "add a field" from
   "make the field do anything" cleanly is exactly what CLAUDE.md's own migration precedents
   (`AddItemSeriesTemplateItemId` before `MigrateLegacyRecurringItems`) already model.
2. **Service layer** — all the validation/resolution/materialization/rendering/guard changes
   above. This is where `basis: DUE_DATE` actually stops being accepted and Task series actually
   switch to due-date-only materialization — the one genuinely behavior-changing stage for
   existing data (decision 5).
3. **Web UI** — forms, list nesting, calendar/list rendering switched to `display_date`.
4. **CLI + MCP server + docs** — `todo-cli/src/series.rs`, `mcp-server/src/index.ts` (+
   `npm run build`), `docs/prl-user-guide.md`, and finally moving this plan's originating entry
   in `docs/issues_and_features.md` into `docs/archived/archived_issues_and_features.md` once
   stage 4 lands.

Per this repo's established convention for multi-stage plans: commit at the end of each stage,
and it's fine to clear context and pick this doc back up cold before starting the next one —
each stage above is written to be actionable on its own, referencing exact files and functions
rather than "see the discussion above."
