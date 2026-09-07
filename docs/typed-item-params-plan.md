# Typed item params

## Context

`Item` was split into `TaskItem`/`EventItem`/`TemplateItem`/`SimpleItem` by
`docs/archived/item-kind-split-plan.md` (Stages 0–8, complete). That split stopped at the domain
layer by explicit decision. Its Non-goals #1 and #2 ruled the wire format and the SQLite schema
out of scope, and its Service layer section ruled that the params structs
**"stay flat, kind-agnostic DTOs — they mirror the wire shape on purpose."**

This plan revisits only the third of those. The wire (Non-goal #1) and the schema (Non-goal #2)
stay exactly as they are, and both decisions still look right:

- **Schema.** A per-kind table split trades NULL-column risk for JOIN complexity across every
  cross-kind query the app already has (`list_by_project`, `list_due_by_project`, both calendars,
  `all_projects_tasks`), and would make seven tables' `item_id` foreign keys polymorphic —
  `comments`, `attachments`, `reminders`, `item_dependencies`, `activity_log`,
  `item_occurrences`, `series_child_occurrences`, across ~87 query sites. `parent_item_id` is
  heterogeneous too (Task children under Events via the template trigger, Task children under
  Task occurrences via series sub-items, Template under Template), so a self-FK would become a
  cross-table one. No Rust-type-safety benefit, since a table row is never a value callers touch.
- **Wire.** Tightening it means a breaking API change rippling through every generated SDK,
  `prl`, and the MCP server. If it is ever done, a Smithy `union` for the per-kind detail is the
  cheaper shape than four resources (5 item operations would otherwise become 20) — and it should
  come *after* this plan, which is what will establish what the per-kind shapes actually are.
  Nothing in this plan touches `model/`, so no `task codegen` run is needed at any stage.

### Why the "mirrors the wire shape" reasoning doesn't hold for the service layer

It is correct about the wire, and it over-generalized from there. Counting the actual callers:

| Struct | Construction sites | Genuinely untyped input |
|---|---|---|
| `CreateProjectItemParams` | 21 | **1** — `json_api/project_items.rs` |
| `UpdateProjectItemParams` | 26 | **1** — `json_api/project_items.rs` |

Everything else passes a **literal** `ItemKind::Task`/`Event`/`Simple`, or is `service/import.rs`
(kind from a CSV column) or `service/item_series.rs` (kind from `series.item_type`, Task or Event
only). So the flat DTO is an honest representation of its input for 1 caller in 21, and a
19-field form that the other 20 fill largely by omission.

`#[derive(Default)]` plus `..Default::default()` already softens the typing burden at most sites.
The burden was never the real cost. Three things are:

1. **Three parallel flat structs, transcribed field by field.** `create_project_item` copies 19
   fields into `CreateTeamItemParams` and 17 into `CreateItemParams`; `update_project_item` does
   the same twice more. That is ~80 lines of pure transcription which every new field must be
   threaded through, and where **omitting a field is a silent behavior change, not a compile
   error**. This is already load-bearing — `web_ui/project_templates/handlers.rs` carries a
   comment reading *"Dropped by `create_project_item` on the personal branch (`CreateItemParams`
   has no slot for it) — harmless to always pass through"*, which is a documented silent drop.
2. **`Default` hides which fields are required for which kind.**
   `CreateProjectItemParams::default()` is a Task with no project and no name. Nothing in the
   type says `event_type` is meaningless on a Task, or that `points` is Task-and-team-only.
3. **Direct-overwrite semantics are invisible.** `priority`, `event_type` and `due_offset_days`
   are cleared by omission on update, so every caller must round-trip a value it did not intend
   to touch. That convention is currently documented in four separate doc comments and enforced
   nowhere.

### What becomes unrepresentable

Seven classes of input that are today either silently dropped or caught by a runtime check:

| Input | Today | After |
|---|---|---|
| `complete` on Simple / Event | `Item::validate()` rejects | no field on `NewSimple`/`NewEvent` |
| `points` / `assigned_to_user_id` on non-Task | silently dropped | no field |
| `priority` on non-Task | silently dropped | no field |
| `event_type` on Task / Simple | silently dropped | no field |
| `parent_item_id` on Event | silently dropped | no field |
| `source_event_id` on non-Task | silently dropped | no field |
| `series_id` on Simple / Template | silently dropped | no field |

