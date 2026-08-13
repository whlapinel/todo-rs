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
