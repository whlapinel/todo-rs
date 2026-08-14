# Rough plan: virtual/computed recurring-event occurrences (RRULE-style)

Status: **rough sketch, deliberately not implementation-ready — not being pursued now.** Captured 2026-08-13 from a planning discussion, for reference if this app's needs outgrow the lighter approach actually chosen (`docs/event-complete-removal-plan.md`, "auto-advance on read"). This doc is intentionally lower-fidelity than a real plan — revisit and flesh out properly before ever implementing.

## When to revisit this

The auto-advance-on-read model (chosen instead, see the sibling doc) only ever materializes **one occurrence at a time** — "what's the next occurrence of this recurring thing," recomputed lazily when stale. That's sufficient for how this app is actually used today. This fuller model becomes worth its cost only if a real need shows up to **browse a range of a recurring series** — e.g. a calendar view that should show "every Monday meeting for the next 3 months," not just the next one — since that's the one thing the lighter model structurally can't do (it only ever has one row per series, representing whichever occurrence is current).

## Core model shift

Today, one `Item` row conflates two things: "the recurrence definition" (pattern, basis) and "the currently-active instance" (a concrete `scheduled_date`, editable fields, an id other things can link to). A real RRULE-style model needs these to be separate concepts:

- **A series**: the recurrence rule + anchor/start date + the event's static fields (name, description, `event_type`, etc.) — this is the thing that has *no* single date, just a rule.
- **An occurrence instance**: a specific date arising from the series, which may or may not be materialized as a real row.

This is exactly the iCalendar (RFC 5545) model: a series (`RRULE`), plus exceptions layered on top — `EXDATE` ("skip this one date entirely") and an overridden instance keyed by `RECURRENCE-ID` ("this one instance has a different time/title/etc. than the rule would produce").

## The materialization problem (the actual hard part)

Every `Item` in this app today is an addressable row with a real id — detail pages, edit links, and `sourceEventId` (a Task pointing at one specific Event) all depend on that. A purely virtual, never-persisted occurrence has no id to hang a detail page, an edit, or a task-link off of. So a genuinely "pure" virtual model doesn't actually work end-to-end for this app — the moment a user wants to *interact* with one occurrence (open it, edit it, link a task to it), it has to become a real row.

The realistic shape is therefore **lazy materialization**: occurrences are computed/virtual for *display* (listing a date range), but the first time someone touches a specific occurrence, a real row gets created for it (carrying the `RECURRENCE-ID`-equivalent — which date in the series this row represents), and from then on that occurrence is "materialized" and behaves like today's items. An occurrence a user explicitly wants to skip becomes an `EXDATE`-equivalent marker on the series rather than ever materializing.

## What would actually need to change

1. **Range-based occurrence generation** in `src/domain/recurrence.rs`. Today's `next_date()` only computes *one* date after a reference point. A real implementation needs something like `occurrences_between(rule, anchor, range_start, range_end) -> Vec<DateTime<Utc>>` — a materially different algorithm (an iterator/generator over the rule within a window), with its own edge cases at month/year/DST boundaries distinct from the single-next-date logic that exists today.

2. **A series/instance split in the domain model** — whether this is a schema change (a new `event_series` concept alongside `items`) or a reinterpretation of the existing `Item::Event` row (with materialized occurrences as separate rows referencing it) is an open design question, not resolved here.

3. **Every current single-`scheduled_date`-value consumer** needs to become range-aware, or explicitly declare "I only want the next occurrence": the project dashboard's list and calendar-month views, the CSV importer/exporter, `Item::is_overdue`, any sort/filter by date. Each of these currently just reads `item.scheduled_date()` as one value.

4. **The calendar month-grid view** specifically would need to expand each series into its occurrences within the visible month for display, most of which would be non-materialized/synthetic (not directly editable — clicking one would trigger materialization first).

5. **`sourceEventId` redesign** — today a Task points at one concrete Event row's id. Under this model it would need to either point at the series and resolve "whichever occurrence is current" at read time (reintroducing a live-computation dependency similar to what auto-advance-on-read already does, just relocated), or allow linking to a specific occurrence that may not be materialized yet (requiring the link itself to trigger materialization).

6. **The lighter mechanism becomes redundant once this lands** — auto-advance-on-read's "one row, kept fresh" model is subsumed by "many correctly-computed occurrences, one of which is always the fresh one." If this is ever built, the auto-advance logic from the sibling doc can be retired.

## Rough sizing

This is closer to "build a recurring-events engine" than any single-session change — it touches the domain model's core identity assumption (one row = one addressable thing), the calendar rendering, the `sourceEventId` feature, and every date-reading surface in the app. Expect real edge-case-prone test surface around occurrence generation (this is exactly the class of bug that's easy to get subtly wrong — the kind of thing `recurrence.rs`'s existing single-`next_date` tests already had to be careful about, multiplied across range generation instead of a single lookup).