Plus one that an enum makes structural rather than a check: `Item::validate()`'s
*"an item cannot both have a parent and reference an event"* becomes `TaskAnchor::Parent(_) |
TaskAnchor::SourceEvent(_)`, since a Task has exactly one anchor source by construction.

Checks that deliberately **stay** runtime, so this plan does not overpromise: name/description
length, `priority` in 1–4, `scheduled_end_date >= scheduled_date`, points-on-a-child,
`due_offset_days` non-positive, and every permission/role gate. Newtypes (`Priority`,
`DaysBefore`) could absorb two of those and are explicitly out of scope — they would touch the
domain layer, which this plan does not.

## Design

A new `src/service/item_input.rs`, shaped to **mirror `ItemType`** — these types are the input
counterpart of the domain enum they construct, which is what lets `build_item_type` collapse.

```rust
/// The envelope every kind carries — mirrors `Item`'s own envelope.
pub struct NewItem {
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub timezone_offset_minutes: Option<i32>,
    pub kind: NewItemKind,
}

pub enum NewItemKind {
    Task(NewTask),
    Event(NewEvent),
    Simple(NewSimple),
    Template(NewTemplate),
}

/// Exactly one anchor source — replaces `validate()`'s parent/source_event exclusion check.
pub enum TaskAnchor {
    None,
    Parent(String),
    SourceEvent(String),
}

pub struct NewTask {
    pub anchor: TaskAnchor,
    pub schedule: ScheduleInput,
    pub due_offset_days: Option<i32>,
    pub priority: Option<i32>,
    pub complete: bool,
    /// Team-backed projects only; the personal branch rejects rather than drops it.
    pub assignment: Option<AssignmentInput>,
    /// Internal-only — set exclusively by `item_series::get_or_materialize_occurrence`.
    pub series_id: Option<String>,
}

pub struct NewEvent {
    pub schedule: ScheduleInput,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
    pub series_id: Option<String>,
}

pub struct NewSimple {
    pub parent_item_id: Option<String>,
}

pub struct NewTemplate {
    pub parent_item_id: Option<String>,
    pub schedule: ScheduleInput,
    pub event_type: Option<String>,
    pub due_offset_days: Option<i32>,
}
```

`ScheduleInput` mirrors `Schedule` (`due_date`/`has_due_time`/`scheduled_date`/
`has_scheduled_time`/`scheduled_end_date`/`has_end_time`), so the split date+time control
convention described in root CLAUDE.md's Scheduled start/end section carries over unchanged.
`EditItem`/`EditItemKind` are the update counterparts, adding the update-only
`depends_on_item_ids` to the envelope (`None` = leave alone, per its existing convention).

**`google_event_id`/`calendar_subscription_id` get no input field at all**, matching today:
`service::calendar_sync` writes them by building an `Item` directly rather than going through
this funnel, and `build_item_type` already hardcodes both to `None`.

**The template-child kind coercion.** `items::create_item` currently coerces a child of a
Template to `ItemKind::Template`. `web_ui/project_templates/handlers.rs` relies on it — it
creates template children by passing the default kind. Under typed params that path passes
`NewItemKind::Template` explicitly instead, which is what it means. The coercion itself stays for
the wire path, where the caller genuinely may not know the parent's kind.

## Staging

Every stage leaves the **whole workspace** — `todo-cli` included — building and testing green,
per this repo's convention (`docs/archived/team-id-removal-plan.md`,
`docs/archived/project-abstraction-plan.md`). Baseline is 655 test functions in `src`.

