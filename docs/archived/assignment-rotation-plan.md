# Assignment rotation for item series — design sketch (complete, all 5 stages done)

Status: **Complete.** All 5 stages (storage, service, Smithy/codegen/json_api,
web UI, CLI/MCP/docs) implemented and tested — see "Suggested staged
rollout" at the bottom. Moved from `docs/issues_and_features.md` to
`docs/archived/archived_issues_and_features.md` (2026-08-20).

## Implementation status

- **Stage 1 (storage)**: `item_series_rotation_members` table (baseline +
  migration 24), `ItemSeriesRepo::list_rotation_members`/`set_rotation_members`,
  fully tested.
- **Stage 2 (service, `src/service/item_series.rs`)**: `occurrence_index`/
  `rotation_assignee` pure functions; `resolve_occurrence_assignee` (fixed
  assignee or rotation-computed, used by `get_or_materialize_occurrence`);
  `resolve_series_assignment` grows a `rotation_user_ids` parameter with the
  mutual-exclusion/empty-list/membership validation described below;
  `CreateItemSeriesParams`/`UpdateItemSeriesParams` gain `rotation_user_ids:
  Option<Vec<String>>`; `create_series`/`update_series` persist rotation
  membership via `set_rotation_members`; `duplicate_series` copies rotation
  membership onto the new series; `list_occurrence_states_for_project` resolves
  each occurrence's assignee individually for a rotating series instead of once
  per series.
- **Stage 3 (`model/src/main/smithy/item_series.smithy`,
  `src/json_api/item_series.rs`)**: added a `StringList` shape and a
  `rotationUserIds: StringList` field (optional, `@notProperty` on the
  input/output structures) to `ItemSeriesSummary`, `CreateItemSeries` input,
  `GetItemSeries` output, and `UpdateItemSeries` input — both
  `assignedToUserId` and `rotationUserIds` stay present on the wire
  simultaneously, per the plan. `task codegen` run after. `json_api`'s
  `create_item_series`/`update_item_series` now forward `input.rotation_user_ids`
  straight through to the service layer instead of hardcoding `None`.
  `get_item_series`/`list_item_series_for_project`/`to_summary` now call
  `ItemSeriesRepo::list_rotation_members` (the domain `ItemSeries` struct
  itself still doesn't carry rotation membership, per Stage 2's design) and
  populate `rotationUserIds` on the response, using `None` (not `Some(vec![])`)
  when the list is empty so a non-rotating series's wire shape is unchanged
  from before this stage.
- **Stage 4 (web UI, `templates/project_item_series/{new,edit}_page.html`,
  `src/web_ui/project_item_series/{handlers,templates}.rs`,
  `src/web_ui/project_tasks/{handlers,templates}.rs`,
  `src/service/item_series.rs`)**: implemented and manually smoke-tested
  end-to-end against a live local server (create rotating series, list-page
  display, edit-page round-trip, mode switch, empty-rotation validation —
  see "Web UI implementation notes" below for what differed from the
  original sketch and why).

## Motivation