No schema, migration, or API-shape decisions are finalized in this document — treat it as a starting point for a real planning session if range-browsing of recurring series ever becomes an actual need, not as something to implement as-is.

## Staged breakdown (added 2026-08-13, still not being pursued now)

The section above deliberately leaves the schema question open ("an open design question, not resolved here"). A *staged* plan can't do that — each stage has to build on a concrete decision made by an earlier one. So this section commits to one working data-model shape, purely so the work can be cut into stages that each compile, test, and commit independently, with context cleared in between. If this is ever picked up for real, re-validate these assumptions first — don't just start executing stage 1 on stale reasoning.

### Working assumptions this staging depends on

- **New tables, not a reinterpretation of `items`.** A new `event_series` table holds the recurrence rule + anchor date + the event's static fields (name, description, `event_type`, `project_id`, ...). A new `event_occurrences` table tracks each date's state: `(series_id, occurrence_date, item_id NULL, is_exdate BOOL)`. This avoids overloading `Item`'s existing shape (see "materialization problem" above) — every other consumer of `items` keeps working unmodified.
- **A materialized occurrence is an ordinary `items` row** (`item_type = Event`), created via `get_or_materialize_occurrence` instead of a normal create call. Its detail page, edit form, `sourceEventId` target, points/activity — everything — works exactly like any Event does today. Only *how it came to exist* differs.
- **Purely additive.** Existing non-series Events are untouched; nothing forces migrating them into a series. A series is an opt-in creation path.
- **Skip semantics deferred to stage 6** — whether skipping an already-materialized occurrence deletes its `items` row or just orphans it is an open call left to that stage, not decided here.

If the actual implementation ends up choosing a different shape (e.g. reinterpreting the `Item::Event` row itself as the series), this staged breakdown doesn't hold — redo the staging, don't patch around it.

### Stages

Each stage is scoped to be independently committable and testable; "clear context" below each heading marks a safe point to compact/reset before starting the next stage.

