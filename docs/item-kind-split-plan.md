# Split the `Item` domain model into per-kind Rust types

**Status: Stage 0 complete (audit). Stages 1–8 not started.** This is the canonical copy — edit this file in place as stages complete. It originated in a Claude Code plan-mode session (`~/.claude/plans/splendid-wishing-sphinx.md`), copied here so it survives independently of that session.

## Context

`src/domain/item.rs` already went through one round of this exact idea: `Item.item_type: ItemType` is an enum (`Task { schedule, recurrence, team_assignment, source_event_id, priority }`, `Event { schedule, recurrence, event_type }`, `Template { .. }`, `Simple`) whose variants carry only the payload that kind legitimately has — a `Task` variant has no field to put `event_type` in, a `Simple` variant carries nothing at all. This closed off a real class of bug (a flat `Option<String> event_type` on every kind, relying on `validate()` alone to reject it on a Task) and it's been in place since `e18a028`. CLAUDE.md's "Domain Models" section is stale — it still describes the pre-`ItemType` flat shape.

What's *not* type-safe yet is everything that still lives on the flat `Item` struct alongside `item_type`, where the field is actually kind-restricted only by runtime checks:

- **`complete: bool`** — `Item::validate()` rejects `complete == true` for `Simple` and `Event` at runtime; `Template` never exposes a complete concept in any UI/CLI/MCP path either. Only `Task` genuinely uses it. This is the single biggest remaining gap and the clearest win.
- **`parent_item_id: Option<String>`** — confirmed by reading the code (not assumed): `project_events/mod.rs` hardcodes `parent_item_id: None` on every Event create (Events are never children), and there's no path that creates a Template as a child either. But `project_simple_lists/handlers.rs` *does* support Simple-item nesting (`parent_item_id`, `has_children`, expandable-children fragments) — so this field is legitimately Task-**and**-Simple, not Task-only. Any kind can be a parent *target* (Task/Event/Template sub-items and Simple-under-Simple nesting all exist), but only Task and Simple items can themselves carry a non-null `parent_item_id`.
- **`series_id`** — `ItemSeries.item_type` is restricted to `Task`/`Event` only (`src/domain/item_series.rs`), so a materialized occurrence's `series_id` should only ever appear on those two kinds. Not yet independently verified against every write site — flagged for the Stage 0 audit below, not asserted as fact.
- **`google_event_id`/`calendar_subscription_id`** — every construction site I found (`src/service/calendar_sync.rs`) builds `ItemType::Event`. Likely Event-only in practice; also flagged for Stage 0, not asserted.

**One correction to the request that spawned this plan:** the request's own example — "an Event should be structurally incapable of carrying a due_date" — doesn't match this app's actual, currently-shipped business rules. I checked: `templates/project_events/new_page.html`/`detail_fields.html` render a live "Due date" input alongside the scheduled-window fields, `Item::validate()`'s own test suite has `validate_allows_event_with_all_scheduling_fields` asserting a due date on an Event is valid, and CLAUDE.md's "Events and template triggers" section describes Events as "the same struct [as Task], just scheduled_date-primary for display." Events legitimately carry both `due_date` and the scheduled window today. This plan **preserves that** — it does not silently tighten a real business rule while doing a type-safety refactor. If reducing Event's actual field set is wanted, that's a separate product decision, not a side effect of this refactor.

## Non-goals — explicit scope boundaries

These are the plan's actual load-bearing decisions, called out up front rather than buried in stage prose:

1. **Rust-domain-layer only. The wire format is untouched.** `model/src/main/smithy/project_item.smithy`'s `ProjectItem` resource keeps its one flat property bag (`dueDate`, `scheduledDate`, `itemType`, `eventType`, `points`, ... all as siblings, all optional) exactly as today. No Smithy change, no `task codegen` run, no change to `todo-server-sdk`/`todo-client`/`todo-typescript-client`, `todo-cli`, or `mcp-server`. Reason: a request over HTTP is fundamentally untyped-by-kind until parsed — there is no way to make "the wire payload can't contain `points` unless `itemType` is `TASK`" a *Rust* type-system property, because the wire payload was never a Rust type; it's JSON. Tightening the wire format is a real, independently-useful idea (it would let smithy-rs itself reject cross-kind fields at the deserialization boundary, similar to what `ItemType`-the-enum already gets client/server-side per CLAUDE.md's Smithy section) but it's a different, much larger project — it ripples into every generated SDK, `prl`, and the MCP server, and ships a breaking API change. Out of scope here.
2. **Storage stays one `items` SQLite table, unchanged schema.** No new migration to split kind-specific columns into per-kind tables. `row_to_item` (`src/storage/sqlite/mod.rs`) is already the single place that reconstructs a typed `ItemType` from flat nullable columns — it just needs to build richer per-kind structs instead of inline-variant payloads. A multi-table schema would trade one class of risk (a NULL column that shouldn't apply to this row) for another (JOIN complexity for every cross-kind query this app already has — `list_by_project`, `list_due`, the calendar's per-project merge, `all_projects_tasks`) for no Rust-type-safety benefit, since the table row is never itself a Rust value callers touch directly.
3. **No change to actual valid/invalid states.** Every combination that's legal today (per `Item::validate()` and every service-layer dispatch) stays legal; every combination that's illegal today stays illegal. This is a refactor of *how* the Rust type system represents already-decided business rules, not a re-decision of the rules themselves — see the Event/due_date correction above.
4. **`src/domain/item_series.rs` (`ItemSeries`) is not touched.** It has the exact same "runtime-only kind restriction" shape (`basis`/`template_item_id`/`assigned_to_user_id`/`points`/`priority` all Task-series-only, enforced in `service::item_series.rs`, not structurally) and would be a natural follow-up once this plan's pattern is proven — but it's a second, separate type with its own call sites and isn't required for this plan's goal.

## Design

### Stage 0 deliverable: a confirmed field-restriction table

Before any struct is renamed for real, Stage 0 must nail down, with a citation from the actual code (a `validate()` rule, a service-layer dispatch, or a web UI form/route), exactly which kinds legitimately read/write each currently-flat field. My research this session got most of the way there; treat the table below as the Stage 0 starting draft, not a finished answer — the "Confidence" column says which rows still need a dedicated grep-and-confirm pass before Stage 3+ locks in a struct shape:

| Field | Today's home | Kinds that legitimately use it | Confidence |
|---|---|---|---|
| `schedule` (due/scheduled window) | `ItemType` variant payload (already split) | Task, Event, Template | High |
| `recurrence` (pattern/basis/`due_offset_days`) | `ItemType` variant payload (already split) | Task, Event, Template | High |
| `team_assignment` (points/assignee) | `ItemType::Task` payload (already split) | Task only | High |
| `source_event_id` | `ItemType::Task` payload (already split) | Task only | High |
| `priority` | `ItemType::Task` payload (already split) | Task only | High |
| `event_type` | `ItemType::Event`/`Template` payload (already split) | Event, Template | High |
| `complete` | flat `Item` field | Task only | High — `validate()` rejects Simple/Event explicitly; Template has no complete-toggle UI/CLI/MCP path anywhere |
| `parent_item_id` (as *this item's own* parent, i.e. "am I a child") | flat `Item` field | Task, Simple | High — Event confirmed never-a-child (`project_events/mod.rs` hardcodes `None`); Template confirmed too: `service::templates::create_template` builds its `Item` via `Item::new_user_item` (defaults `parent_item_id: None`) and never assigns it anywhere in that file |
| `has_children` (computed, read-only) | flat `Item` field | universal — every kind can be a parent/container | High — confirmed all four: `project_tasks`/`project_simple_lists`/`project_templates` (`project_template_children_fragment` et al.) all render a children fragment; Events' sub-item routes are documented in CLAUDE.md's Events section |
| `depends_on_item_ids` | **not an `Item` field** — side table via `ItemDependencyRepo` | Task only, enforced in `service::item_dependencies.rs` | High — out of scope for the struct shape below since it isn't a domain-model field at all, but the Task-only runtime check becomes one of the "closed by construction" wins once only `TaskItem`'s construction path can even reach that validator meaningfully |
| `series_id` | flat `Item` field | Task, Event (mirrors `ItemSeries.item_type`'s own Task/Event restriction) | High — every non-test write site (`service::items.rs`/`team_items.rs`'s round-trip-on-update, `service::item_series.rs`'s `get_or_materialize_occurrence`) either round-trips an existing value or is reached only through `ItemSeries` construction, which is itself already restricted to `item_type: Task \| Event` |
| `google_event_id` / `calendar_subscription_id` | flat `Item` field | Event only | High — the only two non-test write sites in the whole codebase are both in `service::calendar_sync.rs`, and both immediately followed by (or preceded by) `item.item_type = ItemType::Event { .. }` in the same function. **New finding, not in the original draft:** four *read* sites — `current.google_event_id.is_some()` guards in `update_item`/`delete_item` (`src/service/items.rs:253,414`) and their `team_items.rs` twins (`:321,440`) — reject editing/deleting a calendar-imported item, and today run generically for every kind (they happen to only ever fire for Event rows in production, but the check itself doesn't know that). Once the field moves onto `EventItem` only, these four checks change shape from `current.google_event_id.is_some()` to `matches!(&current.item_type, ItemType::Event(e) if e.google_event_id.is_some())` — noted explicitly in Stage 6 below so it isn't rediscovered mid-stage. |

### Target shape (pending Stage 0 sign-off)

```rust
pub struct Item {
    pub id: String,
    pub user_id: Option<String>,
    pub project_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub has_children: bool,        // computed; meaningful for every kind
    pub item_type: ItemType,
}

pub enum ItemType {
    Task(TaskItem),
    Event(EventItem),
    Template(TemplateItem),
    Simple(SimpleItem),
}

pub struct TaskItem {
    pub parent_item_id: Option<String>,
    pub complete: bool,
    pub schedule: Schedule,
    pub recurrence: Recurrence,
    pub team_assignment: Option<TeamAssignment>,
    pub source_event_id: Option<String>,
    pub priority: Option<i32>,
    pub series_id: Option<String>,
}

pub struct EventItem {
    pub schedule: Schedule,
    pub recurrence: Recurrence,
    pub event_type: Option<String>,
    pub series_id: Option<String>,
    pub google_event_id: Option<String>,
    pub calendar_subscription_id: Option<String>,
}

pub struct TemplateItem {
    pub schedule: Schedule,
    pub recurrence: Recurrence,
    pub event_type: Option<String>,
}

pub struct SimpleItem {
    pub parent_item_id: Option<String>,
}
```

`Schedule`/`Recurrence`/`TeamAssignment` are unchanged — they're already exactly the right shape (shared sub-structs embedded wherever a kind legitimately has them). Named per-kind structs (`TaskItem`/`EventItem`/`TemplateItem`/`SimpleItem`) replace today's inline-fields-per-variant — this is what makes the shape *reusable* (a function can now take `&TaskItem` directly, not just destructure an `ItemType::Task { .. }` match arm) without changing what's structurally possible versus today's already-split payloads.

### Traits for generic code

The request specifically asked for a trait (or small set) so code genuinely needing to work across kinds doesn't regress into a giant match. Two, deliberately narrow:

```rust
pub trait HasSchedule {
    fn schedule(&self) -> &Schedule;
    fn recurrence(&self) -> &Recurrence;
}
impl HasSchedule for TaskItem { .. }
impl HasSchedule for EventItem { .. }
impl HasSchedule for TemplateItem { .. }
// SimpleItem: no impl — it genuinely cannot answer this, and that's the point.

pub trait Completable {
    fn complete(&self) -> bool;
}
impl Completable for TaskItem { .. }
// No other kind implements it.
```

**Important nuance — these traits are for *generic helper functions*, not a replacement for `Item`'s own delegation accessors.** `Item::due_date()`, `Item::scheduled_date()`, `Item::is_overdue()`, etc. already exist today as delegation methods (`self.item_type.schedule().and_then(|s| s.due_date)`) and almost every call site in the codebase already goes through them rather than touching `item_type` directly — that's exactly why the *previous* round of this split (payload-in-variants) was low-risk to land. Keep that pattern: `Item` keeps `pub fn due_date(&self) -> Option<DateTime<Utc>>` etc., now implemented via a match over `item_type` calling into `HasSchedule` where relevant. The trait is what a genuinely kind-blind helper (`list_filters.rs`'s date-based sort/filter code, `item_series.rs`'s virtual/materialized-occurrence merge, `main_calendar.rs`'s per-project fan-out) can write against — `fn is_overdue(x: &impl HasSchedule, now: DateTime<Utc>) -> bool` — instead of matching `ItemType` itself. Don't reach for the trait anywhere a plain `Item` accessor already does the job; the goal is one narrow generic surface for the handful of places that truly need it, not a parallel API to the whole struct.

**Deliberately no `set_complete()` on the trait, and no `Item::set_complete()` convenience method either.** A setter that silently no-ops (or panics) on `EventItem`/`SimpleItem`/`TemplateItem` would just relocate the exact runtime bug this plan exists to close — "something tried to complete a non-Task item and nothing stopped it" — one layer down, disguised as a type-safe-looking method call. Every call site that mutates `complete` must go through the concrete `ItemType::Task(task) => task.complete = value` match, the same way `set_source_event_id`/`set_points` already have to in today's test helpers (see `src/domain/item.rs`'s `#[cfg(test)] mod tests`' `set_points`/`set_priority`/`set_event_type` — this plan's non-test call sites follow that identical precedent). This is the actual mechanism by which "an Event can't be completed" becomes a compile-time fact instead of a `validate()` rule: there is no code path left that can even attempt it without first pattern-matching into `ItemType::Task`.

### Storage layer

- Schema unchanged (see Non-goals #2). `ITEM_SELECT`, `create`'s `INSERT`, `update`/`update_by_project`'s `UPDATE` in `src/storage/sqlite/items.rs` keep binding the same flat columns — they already call `item.due_date()`/`item.event_type()`/etc. (delegation methods) rather than touching `item_type` directly, so most of that file is unaffected. What changes: those delegation methods that move from "read a field that's on every variant's payload" to "read a field that's now only on `TaskItem`'s payload" — mechanical.
- `row_to_item` (`src/storage/sqlite/mod.rs`, around line 620) is the one function that must know the full column set and construct the right `ItemType::Task(TaskItem { .. })`/etc. — same responsibility as today, just building named structs.

### Service layer

- `CreateItemParams`/`UpdateItemParams` (`src/service/items.rs`) and their `team_items.rs` twins **stay flat, kind-agnostic DTOs** — they mirror the wire shape on purpose (see Non-goals #1) and that's still correct after this refactor; the input hasn't gotten more typed, only the internal representation has.
- `build_item_type` (`items.rs`) and its `team_items.rs` counterpart become the one place that turns "flat params + resolved kind" into the correctly-shaped `TaskItem`/`EventItem`/`TemplateItem`/`SimpleItem` — same responsibility as today, just constructing named structs instead of inline variant literals. This is also where the type system starts paying for itself: it becomes impossible for a future edit to this function to accidentally attach `team_assignment` to an `EventItem` literal, because `EventItem` has no such field to assign — today that mistake compiles fine and is caught only by `validate()` (or isn't, if nobody thought to test it).
- Every `if let ItemType::Task { priority, .. } = &mut edited.item_type` becomes `if let ItemType::Task(task) = &mut edited.item_type { task.priority = .. }` — same shape, mechanical rename. `is_pure_complete_toggle`, the activity-log completion hooks, and the offset/anchor sync helpers (`sync_offset_children`, `sync_source_event_tasks`, `copy_template_children`) are unaffected in logic, only in field-access syntax.
- `service::item_dependencies.rs`'s "only Task items can depend on other items" runtime check stays — it's still guarding the flat, wire-shaped input boundary (Non-goals #1) — but its *internal* plumbing (once it has a resolved `Item`) can now match into `ItemType::Task` once and work with a real `&TaskItem` rather than re-deriving "is this a Task" via `.kind()` at every step.

### Web UI

Lower risk than the file count suggests, because of something I confirmed by grepping `templates/`: only two files under `templates/` reference `item_type`/`ItemKind` at all (`macros.html`'s kind-radio-buttons partial, `project_item_series/row.html`'s badge), and neither touches the Rust enum shape — they consume a pre-formatted string. Every screen (`project_tasks`, `project_events`, `project_simple_lists`, `project_templates`, `project_item_series`) already funnels the raw `Item` through a `Row::from_item(&Item, ..)`-style conversion function before anything reaches Askama. That conversion layer is where the impact concentrates, not the templates themselves.

- Direct field access (`item.complete`, `item.parent_item_id`) needs updating at every read/write site — I counted 83 `.complete` reads/writes and 89 `.parent_item_id` reads/writes across the codebase (21 files for the latter). This is mechanical and compiler-guided: once `Item` no longer has these fields, `cargo build` fails at every site that needs fixing, the same "fix the Rust compile errors" workflow CLAUDE.md's own Key Workflows section already documents for Smithy-driven changes.
- Every screen already gates with a `require_task`/`require_event`/`require_simple`-style guard (e.g. `project_tasks::require_task`) before doing anything kind-specific. Once that guard passes today, the rest of the handler still calls generic `Option`-returning `Item` accessors for fields it *knows* must be present. As an optional, later-stage cleanup (not required for the split itself), each guard could return `(Item, TaskItem)`/etc. so the handler works against the concrete struct directly instead of unwrapping `Option`s that "can't actually be `None` here" — real ergonomic and safety upside, but purely additive polish, sequenced last and screen-by-screen so it never blocks the core migration.

### Non-breaking incremental delivery

`Item`/`ItemType` are referenced across storage, service, web_ui, and test fixtures in 50+ files. This cannot land as one PR — matches this repo's own precedent (`docs/archived/team-id-removal-plan.md`, `docs/archived/project-abstraction-plan.md`: both explicitly staged, one commit per stage, `cargo test` green after each).

**Recommended sequencing — grow the new shape inside the old one first, then move fields one at a time:**

1. Wrap today's inline-variant payloads into named structs with **zero field movement** (`ItemType::Task { schedule, .. }` → `ItemType::Task(TaskItem)`, where `TaskItem` initially has exactly the fields the `Task` variant has today). This alone touches every construction/match site once, but is a pure mechanical rename with no risk of getting a kind-restriction wrong, because nothing about *what's legal* changes in this stage.
2. Only after that lands and is verified, move one genuinely-kind-specific field at a time off `Item`'s envelope onto the right struct(s) — `complete` first (highest confidence, biggest win), then `parent_item_id`, then `series_id`/calendar-import fields last (lowest confidence, smallest blast radius if the Stage 0 audit turns out to be wrong about them).

Each field-move is independently shippable and independently revertable. If Stage 0's audit turns out to be wrong about a field (e.g. it turns out a Template *can* legitimately be a child in some path this session's research missed), that costs one stage's rework, not the whole plan — matching this repo's own "if a stage's implementation reveals the plan was wrong about something, fix this plan file itself before moving on" convention (`team-id-removal-plan.md`'s Workflow section).

A big-bang alternative (define the final shape once, fix every resulting compile error in one enormous PR) was considered and rejected: faster in theory, but a 150+ file diff is unreviewable and un-bisectable if something regresses, and it's inconsistent with every large migration this codebase has actually shipped.

## Workflow: commit and clear context between stages

Each stage below is its own work session, not a continuous one. After finishing a stage: run its verification, commit the stage on its own (reference the stage number in the commit message, e.g. "Stage 3: ..."), update the Stage status checklist below to mark it done, then clear context before starting the next stage.

- Each stage's description must stand alone: don't rely on the next session inferring intent from an earlier stage's reasoning, only from what's written here and what's visible in the code/git history at that point.
- Before starting a stage, re-read this whole plan file (not just that stage's section) plus the Stage status checklist to confirm what's actually merged — git log/diff is the source of truth over the checklist if they ever disagree.
- If a stage's implementation reveals the plan was wrong about something (the way Stage 0 turned up the four `google_event_id` guard-check sites, folded into Stage 6 above), fix this plan file itself before moving on — don't just fix it in code and leave the doc stale for whoever picks up the next stage.

This plan document is the source of truth. It originated in a Claude Code plan-mode session (`~/.claude/plans/splendid-wishing-sphinx.md`), then was copied here (`docs/item-kind-split-plan.md`) so it survives independently of that session, following `docs/archived/team-id-removal-plan.md`'s own precedent. Update this file in place as stages complete.

## Staged plan

Ship in order; each stage is its own PR/commit and leaves `cargo fmt`/`cargo test`/`task check` clean.

### Stage status

- [x] Stage 0 — Confirm the field-restriction table above (audit only, no code changes)
- [ ] Stage 1 — Wrap `ItemType`'s inline variants into named per-kind structs (no field movement)
- [ ] Stage 2 — Add `HasSchedule`/`Completable` traits; repoint the genuinely generic call sites onto them
- [ ] Stage 3 — Move `complete` off `Item` onto `TaskItem`
- [ ] Stage 4 — Move `parent_item_id` off `Item` onto `TaskItem`/`SimpleItem`
- [ ] Stage 5 — Move `series_id` onto `TaskItem`/`EventItem`
- [ ] Stage 6 — Move `google_event_id`/`calendar_subscription_id` onto `EventItem`
- [ ] Stage 7 — Optional per-screen web_ui ergonomic cleanup (`require_task` etc. return the concrete struct)
- [ ] Stage 8 — Remove now-provably-dead `Option`-returning delegation methods; update CLAUDE.md's Domain Models section (it's already stale re: the pre-`ItemType` shape, and will need a rewrite reflecting this plan's final shape)

### Stage 0 — Field-restriction audit

Pure research, no code changes. For every row in the target-shape table above marked below "High" confidence, produce one grep/read that confirms it (many already done this session — cite them). For every "Medium" row, do the work to raise it to High or correct it:

- `parent_item_id` + Template: grep every `create_project_template`/template-child construction site (`src/service/templates.rs`) to confirm no path ever sets a Template's own `parent_item_id`.
- `series_id`: grep every `repo.create`/`Item { series_id: .. }` construction site outside `get_or_materialize_occurrence` to confirm Task/Event are the only kinds it's ever set on, and cross-check against `ItemSeries.item_type`'s own restriction in `service::item_series.rs`.
- `google_event_id`/`calendar_subscription_id`: grep every write site (should be `service::calendar_sync.rs` only) to confirm Event-only.
- Double check `has_children`: confirm every kind's screen (including Simple, Template) genuinely supports a child count/expand affordance, not just Task.

**Verify:** no code changes to verify; the deliverable is this plan file updated in place with the table's "Confidence" column raised to High everywhere (or corrected, with the correction noted the way `team-id-removal-plan.md`'s own "Stage N as implemented — corrections" sections do).

**Stage 0 as implemented:** all four "Medium" rows raised to High, no corrections needed — the original draft's guesses all held up:
- `parent_item_id`+Template: confirmed via `service::templates::create_template`, which builds its `Item` through `Item::new_user_item` (defaults `parent_item_id: None`) and never assigns the field.
- `series_id`: confirmed Task/Event-only — every non-test write site either round-trips an existing value or is reached only via `ItemSeries` construction, itself already `item_type`-restricted to Task/Event.
- `google_event_id`/`calendar_subscription_id`: confirmed Event-only — both fields are written in exactly one production file (`service::calendar_sync.rs`), always alongside `item.item_type = ItemType::Event { .. }`.
- `has_children`: confirmed universal — Task, Event, Template, and Simple all have a working children-fragment/expand affordance in the web UI.

**One new finding, not anticipated by the original draft:** `google_event_id`'s read side has four call sites — `current.google_event_id.is_some()` guards in `update_item`/`delete_item` (`src/service/items.rs:253,414`) and their `team_items.rs` twins (`:321,440`) that reject editing/deleting a calendar-imported item. These run generically today (any kind), not just for Event rows — harmless in practice since the field is only ever set on Events, but Stage 6 must update these four sites' shape, not just the field's home. Folded into Stage 6's own section below so it isn't rediscovered mid-stage.

### Stage 1 — Wrap `ItemType` variants into named structs, zero field movement

- `src/domain/item.rs`: define `TaskItem`/`EventItem`/`TemplateItem`/`SimpleItem` with exactly today's per-variant field sets (no field moves yet); change `ItemType` to `Task(TaskItem)`/`Event(EventItem)`/`Template(TemplateItem)`/`Simple(SimpleItem)`. Keep every existing `Item::due_date()`/`event_type()`/etc. delegation method working (just re-point the match arms at `.0.schedule` instead of destructured field names).
- Fix every resulting compile error across `src/storage/sqlite/`, `src/service/`, `src/web_ui/` — purely mechanical (`ItemType::Task { schedule, .. }` patterns become `ItemType::Task(TaskItem { schedule, .. })` or `ItemType::Task(task)` + `task.schedule`).
- **Verify:** `cargo build`, `cargo test` clean, no behavior change (this stage cannot change behavior — it moves zero fields and adds zero restrictions). Diff the SQL and Askama layers to confirm they're untouched (per the Web UI section above, they should need zero edits at this stage).

### Stage 2 — `HasSchedule`/`Completable` traits

- `src/domain/item.rs`: add the two traits from the Design section, implement `HasSchedule` for `TaskItem`/`EventItem`/`TemplateItem`, `Completable` for `TaskItem`.
- Repoint the handful of genuinely kind-blind generic helpers onto the trait instead of an `ItemType` match — candidates to check: `src/web_ui/list_filters.rs`'s date-based filter/sort predicates, `src/service/item_series.rs`'s virtual/materialized-occurrence merge (`ProjectOccurrence`), `src/web_ui/main_calendar.rs`'s per-project fan-out. Only repoint a call site if it's actually kind-blind today (matches on 2+ variants doing the same thing) — leave alone anything that already legitimately branches per-kind for different logic.
- **Verify:** `cargo test`; this stage is additive (new traits, optional call-site simplification) so a clean build with no behavior change is the whole bar.

### Stage 3 — Move `complete` onto `TaskItem`

- `src/domain/item.rs`: remove `complete: bool` from `Item`; add it to `TaskItem`. `Item::complete() -> bool` becomes a delegation method (`false` for every non-Task kind, matching today's implicit behavior since `validate()` already forbids `true` on those kinds). No `Item::set_complete()` — see the Design section's nuance; every write site must match into `ItemType::Task`.
- Fix every compile error this produces (~83 read sites, ~16 write sites, per this session's grep) across `src/service/items.rs`, `team_items.rs`, `project_items.rs`, `reminders.rs`, `item_dependencies.rs`, `src/web_ui/list_filters.rs`, and test fixtures throughout.
- `Item::validate()`'s existing "simple items cannot be marked complete" / "events cannot be marked complete" checks become dead code for any *internally constructed* `Item` (compiler now guarantees it) but must stay for the *wire-input* boundary (Non-goals #1) — don't delete them, just note in a comment that they're now solely guarding untyped input, not internal state.
- **Verify:** full `cargo test`. Specifically re-run `src/domain/item.rs`'s own `validate_rejects_complete_simple_item`/`validate_rejects_complete_event` tests unchanged (they exercise the wire-input boundary, still relevant) plus new tests asserting `Item::complete()` returns `false` and there is no way to construct a completed `EventItem`/`SimpleItem`/`TemplateItem` at all (a type-level assertion, not a runtime one — e.g. via a doc-comment/compile-fail test rather than a `#[test]`, since "this doesn't compile" isn't itself a runtime-testable fact).

### Stage 4 — Move `parent_item_id` onto `TaskItem`/`SimpleItem`

- Same shape as Stage 3: remove from `Item`, add to both `TaskItem` and `SimpleItem`, `Item::parent_item_id() -> Option<&str>` delegates (returning `None` for `Event`/`Template`, matching confirmed-in-Stage-0 behavior).
- Fix ~89 call sites across `src/service/items.rs`/`team_items.rs`/`project_items.rs`, `src/web_ui/project_tasks/`, `project_simple_lists/`, `project_events/` (which reads it to confirm it's always `None`), `project_templates/`.
- **Verify:** full `cargo test`; specifically the item-nesting tests in `project_simple_lists` and `project_tasks` (sibling-group queries, expandable-children fragments) since those are the two kinds actually exercising this field's write path.

### Stage 5 — Move `series_id` onto `TaskItem`/`EventItem`

- Same shape. Fix call sites in `src/service/item_series.rs` (`get_or_materialize_occurrence`), `src/storage/sqlite/items.rs`'s series-scoped queries, `src/web_ui/project_item_series/`.
- **Verify:** `cargo test`, particularly `item_series`'s materialization tests and the sqlite-level `series_id_round_trips_through_create_and_update` test already in `src/storage/sqlite/items.rs`.

### Stage 6 — Move `google_event_id`/`calendar_subscription_id` onto `EventItem`

- Same shape. Fix call sites in `src/service/calendar_sync.rs`, `src/web_ui/assigned_items.rs`/wherever else reads these for display.
- **Confirmed in Stage 0's audit:** four generic guard checks — `current.google_event_id.is_some()` in `update_item`/`delete_item` (`src/service/items.rs:253,414`) and their `team_items.rs` twins (`:321,440`) — reject editing/deleting a calendar-imported item regardless of kind today. Update each to `matches!(&current.item_type, ItemType::Event(e) if e.google_event_id.is_some())`. This is a real (small) behavior consideration to double check: today the check is kind-blind, so if `google_event_id` ever ends up set on a non-Event row for any reason not caught by this audit (shouldn't happen post-migration since the field won't exist there, but worth a second look at any in-flight/legacy data), the guard silently stops applying to it. Since the field structurally can't exist on non-Event rows after this stage, that's fine going forward — flagging only so the four call sites aren't missed.
- **Verify:** `cargo test`, plus the existing regression tests in `src/storage/sqlite/items.rs` (`list_due_by_project_does_not_panic_and_round_trips_google_event_id`, etc. — these already exist because this exact field caused a live-data panic once before; keep them passing, don't weaken them), plus new/updated tests for the four guard checks above confirming they still reject edits to an imported Event.

### Stage 7 — Optional per-screen web_ui ergonomic cleanup

- Per screen, change `require_task`/`require_event`/`require_simple` (currently `fn require_task(item: Item) -> Result<Item, ItemError>`) to also hand back the unwrapped concrete struct, e.g. `fn require_task(item: Item) -> Result<(Item, TaskItem), ItemError>`, so the rest of that screen's handler code can work against `&TaskItem` directly instead of continuing to call `Option`-returning `Item` accessors it already knows will be `Some`.
- Purely additive ergonomics — sequence one screen per commit (`project_tasks`, then `project_events`, then `project_simple_lists`, then `project_templates`'s children, then `project_item_series`), each independently shippable, each skippable if it doesn't turn out to read better in practice.
- **Verify:** `cargo test` per screen; no behavior change expected, this is a readability/API-ergonomics pass only.

### Stage 8 — Cleanup + CLAUDE.md update

- Sweep for any `Option`-returning `Item` delegation method that's now provably redundant given Stage 7's direct-struct-access convention (only if Stage 7 shipped for that screen) — don't remove any that are still genuinely used by cross-kind generic code from Stage 2.
- Rewrite CLAUDE.md's "Domain Models" section, which currently describes the pre-`ItemType` flat shape (stale even before this plan) — bring it in line with the actual final shape (`Item` envelope + `ItemType(TaskItem | EventItem | TemplateItem | SimpleItem)` + `HasSchedule`/`Completable` traits), following this doc's own precedent of explaining *why*, not just *what*.
- **Verify:** full `cargo test` + `task check`; read the new CLAUDE.md section back against the actual code once more before committing it, the same "verify infra config syntax against primary sources" discipline this user has flagged before in other contexts — don't let the doc drift from the code the way the pre-existing Domain Models section already had.

## Verification approach (every stage)

- `cargo fmt` (plain, no path args — per CLAUDE.md's Git section) before every commit.
- `cargo build` + `cargo test` must be clean at the end of every stage, not just the end of the plan.
- `task check` before considering a stage done.
- No Playwright/browser click-through per CLAUDE.md's explicit instruction — verify web_ui changes by reading the handler/template diff carefully, and say so explicitly if live-in-browser behavior can't be confirmed that way. Stages 3–4 in particular touch enough web_ui call sites that a careful read of each changed handler (not just "it compiles") matters.

## Critical files

- `src/domain/item.rs` — the type definitions themselves; every stage's primary edit site.
- `src/storage/sqlite/mod.rs` (`row_to_item`) and `src/storage/sqlite/items.rs` — the single reconstruction/flattening boundary between DB rows and the new types.
- `src/service/items.rs` / `src/service/team_items.rs` (`build_item_type` and its twin) — the single boundary between flat wire-shaped params and the new types.
- `src/service/item_series.rs`, `src/service/calendar_sync.rs`, `src/service/item_dependencies.rs` — the three "kind-restricted side feature" modules whose fields move in Stages 5–6.
- `src/web_ui/project_tasks/`, `project_events/`, `project_simple_lists/`, `project_templates/`, `project_item_series/` — one representative pattern (`require_*` guard → generic `Item` accessors) repeated per screen; Stage 4/7 touch all five the same way.
- `docs/archived/team-id-removal-plan.md` / `docs/archived/project-abstraction-plan.md` — the house style this plan follows (stage-per-commit, corrections-in-place, stage status checklist) and worth re-reading before Stage 0 for calibration on how much detail each stage write-up should carry.