`ItemSeries` (`src/domain/item_series.rs`) today carries a single fixed
`assigned_to_user_id`/`points` pair (CLAUDE.md's Points section) — every
occurrence a Task-typed team series materializes gets the same assignee
forever. The user wants chores/recurring tasks that **rotate** across a set of
people (e.g. "trash day" alternates Alice/Bob/Carol each week) without having
to manually re-edit the series' assignee after every occurrence.

## Decisions already made (2026-08-20)

Two forks were resolved via direct discussion with the user; both took the
lower-complexity option, consistent with this codebase's general bias toward
deriving state rather than tracking it:

1. **Stateless / positional rotation**, not stateful/advance-on-settle. The
   assignee for a given occurrence is a pure function of that occurrence's
   *calendar position* in the series (its index from `anchor_date`), computed
   fresh every time — no new mutable "whose turn is it" cursor, no interaction
   with the existing `cursor_date`/skip/unskip/uncomplete guard machinery
   (already the most fragile part of `service/item_series.rs`, per its own
   extensive doc comments). A skipped occurrence simply doesn't reassign
   anything to anyone that round — the rotation schedule itself never moves.
2. **Unordered set, stable derived order** — not an explicitly user-ordered
   list. The user picks *which* project members are in the rotation; the
   cycle order is derived deterministically (sorted by `user_id`) rather than
   separately authored. This avoids a new position-tracking table and avoids
   needing a reorderable-list UI widget (this app has no browser-side JS
   framework — see CLAUDE.md's Web UI section — so drag/up-down reordering
   would be meaningfully more UI work than a plain multiset of checkboxes).

## Core algorithm

Reuses `domain::recurrence::occurrences_between` (already what
`current_occurrence_date`/`list_occurrence_states_for_project` build on) rather
than adding new date arithmetic:

```rust
/// Index of `occurrence_date` within `rule`'s sequence starting at `anchor`,
/// 0-based. `occurrence_date` is always itself a member of that sequence (every
/// caller derives it from the same rule), so this never returns None in practice.
fn occurrence_index(rule: &RecurrenceRule, anchor: DateTime<Utc>, occurrence_date: DateTime<Utc>, tz_offset_minutes: i32) -> usize {
    recurrence::occurrences_between(rule, anchor, anchor, occurrence_date, tz_offset_minutes).len() - 1
}

/// Pure: same inputs always produce the same assignee, regardless of
/// materialization order, skip history, or which occurrence is touched first.
fn rotation_assignee(rotation: &[String], index: usize) -> Option<&String> {
    if rotation.is_empty() { None } else { Some(&rotation[index % rotation.len()]) }
}
```

**Known accepted tradeoff:** `occurrences_between` walks the full sequence from
`anchor` to `occurrence_date` to produce the count, which is O(occurrences so
far) rather than O(1). Fine for realistic chore cadences (weekly/monthly over
months/years); would get slow for e.g. a daily series left running for
several years. Not worth a closed-form per-unit index calculation (which would
duplicate `advance`/`to_rrule`'s per-`RecurrenceUnit` clamping logic) unless
this actually shows up as a problem — noted here so it isn't rediscovered as a
surprise later.

For a completion-basis Task series (`is_completion_basis`), the "index" is
still measured from the series' fixed `anchor_date`, **not** from
`current_occurrence_date` — completion-basis only changes *when* the next
occurrence's date lands, never what `anchor_date` was, so the position-in-
sequence (and therefore rotation assignment) stays well-defined and stable
even as the schedule itself drifts.

## Data model

Rotation membership is a genuinely different shape from today's single
`assigned_to_user_id: Option<String>` scalar — a new table, not a packed
string column (unlike `recurrence`'s raw-string convention, membership here
benefits from real per-row FK-shaped storage the way `item_occurrences`/
`project_members` already get):

```sql
CREATE TABLE IF NOT EXISTS item_series_rotation_members (
    series_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    PRIMARY KEY (series_id, user_id)
);
```

No `position` column — order is derived at read time via `ORDER BY user_id
ASC` (stable, deterministic, no extra join needed at the materialization hot
path). New migration file `add_item_series_rotation_members.rs`, following the
existing `add_item_series_*` migrations' pattern; also added to the baseline
`CREATE TABLE IF NOT EXISTS` block in `create_pool()`.

`ItemSeriesRepo` (`src/storage/sqlite/mod.rs`) gains:

```rust
async fn list_rotation_members(&self, series_id: &str) -> Result<Vec<String>, RepoError>; // sorted by user_id
async fn set_rotation_members(&self, series_id: &str, user_ids: &[String]) -> Result<(), RepoError>; // delete+reinsert, like a full-replace update
```

`ItemSeries` (domain struct) does **not** grow a `rotation_members` field
itself — membership is loaded separately via the new repo methods wherever
needed, the same way `item_occurrences` rows are looked up separately rather
than embedded on the struct. `assigned_to_user_id`/`points` stay exactly as
they are today.

**Fixed vs. rotating are mutually exclusive** per series: setting
`rotationUserIds` clears `assignedToUserId` and vice versa, validated at the
service layer (same shape as the existing `event_type`/`item_type`
either/or precedents). `points` is unaffected either way — a rotating
series still has one `points` value, awarded to whichever member is up for
that occurrence.

## Service layer (`src/service/item_series.rs`)

- `resolve_series_assignment` grows a third branch: today it's
  `(None, None)` or `(Some(fixed_assignee), points)`; add
  `(Rotating(Vec<String>), points)` — same Task-type + team-backed-project
  gate as today, plus: reject if `assignedToUserId` and `rotationUserIds` are
  both provided; validate every rotation member via the same
  `resolve_project_assignee`-style membership check the fixed case already
  does (loop, or a new bulk variant); reject an explicitly-provided but empty
  rotation list as `ItemError::Invalid` (ambiguous with "clear it") rather
  than silently treating it as "no rotation."
- `get_or_materialize_occurrence`: replace the two
  `assigned_to_user_id: series.assigned_to_user_id.clone()` lines with a call
  to a new `resolve_occurrence_assignee(&series, rotation_members, occurrence_date, tz_offset_minutes) -> Option<String>`
  that returns the fixed assignee if the series isn't rotating, or
  `rotation_assignee(&rotation, occurrence_index(...))` if it is. Requires
  loading `list_rotation_members` in this function — one extra query, only on
  the "series has no fixed assignee" path (skippable via an
  `Option<Vec<String>>`-typed series-level flag if this ever becomes a hot
  path worth avoiding the query on non-rotating series — likely unnecessary
  given series aren't materialized often).
- `list_occurrence_states_for_project`: currently resolves
  `assigned_to_user_id`/`assigned_to_user_name` **once per series**, outside
  the per-date loop (lines ~1043–1058), then reuses it for every occurrence.
  For a rotating series this must move **inside** the per-date loop — each
  virtual occurrence's preview needs its own computed assignee before it's
  ever materialized, so what the calendar/list view shows matches what
  clicking into that occurrence will actually produce. Name lookups should
  still batch/cache via the existing `names: HashMap<String, String>` map
  (already keyed by `user_id`), just now populated by multiple distinct ids
  per series instead of at most one.

## Smithy (`model/src/main/smithy/item_series.smithy`)

Add `rotationUserIds: StringList` (a `list<String>`, no `@required`) to
`ItemSeriesSummary`, `CreateItemSeries` input, `GetItemSeries` output, and
`UpdateItemSeries` input — same optional/`@notProperty` shape
`assignedToUserId` already has on each. `task codegen` after. Both
`assignedToUserId` and `rotationUserIds` stay present on the wire
simultaneously (client always sees which mode is active by which one is
populated); the mutual-exclusion rule is enforced service-side on write, not
schema-side (Smithy has no clean "exactly one of" constraint here, and this
matches how e.g. the `basis`/`templateItemId` optional-string precedents are
already handled).

## Web UI (`templates/project_item_series/`, `src/web_ui/project_item_series/`)

`new_page.html`/`edit_page.html`'s single `<select name="assignedToUserId">`
(the `.assignment-field` block) becomes a mode toggle:

- **Fixed** (today's behavior) — the existing single `<select>`.
- **Rotate among** — a plain `<input type="checkbox" name="rotationUserIds" value="{id}">`
  per project member (no framework needed — a checkbox group, not a
  reorderable list, per the "unordered set" decision above). Order shown in
  the UI can just be alphabetical-by-name (matching the derived storage
  order), so what's displayed already matches the actual cycle order with no
  separate "here's the order" explanation needed.

A toggle (radio buttons or a `<select>`) switches which block is visible/
active, mirroring the existing `.completion-basis-field`/`.template-field`
show/hide JS already in these two templates (`form.querySelectorAll(...)`).

`row.html` and any series detail/calendar display should show something like
`Rotating: Alice, Bob, Carol` instead of `Assigned to: {name}` when rotating,
and — where a specific occurrence is being shown (calendar day, occurrence
detail) — the actually-resolved-for-that-date assignee, not the whole set.

**Superseded by open question 4's resolution below** (dated the same day,
after this section was drafted): the list row instead reuses the existing
`Assigned to: {name}` line unchanged, populated with the *current*
occurrence's resolved assignee for a rotating series — no separate "Rotating:
..." display was built. See "Web UI implementation notes" below for what was
actually implemented.

## CLI (`todo-cli/src/`) / MCP (`mcp-server/src/index.ts`)

- `prl series create`/`update` gain a repeatable `--rotate <user-id>` flag
  (mirrors how multi-value flags are already done elsewhere in `prl`, e.g.
  check existing repeated-flag precedent before inventing a new convention);
  mutually exclusive with `--assign` at the CLI's own validation layer,
  matching `resolve_series_assignment`'s server-side rule (fail fast
  client-side, still enforced server-side regardless).
- `prl series get` / `get_item_series` MCP tool should surface
  `rotationUserIds` in its output the same way `assignedToUserId` already is.
- `create_item_series`/`update_item_series` MCP tools gain an optional
  `rotationUserIds: string[]` parameter alongside the existing
  `assignedToUserId`.
- `docs/prl-user-guide.md` gets a rotation section once implemented.

## Open questions — resolved (2026-08-20)

All four resolved by reading the existing assignment-resolution code path
rather than by new design; none of them require new mechanism.

1. **Removing a rotation member who's already had turns.** Confirmed
   no-op: `rotation_assignee` is `index % rotation.len()`, recomputed fresh
   on every call with no stored "whose turn" state — shrinking the set
   just changes future modulo results. Already-materialized past
   occurrences are plain items, untouched either way. One-line note in the
   eventual code comment, not a design change.
2. **A rotation member who's no longer a project member at all.** Also
   already handled, for free — traced through
   `get_or_materialize_occurrence` → `create_project_item` →
   `create_team_item` (`src/service/team_items.rs:177-183`): **every**
   materialization, including ones where `assigned_to_user_id` was carried
   forward from a series (`params.series_id.is_some()`), calls
   `resolve_project_assignee` (`src/service/projects.rs:51-69`)
   unconditionally, which hard-errors (`ItemError::Invalid("assignee must
   be a member of this project")`) if that user isn't currently a project
   member. This is already true today for the *fixed*-assignee case — a
   series whose fixed assignee left the project already fails to
   materialize any further occurrences. Rotation's computed assignee flows
   through this identical path, so it inherits the identical failure mode
   with zero new code. Not redesigning this (e.g. auto-pruning a departed
   member from the rotation, or silently skipping to the next person) —
   that would be a behavior change from what fixed-assignee series already
   do, out of scope for this feature.
3. **Manual override of a single occurrence's assignee.** Confirmed free.
   `update_team_item` (`src/service/team_items.rs:436-464`) already lets a
   materialized item's `assigned_to_user_id` be edited like any other team
   item field, revalidated the same way. Since
   `get_or_materialize_occurrence`'s cache-hit branch
   (`src/service/item_series.rs:45-57`) returns an already-materialized
   occurrence's item untouched — the rotation computation only ever runs on
   *first* materialization — a manual edit sticks permanently and is never
   recomputed or clobbered by the rotation logic later. No new feature
   needed; this is just "edit the item," already true today.
4. **Display of "who's up next."** Decided: show the series row/detail
   view's *current* occurrence's resolved assignee
   (`current_occurrence_date` + `occurrence_index`), replacing today's
   static "Assigned to: {name}" the same way for a rotating series as it
   already does for a fixed one — not the whole rotation set, not the
   *next* occurrence. Keeps the rotating case visually consistent with the
   fixed case rather than introducing a new display concept.

## Web UI implementation notes (Stage 4, 2026-08-20)

Three things came up during implementation that the sketch above didn't
anticipate:

1. **Checkbox group can't use a repeated `name` — axum 0.6's `Form`
   extractor can't deserialize it.** The original sketch's `<input
   type="checkbox" name="rotationUserIds" value="{id}">` per member (a
   repeated same-named key, the standard HTML multi-value-checkbox pattern)
   was tried first and confirmed broken via a live smoke test: axum 0.6 is
   backed by `serde_urlencoded`, which cannot deserialize repeated
   same-named form keys into a `Vec` — every submission 422s with `"invalid
   type: string ..., expected a sequence"`, even with the key repeated.
   Fixed by giving the checkboxes no `name` at all (just a
   `.rotation-member-checkbox` class) and adding one real `<input
   type="hidden" name="rotationUserIds">` per form, kept in sync with
   whichever boxes are checked (comma-joined) by each page's own `<script>`
   on every checkbox `change` and on the existing TASK/EVENT and Fixed/Rotate
   toggle re-renders. The Rust-side form field is `Option<String>`
   (comma-joined), not `Option<Vec<String>>` — split/trimmed/filtered into
   the `Vec<String>` `Create/UpdateItemSeriesParams` expects in
   `resolve_assignment_mode_fields` (`src/web_ui/project_item_series/
   handlers.rs`). This makes the checkbox group JS-dependent for submission,
   same as the pre-existing TASK/EVENT toggle it sits inside of — not a new
   class of degradation for this form.
2. **A rotating occurrence's own detail/edit page needed threading through,
   not just the series list.** `project_tasks::handlers::
   render_series_occurrence_detail_page`/`render_series_occurrence_edit_page`
   (the still-virtual-occurrence preview/edit-form pair) were reading
   `series.assigned_to_user_id` directly, which is always `None` for a
   rotating series — the detail page would've shown "unassigned" for an
   occurrence that actually has a computed rotation assignee, and worse, the
   edit form's `<select>` would've pre-selected "Unassigned," so saving it
   *unmodified* would have silently overwritten the just-materialized
   correct assignee with `None` (`overlay_str` in `project_tasks/mod.rs`
   always applies whatever the select submits). Fixed by making
   `resolve_occurrence_assignee` (`src/service/item_series.rs`) `pub(crate)`
   and calling it from both handlers before building
   `ProjectTaskSeriesOccurrenceView`/`Fields`, which now take a
   `resolved_assignee_id: Option<String>` parameter instead of reading
   `series.assigned_to_user_id` themselves. This required adding an
   `item_series: &Arc<dyn ItemSeriesRepo>` parameter to both handler
   functions and threading it from their one caller,
   `project_item_series::handlers`'s occurrence detail/edit dispatch.
3. **List-page "who's up now" needed a new service function, not reuse of
   `resolve_occurrence_assignee` directly** — the series list shows one row
   per series, not per occurrence, so there's no `occurrence_date` on hand.
   Added `current_series_assignee` (`src/service/item_series.rs`, `pub`):
   fixed assignee if set, else — for a rotating Task series — resolves
   `current_occurrence_date` and delegates to `resolve_occurrence_assignee`.
   Used by `project_item_series::handlers::project_item_series_page` (looped
   with `.await` per row instead of the prior synchronous `.iter().map`,
   since resolving now needs a repo call per series).

Manually verified end-to-end against a live local server (`TODO_AUTH_MODE=
caddy` + `TODO_DEV_EMAIL`, throwaway SQLite DB): created a rotating series,
confirmed the list row showed the resolved current assignee, confirmed the
edit page pre-selected Rotate mode with the right member checked, round-tripped
an unmodified save, switched Rotate→Fixed and back, and confirmed submitting
Rotate mode with nothing checked 422s (the "explicitly empty rotation" guard).

## Suggested staged rollout

Design is now complete enough to implement — no open questions block
starting. Rough shape, to be refined into actual numbered stages (matching
this project's `docs/*-plan.md` convention) once implementation begins:

1. Storage: migration + `ItemSeriesRepo` rotation methods + tests. **Done.**
2. Service: `resolve_series_assignment` rotation branch,
   `resolve_occurrence_assignee`/`occurrence_index`, wire into
   `get_or_materialize_occurrence` and `list_occurrence_states_for_project`.
   **Done.**
3. Smithy + codegen + json_api wiring. **Done.**
4. Web UI: create/edit forms, row/detail/calendar display. **Done** — see
   "Web UI implementation notes" above.
5. CLI + MCP + user guide. **Done.** `prl series create`/`update` gained a
   repeatable `--rotate <user-id>` flag (mutually exclusive with `--assign`,
   validated client-side), `prl series get` prints the resolved rotation.
   MCP `create_item_series`/`update_item_series` gained an optional
   `rotationUserIds: string[]` parameter, passed through unchanged.
   `get_item_series`/`list_item_series` needed no code change — they proxy
   the JSON API response verbatim, which already included `rotationUserIds`
   from Stage 3. `docs/prl-user-guide.md`'s Item Series section documents
   `--rotate` (and, as a byproduct, the previously-undocumented series
   `--assign`/`--points` flags). No `task codegen` rerun needed — the
   generated `todo-client`'s `rotation_user_ids()` builder/accessor methods
   already existed from Stage 3.