**Stage 1 — Range-based occurrence generation** (covers change #1 above)
- Add `occurrences_between(rule: &RecurrenceRule, anchor: DateTime<Utc>, range_start: DateTime<Utc>, range_end: DateTime<Utc>) -> Vec<DateTime<Utc>>` to `src/domain/recurrence.rs`.
- Pure function, no schema/DB/consumer changes. Exhaustive unit tests for month/year/DST boundaries, matching the care `next_date`'s existing tests already take.
- Verify: `cargo test` (recurrence module only needs to pass — nothing else is touched).
- Commit: "Add range-based occurrence generation to recurrence.rs".
- Safe to clear context after — nothing later depends on anything but this function's signature.

**Stage 2 — `event_series`/`event_occurrences` storage layer** (covers change #2 above)
- New migration under `src/storage/migrations/` plus matching `CREATE TABLE IF NOT EXISTS` additions in `create_pool()` (per CLAUDE.md's "Adding a DB column"/schema workflow).
- New repo trait, e.g. `EventSeriesRepo` (`#[cfg_attr(test, mockall::automock)]` per existing convention): `create_series`, `get_series`, `list_series_for_project`, `get_occurrence`, `record_materialized_occurrence`, `mark_exdate`, `list_occurrences_between`.
- No handler/service wiring yet — repo-level tests only (mocked in later stages, exercised directly here via a real SQLite pool the way other repo tests do).
- Commit: "Add event_series/event_occurrences storage layer".

**Stage 3 — Materialization service** (covers change #2's runtime half)
- New `src/service/event_series.rs`: `get_or_materialize_occurrence(series_id, date) -> Result<Item, ItemError>` — returns the existing materialized Item if `event_occurrences` already has one for that date, otherwise builds a new Event `Item` from the series' static fields + `date`, persists it via the existing item-creation path, records the mapping, returns it.
- `skip_occurrence(series_id, date) -> Result<(), ItemError>` — sets `is_exdate`; exact behavior toward an already-materialized row deferred to stage 6.
- Still no web_ui/json_api wiring — unit-testable in isolation against mocked repos, same style as existing `service/` tests.
- Commit: "Add occurrence materialization service".
- **Committed** (`4e57677`), per the refined plan at `~/.claude/plans/can-you-tell-me-jiggly-spring.md`: `get_or_materialize_occurrence`/`skip_occurrence` delegate item creation to `project_items::create_project_item` rather than a hand-rolled dispatch of its own — that delegation choice (raised as an open question in an earlier, since-superseded refined-planning pass) is settled and not up for reconsideration. Implemented as specified above, no deviations. 7 new unit tests, full suite (262 tests) and `cargo check` clean.

**Stage 4 — Series CRUD surface** (Smithy + json_api + minimal web UI)
- New Smithy operations for creating/reading/updating a series (rule, anchor, static Event fields) — run `task codegen` and follow CLAUDE.md's "Adding or removing an operation" touch-point checklist in full (handler wiring, MCP tool, CLI subcommand, docs).
- Minimal web UI: a form to create a series and see it listed. Deliberately not enough yet to browse or interact with its occurrences (that's stage 5).
- This is the largest single stage — touches the most files per CLAUDE.md's checklist. Consider sub-splitting further (e.g. Smithy+codegen+json_api as 4a, web UI as 4b) when actually executing, rather than in this rough doc.
- Commit: "Add series CRUD (Smithy + service + minimal web UI)".

**Stage 5 — Range-aware display** (covers changes #3 and #4 above)
- Wire `occurrences_between` + `event_occurrences` into the project dashboard's list/calendar views and the Events month-grid view: for each series overlapping the visible range, render its occurrences — materialized ones link to their real detail page, virtual ones render distinctly (no edit link, a "create/open" affordance).
- Clicking a virtual occurrence calls `get_or_materialize_occurrence` then redirects to the resulting item's detail page.
- Non-series Events (today's model) render unchanged — this stage only adds a second, parallel rendering path for series.
- Commit: "Wire series occurrences into dashboard/calendar views".

**Stage 6 — Skip (EXDATE) UI**
- Add a "skip this occurrence" action wherever an occurrence (virtual or materialized) is shown; resolve the deferred skip-after-materialize behavior from stage 3 here.
- Commit: "Add skip-occurrence (EXDATE) UI".

**Stage 7 — `sourceEventId` → series support** (covers change #5 above)
- Let a Task's `sourceEventId` point at a series, resolved to "whichever occurrence is current" at read time (materializing on demand if needed).
- Highest-risk stage: touches the template-trigger matching and `copy_template_children` logic described in CLAUDE.md's Events section, which today assume a concrete Event item id. Budget extra time for this one; don't bundle it with anything else.
- Commit: "Support sourceEventId pointing at a recurring series".

**Stage 8 — Retire the superseded single-row path, import/export** (covers change #6 above)
- Decide whether Event's old `recurrence`/`recurrence_basis` auto-advance-on-read mechanism (see the sibling doc, if it was built) gets retired now that series cover the same need for Events specifically — Tasks keep the old mechanism regardless, since this doc only ever addressed Events.
- Update `src/service/import.rs`/the PRL CSV format if series should be importable/exportable.
- Commit: "Retire single-row recurrence path for Events now superseded by series".

### Notes on staging itself

- Stages 1–3 have no user-visible effect and are safe to land well ahead of the rest — they're pure addition with no existing call site touched.
- Stage 4 is the first point where the touch-point checklist (Smithy/codegen/CLI/MCP/docs) applies in full; everything before it is Rust-only.
- Stage 7 is deliberately last-but-one, not folded into stage 4 or 5, because it's the one place this feature reaches into *existing* behavior (event-trigger matching) rather than only adding new surface — isolating it makes it easier to revert on its own if it goes sideways.
- As in the rest of this doc: none of this is scheduled or approved for implementation. This breakdown exists so that *if* it's picked up, the work doesn't have to be re-planned from scratch first.

**Each stage below should get its own refined-planning pass before implementation, not just be executed off this doc's one-paragraph sketch.** Stage 1 got this treatment (a dedicated plan-mode session, resolving specific open questions like whether to hand-roll occurrence generation or adopt the `rrule` crate, and the exact `MonthlyDay`-clamp-vs-skip semantics, via `AskUserQuestion` before any code was written — see the git history around `40eca9d`). Stage 2 did not: it was implemented directly off this doc's method-name list and table sketch, with several unstated design calls (upsert semantics via `ON CONFLICT`, storing `recurrence` as a raw string rather than a structured column, `mark_exdate` preserving an existing `item_id` rather than clearing it, index shape) made ad hoc during implementation rather than surfaced and confirmed first — acceptable there mainly because every call mirrored an existing, already-reviewed precedent (`ProjectRepo`, `ItemRepo`, `ActivityLogRepo`). Later stages (especially 4, 5, and 7 — see the sizing/risk notes above) have design surface this doc doesn't resolve at all (exact Smithy shape, how virtual-vs-materialized renders in the calendar grid, the `sourceEventId`-to-series resolution semantics) and should not be implemented directly from their one-paragraph descriptions here.