1. **The input types plus a conversion shim.** Add `src/service/item_input.rs` and
   `impl From<NewItem> for CreateProjectItemParams` / `From<EditItem> for
   UpdateProjectItemParams`, with `create_project_item_typed`/`update_project_item_typed`
   delegating through it. Purely additive — no existing caller changes. Unit tests that each
   variant converts to the flat params the old callers would have built by hand. **Done**
   (662 tests passing, up from a 654 baseline).

   Two deviations from the design above:

   - **The shims are named `create_item_typed`/`update_item_typed`**, not
     `create_project_item_typed`/`update_project_item_typed`. They live in
     `service::project_items` already, so the module name carried the `project_item` half; at
     Stage 8 they take over the plain names.
   - **`EditItem` carries `depends_on_item_ids` on the envelope, not on `EditTask`**, despite
     dependencies being Task-only (root CLAUDE.md's Item dependencies section). Clearing must
     stay possible for an item whose kind has since changed — that is the *only* way rows on a
     no-longer-Task item can be removed — so putting the field inside the Task variant would
     make an existing, deliberate escape hatch unrepresentable.
2. **Simple, end to end.** Migrate `web_ui/project_simple_lists/` (3 create, 4 update sites).
   Smallest payload — `NewSimple` is one field against the flat struct's 19 — and a
   self-contained screen, so it proves the pattern cheaply. **Done** (no test-count change —
   this stage rewrites call sites, it adds no behavior).

   Two things worth knowing before Stages 3–5 repeat the pattern:

   - **A handler that read `params.parent_item_id` back had to change.** `create_item_form`
     used it to decide which scope to re-render; a `NewItem`'s parent lives inside its kind
     payload, so it now reads `non_empty(&form.parent_item_id)` from the form directly — the
     same expression the builder itself uses. Expect one of these per screen that re-renders
     conditionally.
   - **`TaskAnchor`, `NewItemKind` and `EditItemKind` carry a transient
     `#[allow(dead_code)]`.** Only the `Simple` arms have callers until Stages 3–5 land.
     Remove the attribute as each kind's stage lands; it is gone entirely by Stage 5.
3. **Event.** `web_ui/project_events/` (4 create, 2 update) and `all_projects_events.rs`.
   **Done** (662 tests passing — unchanged, as in Stage 2; this rewrites call sites, it adds no
   behavior).

   Three deviations from the design above:

   - **`all_projects_events.rs` needed nothing.** It has no create or update path at all — it's
     a read-only cross-project list. The stage line above overcounted by assuming it mirrored
     `all_projects_tasks.rs`.
   - **Two of the four "create" sites build a `NewTask`, not a `NewEvent`.** Both
     `create_project_event_child_form` and the series-occurrence child form create a *linked
     task* anchored on the event via `sourceEventId` — an Event can never have structural
     children (root CLAUDE.md's Events section), so its "children" are Tasks pointing back at
     it. They are the first non-test users of `TaskAnchor::SourceEvent`, which is exactly the
     shape the type exists for: `..Default::default()` on `NewTask` can no longer accidentally
     also set a `parentItemId`.
   - **`has_*_time` collapsed from `Option<bool>` to `Schedule`'s plain `bool`** via a local
     `has_time` helper. Verified lossless first: every consumer reads the field through
     `unwrap_or(false)` (`service::items` and `service::team_items`, create and update paths
     alike), so `None` and `Some(false)` were never distinguishable.

   The three transient `#[allow(dead_code)]` attributes moved from whole enums onto the four
   variants that genuinely still have no non-test caller — `TaskAnchor::Parent`,
   `NewItemKind::Template`, `EditItemKind::{Task, Template}`. Warning count is back to the
   repo's pre-existing 11.
4. **Template.** `web_ui/project_templates/`, including the explicit `NewItemKind::Template`
   above. `service/templates.rs` has its own `Create*TemplateParams` family, already per-kind;
   audit whether it should fold into `NewTemplate` or stay separate. **Done** (665 tests
   passing, up from Stage 3's 662 — this stage fixes behavior, so unlike Stages 2–3 it adds
   tests).

   **The audit's answer: `service/templates.rs` stays separate.** Its functions build an `Item`
   and call `repo.create`/`repo.update` directly, deliberately never entering the
   `create_item`/`create_project_item` funnel — that bypass is precisely what lets the funnel
   carry a guard against minting library templates at all. Folding them into `NewTemplate`
   would mean routing template creation through the one funnel designed to reject it, which is
   a different (and much larger) change than this plan's.

   Three deviations from the design above:

   - **`NewItemKind::Template` was unconstructible as designed, and the guard had to narrow
     before it could exist.** All four of `items::create_item`/`update_item`,
     `team_items::create_team_item`/`update_team_item` rejected `item_type: Some(Template)`
     outright, so the conversion's `item_type: Some(new.kind.kind())` would have been rejected
     at every call site. The guard now rejects only a Template whose parent is not itself a
     Template (shared helper `items::require_template_has_template_parent`). That is a strict
     narrowing — the accepted-request set grows by exactly one shape, "explicit Template under
     a Template parent", which `create_item`'s long-standing parent-coercion already produced
     from the equivalent request with `item_type` omitted. Confirmed with the user before
     writing it; the alternative (delete `NewItemKind::Template`, treat template children as
     Tasks) was rejected because `copy_children_as_template` mints Template-typed children, so
     hand-added and saved-from-item children would have permanently disagreed.

   - **Two live defects fell out of the audit, both fixed here with regression tests.** Neither
     was in the plan's scope; both are the exact silent-kind class this plan exists to close.
     (1) `update_item` had no parent-coercion and `project_templates/handlers.rs` passed
     `item_type: Some(ItemKind::Task)`, so **editing a template child rewrote its kind to
     Task** — proved with a probe before fixing (`persisted kind = Task`), and the `UPDATE`
     statement does write `item_type`. (2) `create_team_item` had no coercion either and
     rejected Template, so **a template child on a team-backed project was created as a Task
     from birth**. Blast radius was contained — the library query is root-only and
     `copy_template_children` walks `list_children` kind-agnostically — which is why it went
     unnoticed. Both paths now coerce.

   - **`NewTemplate`/`EditTemplate` lost `Default` and their `parent_item_id` is a plain
     `String`.** A root template is `service::templates`' business, so an unparented
     `NewTemplate` is a request the narrowed guard rejects; making it unconstructable is
     cheaper than a runtime error. Only two call sites build these, so the lost `..Default`
     costs nothing.

   This is the first stage needing a **root CLAUDE.md** edit — the Events section stated the
   old guard as an invariant, and the Touch-Point Checklist requires the owning section be
   updated when one changes. Stages 1–3 genuinely needed none. The `#[allow(dead_code)]` on
   `NewItemKind::Template`/`EditItemKind::Template` is gone; `TaskAnchor::Parent` and
   `EditItemKind::Task` still carry theirs until Stage 5.
5. **Task.** The largest: `web_ui/project_tasks/` (6 create, 6 update), both calendars,
   `all_projects_tasks.rs`, `service/activity_log.rs`. **Done** (671 tests passing, up from
   Stage 4's 665 — the six new ones cover the shared round-trip constructor and the one
   rejection this stage had to relocate; the call-site rewrites themselves add no behavior).

   The three remaining `#[allow(dead_code)]` predictions held: `TaskAnchor::Parent` and
   `EditItemKind::Task` both gained real constructors here, and `src/service/item_input.rs`
   now carries none at all, exactly as Stage 2 said it would by this point.

   Four deviations from the design above:

   - **A shared `EditTask::from_item(&Item)`, which the design never called for.** Five of this
     stage's sites are "change one field, round-trip the other eighteen": the three completion
     toggles (both calendars and `all_projects_tasks`), `identity_params` behind the four batch
     actions, `reparent_params`, and `activity_log`'s undo-reopen. Each had its own hand-written
     nineteen-line transcription of the same `Item`, which is the exact silent-drop hazard in
     this plan's Context section reproduced *inside* a single screen. One constructor on
     `EditTask` (plus `TaskAnchor::from_item`) replaces all of them. It reads through `Item`'s
     `Option`-returning delegation, so it is only correct for an already-`require_task`'d item —
     stated on the function, since the type can't say it.
   - **`reparent_edit` became fallible.** `TaskAnchor` cannot express "parent *and* source
     event", which the old flat helper could build and hand to `Item::validate()` to reject one
     layer down. It is genuinely reachable: an event-linked task is top-level (the anchors are
     mutually exclusive), so it appears in the Move dialog's sibling list and can be
     subordinated. The choice was to re-raise the same rejection where the request is built or
     to silently unlink the event; rejecting is what the code already did, so only the layer
     moved. Message and behavior are unchanged, with a regression test for each of the three
     move shapes.
   - **The two calendar toggles gained a `require_task` guard.** Their flat params passed
     `item_type: Some(current.kind())`, which read as kind-agnostic but never saw anything but a
     Task — `calendar_row` leaves an Event's `complete_url` as `None`, so no Event checkbox
     points at these routes. A crafted `POST` naming an Event did reach them and fell through to
     `Item::validate()`'s "events cannot be marked complete". Under typed params, defaulting to
     the Task variant would instead have silently *rewritten* the row's kind — the Stage 4
     defect class again — so the check moved to the front of the handler. A crafted request now
     gets `NotFound` instead of `Invalid`; nothing reachable through the UI changes.
     `all_projects_tasks`' toggle got the same guard, where its own doc comment already claimed
     it was Task-only.
   - **`event_type` round-tripping disappeared from four sites.** Each passed
     `event_type: current.event_type()` to honor the direct-overwrite convention, on an item
     that structurally cannot have one — the call always returned `None`. `EditTask` has no such
     field, so the lines are gone rather than defaulted.

   **No root CLAUDE.md edit**, unlike Stage 4. Nothing here changes a stated invariant: the
   parent/source-event exclusion is still `Item::validate()`'s rule and still enforced there for
   every other writer; the calendar guard is a hardening of an undocumented handler. Two stale
   references to the long-collapsed `tasks`/`team_tasks` modules were fixed in passing, since
   the comments carrying them were being rewritten anyway.
6. **Internal service callers.** `service/item_series.rs` (3 sites, Task-or-Event from
   `series.item_type`) and `service/import.rs` (kind from a CSV column). Both are the
   "dynamic kind" shape the wire boundary will also need, so doing them here de-risks Stage 7.
   **Done** (675 tests passing, up from Stage 5's 671).

   **Four files still name the flat structs**, and only one of them still *builds* one as
   input: `json_api/project_items.rs`, which is Stage 7 and is the single caller the Context
   section's table identified as genuinely untyped. The other three are the structs' own
   definitions and conversions (`service/project_items.rs`, `service/item_input.rs`) and two
   Stage 5 test assertions on what a conversion produces. The migration's premise held — 20 of
   21 create sites and 25 of 26 update sites knew their kind statically.

   The stage line above said "3 sites" for `item_series`; it is 2 — the occurrence and the
   sub-item. The third, `create_project_task_series_occurrence_child_form`, lives in
   `web_ui/project_tasks/` and went with Stage 5.

   Three deviations from the design above:

   - **The kind dispatch needed a fourth arm neither dynamic site can reach.**
     `get_or_materialize_occurrence` matches on `series.item_type`, which
     `validate_series_item_type` restricts to Task/Event and which is immutable after creation
     — but a `match` on `ItemKind` still has to answer for Simple and Template. It returns
     `Invalid("cannot materialize an occurrence of a {kind} series")` rather than defaulting to
     Task, so a corrupt row fails loudly instead of silently materializing the wrong kind.
     Tested.
   - **Two more rejections relocated, both with byte-identical text.** `ItemError::Invalid` is
     `#[error("{0}")]`, so a per-row import failure already reported the bare message, which is
     what made this checkable. (1) A CSV row carrying both `parentItemId` and `sourceEventId` —
     `TaskAnchor` cannot hold both, so the rejection moves up from `Item::validate()`, exactly
     as it did for `reparent_edit` in Stage 5. (2) A root `TEMPLATE` row — `NewTemplate`'s
     parent is a non-optional `String`, so the unparented case is unconstructable and
     `require_template_has_template_parent`'s wording is raised in the builder instead. Root
     CLAUDE.md's CSV import section already documented both outcomes; only the layer moved.
   - **Cross-kind CSV columns still drop silently, deliberately, and this is now pinned by a
     test.** `points` on an `EVENT` row, `eventType` on a `TASK` row: each has no field on its
     variant and vanishes exactly as it vanished in `build_item_type` one layer down. Rejecting
     instead is the better contract, and import's per-row error channel is the natural place
     for it — but that is precisely the Stage 7 decision below, and making it unilaterally for
     CSV while the JSON API still drops would be worse than either answer consistently applied.
     `import_project_items_still_drops_cross_kind_columns` exists so Stage 7 changes it
     deliberately rather than by accident.

   **No root CLAUDE.md edit**, as in Stage 5: no stated invariant changed, and the CSV import
   section's account of what a `TEMPLATE` row may do is still accurate.
7. **The wire boundary.** `json_api/project_items.rs` gets an explicit
   `try_into_new_item()`/`try_into_edit_item()`. **Done** (683 tests passing, up from Stage 6's
   675).

   **The decision: reject.** Put to the user with the alternatives and the measured blast
   radius, and taken deliberately. A cross-kind field now fails with the API's ordinary client
   error naming the field and the kind, instead of being discarded behind a success response.
   Recorded in root CLAUDE.md under a new "Cross-kind fields at the boundary" heading, which
   the Events, Points and CSV import sections all cross-reference — this is the second stage
   after Stage 4 to need a root CLAUDE.md edit, and for the same reason: an invariant changed.

   Five deviations from the design above:

   - **The guards live in `service::item_input`, not in `json_api`.** The plan named only the
     wire, but `service::import` is the same shape one surface over, and Stage 6 had pinned its
     silent drop with a test explicitly so that this decision could flip it deliberately. Two
     surfaces answering the same question differently would have been the worst of the three
     options on offer, so the rejection helpers are shared and CSV import rejects too — per
     row, naming the column and the kind, with the rest of the file still importing. The wire's
     camelCase field names double as the PRL format's column names, so one set of strings
     serves both.
   - **A cross-kind boolean carrying `false` is accepted and ignored.** Found by reading the
     MCP tool schema rather than by reasoning about it: `complete` is in `update_item`'s
     `required` list, and `@required` on `UpdateProjectItem` itself, so a caller renaming an
     Event has no way *not* to send it. Rejecting `complete: false` would have made editing any
     non-Task impossible through the MCP server. `false` discards nothing — `Some(false)` and
     `None` were already indistinguishable to every consumer of these fields — so `reject_flag`
     rejects only `true`, and `complete: true` on a non-Task does fail.
   - **Resolving an omitted `itemType` on update needs a read, and that read is gated behind
     the membership check.** `EditItemKind` has to *be* some kind; when the request doesn't say,
     only the stored item knows. Reading first and rejecting afterwards would let a non-member
     distinguish "this item exists in that project" (a cross-kind field error) from "it
     doesn't" (not found) by deliberately sending a bad field. `update_item_typed` re-checks
     membership anyway, so the check in the handler is redundant for authorization and
     load-bearing purely for that ordering. A request that states its `itemType` pays nothing.
   - **`prl items done` gained a client-side kind guard**, which the plan never mentioned
     because the bug it fixes only became visible while measuring the blast radius. The command
     read the item, sent every field back plus `complete: true`, and printed "marked … complete"
     — for any kind. On an Event, Simple item or Template the server dropped the flag (no
     non-Task payload has a completion field), nothing changed, and the user was told it had.
     The server now rejects that request, so the guard turns a bare API error into a sentence
     naming the kind, using the `itemType` off the fetch it already makes.
   - **The Simple rejections are the widest and were worth checking twice.** A Simple item is a
     bare checkable name, so `dueDate`, `scheduledDate`, `scheduledEndDate`, the three
     `has_*_time` flags, `eventType` and `dueOffsetDays` are *all* cross-kind on it. That is a
     lot of newly-failing input for one kind, and it is safe only because a CSV blank cell
     reads as absent (`cell` filters empty strings) — so a mixed-kind file with a fixed column
     set, which is the normal shape of an import, is untouched.
8. **Delete the flat structs.** Remove `CreateProjectItemParams`/`UpdateProjectItemParams`/
   `CreateItemParams`/`UpdateItemParams`/`CreateTeamItemParams`/`UpdateTeamItemParams`; have
   `items::create_item` and `team_items::create_team_item` take `NewItem` directly. This is the
   stage that deletes the ~80 lines of field transcription and closes the silent-drop class of
   bug for good. `build_item_type` collapses into the `NewItemKind` match.

## Verification

- `cargo build`, `cargo test`, and plain `cargo fmt` (no path arguments, repo root — CLAUDE.md's
  formatting policy) after every stage.
- No `task codegen` at any stage: `model/` is untouched throughout.
- `task web-styles` only if a template's Tailwind classes change (Stages 2–5 may touch templates
  where a form field's shape changes; most should not).
- Per CLAUDE.md this repo does **not** use Playwright. In-browser behavior is the user's own
  verification step, and any claim about it will be stated as unverified.

## Explicitly out of scope

- The SQLite schema (see Context) and the Smithy wire format beyond Stage 7's conversion.
- Newtypes for `priority`/`due_offset_days` — domain-layer changes, a separate question.
- Splitting `ItemSeries` into `TaskSeries`/`EventSeries` — already tracked in
  `docs/issues_and_features.md` and independent of this, though it is the same idea one level up
  and this plan's staging discipline applies to it too.
- `service::templates.rs`'s own params family, beyond the Stage 4 audit.
