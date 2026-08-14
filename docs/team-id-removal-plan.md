# Remove `team_id` as the scoping key for items — finish the ProjectItem migration

**Status: Complete.** All six stages shipped (see Stage status below); `items.team_id` no longer exists. Kept here as historical context, the same way this plan's own predecessor (`docs/project-abstraction-plan.md`) is — see CLAUDE.md's Smithy/Storage Layer sections for the resulting architecture.

## Context

The app migrated from separate personal `Item` (`user_id`-keyed) and `TeamItem` (`team_id`-keyed) Smithy resources to one unified `ProjectItem` resource (`project_id`-keyed). The *Smithy/HTTP* surface for the old resources was retired, but the underlying storage/service layer never finished the cutover: `team_id` is still the real scoping key used internally for team-backed items (`src/service/team_items.rs`), with `project_id` bolted on as a secondary, dual-written field.

`docs/project-abstraction-plan.md`'s own Stage C drafting notes considered this exact rearchitecture and explicitly declined it, calling it "a rearchitecture on the scale of Stage B itself" and recommending `items.user_id`/`team_id` be kept in the schema permanently. That recommendation was buried in implementation-notes prose rather than flagged as a real decision point, and it papered over a real gap rather than resolving one.

The gap surfaced as a concrete bug during this investigation: `ProjectRepo::attach_team`/`detach_team` only ever update `projects.team_id` and `project_members` — they never touch the `items` table. Since an item's `team_id` is written once at creation and never updated again, and since `update_project_item`/`delete_project_item` (`src/service/project_items.rs`) resolve write-scope from **the project's current `team_id`** rather than the item's own stored value, an item created under a team that's later detached (or replaced by a different team) becomes permanently un-updatable and un-deletable (`NotFound`) — even though reads (`get_project_item`/`list_project_items`, already `project_id`-keyed) still see it fine. No test covers this today.

Reviewed the four "reasons to keep `team_id`" that came up during investigation, and none survive scrutiny as independent architectural justification — each is just "other code currently reads `item.team_id`," which restates the incompleteness rather than justifying it. The one path that could have been a genuine reason — a deliberate "historical snapshot" semantic — is contradicted by the code's own behavior (writes resolve from the *current* project team, not the item's frozen one).

**Goal:** make `project_id` the sole scoping key for item storage/business logic. `Item.user_id` (personal items) and `activity_log.team_id` (a separate, still-live legacy Smithy surface — `ListTeamActivityLog`/`UndoActivityLogEntry`) are explicitly out of scope and untouched by this plan.

**Correction to note:** an earlier draft of this plan (produced by a Plan subagent) proposed adding a `status` column to `project_members` plus new sync logic for team-invite-accept/member-removal, treating that as a prerequisite stage. Verifying that against the actual code showed it's unnecessary — `TeamRepo::accept`/`remove_member` (`src/storage/sqlite/teams.rs:219-287`) and `ProjectRepo::attach_team`/`detach_team` (`src/storage/sqlite/projects.rs:103-161`) already cascade membership changes into `project_members`, with existing passing tests covering exactly this. `project_members` row-presence already reliably means "active member" — `ProjectRepo::member_role` is already sufficient as the check. That stage is removed below; the plan is shorter and lower-risk as a result.

---

## Workflow: commit and clear context between stages

Each stage below is meant to be a separate work session, not a continuous one. **After finishing a stage: run its verification, commit the stage on its own (reference the stage number in the commit message, e.g. "Stage 3: ..."), update the Stage status checklist below to mark it done, then clear context before starting the next stage.** The next session will not have this conversation's context — it only has this plan file and the current state of the repo. So:

- Each stage's description below must stand alone: don't rely on the next session inferring intent from earlier stages' reasoning, only from what's written here and what's visible in the code/git history at that point.
- Before starting a stage, the next session should re-read this whole plan file (not just that stage's section) plus check the Stage status checklist to confirm what's actually merged — git log/diff is the source of truth over the checklist if they ever disagree.
- If a stage's implementation reveals the plan was wrong about something (like the Stage 3 correction above did to the original draft), fix this plan file itself before moving on — don't just fix it in code and leave the doc stale for whoever picks up the next stage.

This plan document is the source of truth (it originated in a Claude Code plan-mode session at `~/.claude/plans/ok-i-just-learned-snug-starlight.md`, then was copied here so it survives independent of that session). Update this file in place as stages complete.

### Stage status

- [x] Stage 0 — copy this plan into `docs/team-id-removal-plan.md` and commit
- [x] Stage 1 — `list_templates_by_project`
- [x] Stage 2 — collapse shared helpers onto `update_by_project`
- [x] Stage 3 — project-native membership/assignee checks
- [x] Stage 4 — rewrite `service::team_items.rs`
- [x] Stage 5 — rewrite `service::templates.rs`'s team-template twin
- [x] Stage 6 — external-API read-site fixes + final cleanup (drop field/column)

## Staged plan

Ship in order; each stage should be its own PR/commit and should leave `cargo test` green.

### Stage 1 — Storage: add `list_templates_by_project`

Closes the one genuine storage gap: no `project_id`-scoped, `TEMPLATE`-filtered list method exists yet (`list_by_project` applies no `item_type` filter).

- `ItemRepo` trait (`src/storage/sqlite/mod.rs`): add `async fn list_templates_by_project(&self, project_id: &str) -> Result<Vec<Item>, RepoError>`.
- `src/storage/sqlite/items.rs`: impl mirroring `list_team_templates`'s query shape (`WHERE project_id = ? AND item_type = 'TEMPLATE' AND parent_item_id IS NULL ORDER BY name ASC`).
- **Verify:** new sqlite-level test — seed a template + non-template in one project, confirm only the template returns; seed a template in a different project, confirm it's excluded.
- Independently shippable, no existing caller touches it until Stage 4.

### Stage 2 — Collapse `service::items.rs`'s shared helpers onto `update_by_project`

`unlink_source_event_tasks`, `sync_offset_children`, `sync_source_event_tasks`, `repoint_source_event_tasks` (`src/service/items.rs`) each branch `if task.team_id.is_some() { repo.update_team_item(...) } else { repo.update(...) }`. Since every item reaching these paths has carried `project_id` since the dual-write stage, collapse each to one unconditional `repo.update_by_project(&task).await?` call (`update_by_project` already exists in `ItemRepo` — added earlier, currently unused).

- Before shipping, run a one-off sanity check against the real `todo.db`: `SELECT COUNT(*) FROM items WHERE project_id IS NULL` should be `0`. If it isn't, decide whether to backfill those rows first — `update_by_project`'s `WHERE id = ? AND project_id = ?` will correctly `NotFound` (not silently corrupt anything) on a `NULL` project_id, but would newly break these specific helper paths for old rows that predate the dual-write.
- **Verify:** existing tests for these four helpers should pass unchanged if their fixtures set `project_id`; audit each and add `project_id` to any fixture missing it.
- Independently shippable.

### Stage 3 — Project-native membership/assignee checks

Replace the two currently-redundant membership checks with one, built on the already-synced `project_members` table (no schema change — see the Correction note above).

- `src/service/projects.rs`: repoint `require_project_member`'s team-backed branch (currently `teams.member_status(team_id, user_id)`, line ~24) to `projects.member_role(project_id, user_id)` — `Some(_)` = active member, `None` = not. This drops `require_project_member`'s dependency on `TeamRepo` for the team-backed case (personal-project branch is unaffected — still checks `owner_user_id`).
- Add `resolve_project_assignee(projects, project_id, assignee_id) -> Result<Option<String>, ItemError>` in `src/service/projects.rs` — a near-verbatim port of `team_items::resolve_assignee`, but validating via `projects.member_role(project_id, &assignee_id).await?.is_some()` instead of `teams.member_status`.
- `team_items::require_active_member`/`resolve_assignee` stay for now (Stage 4 repoints their callers and removes them).
- **Verify:** new tests in `service::projects`'s test module (`MockProjectRepo::expect_member_role`) covering both functions' allow/reject paths. No production-risk gating needed here — unlike the removed original Stage D1/D4 pairing, this consumes a membership signal (`project_members`) that's already correct and already tested; nothing new to "prove" first.

### Stage 4 — Rewrite `service::team_items.rs` to be `project_id`-primary

The core rearchitecture. `CreateTeamItemParams`/`UpdateTeamItemParams` take `project_id: String` instead of `team_id: String`; every internal call switches from `team_id`-keyed repo methods to `project_id`-keyed ones.

- `repo.get_team_item(&params.team_id, id)` → `repo.get_by_project(&params.project_id, id)` (all call sites: parent-Event check, `current` fetch, anchor-resolution helpers).
- `top_level_anchor_team`/`resolve_offset_anchor_team` → take `project_id`, delegate to `repo.get_by_project`. Consider deleting these in favor of `project_items.rs`'s existing `resolve_top_level_anchor_unchecked` (same shape, already `project_id`-keyed) — simplification opportunity, not required.
- `require_active_member(teams, &params.team_id, ...)` → `require_project_member`/new check from Stage 3.
- `resolve_assignee(teams, &params.team_id, ...)` → `resolve_project_assignee` from Stage 3.
- Build the new `Item` via `project_id` directly (not `Item::new_team_item`) — e.g. a new `Item::new_project_item(project_id, name)` constructor, or a plain struct literal. `new_team_item` itself gets deleted in Stage 6.
- `repo.update_team_item(&item)` → `repo.update_by_project(&item)` (both occurrences: plain-update tail and recurrence-archival branch).
- `activity_log.log_activity(&params.team_id, ...)`: `log_activity`'s `team_id` parameter stays `NOT NULL` (out of scope, unaffected). Its caller one layer up (`update_project_item` in `project_items.rs`) already resolves `project.team_id` fresh via its own `projects.get(project_id)` call before delegating down — thread that resolved value down as an explicit parameter rather than re-deriving it inside `team_items.rs`.
- `project_items.rs`'s `create_project_item`/`update_project_item`/`delete_project_item` keep their `project.team_id.is_some()` branch (a genuine business-rule fork — points/assignment/team-completion-guard only apply to team-backed projects) — but both arms now call `project_id`-keyed functions, so the "which repo method" fork collapses even though the "which business rules" fork remains.
- **Verify — largest test-fallout stage:** every existing `team_items.rs` test needs `team_id` params/mocks swapped for `project_id` ones (`expect_get_team_item`→`expect_get_by_project`, `MockTeamRepo::expect_member_status`→`MockProjectRepo::expect_member_role`, etc.). `project_items.rs`'s own tests asserting `item.team_id.as_deref() == Some(...)` move to `item.project_id`.
- **Add the one test that directly proves the original bug is fixed**, not just patched: create an item under a team-backed project, detach the team (or attach a different one), confirm `update_project_item`/`delete_project_item` still succeed against that item. No such test exists today.

**Stage 4 as implemented — two corrections to the plan above:**

1. **`items.team_id` still needs a narrow dual-write on create, which this draft missed.** `ItemRepo::update_by_project`'s `UPDATE` statement never touches the `team_id` column at all (only `create`'s `INSERT` does), so `update_team_item` genuinely has no dual-write concern — but `create_team_item` does. Stage 6's "external-API read sites" (`list_items_due`'s `teamId` field, `list_assigned_items`'s owner fallback, `assigned_items::AssignedItemRow::from_item`'s `item.team_id.as_ref()?` gate) still read `item.team_id` directly and aren't fixed until Stage 6. Building the new item via a bare `Item::new_project_item` (no `team_id`) would have left every team item created between Stage 4 and Stage 6 shipping with `team_id: NULL` — invisible to those not-yet-migrated read sites, a real regression window in a live app, not just a cosmetic gap. Fixed by threading `team_id: &str` into `create_team_item` as a plain parameter (not a `CreateTeamItemParams` field — `project_id` is still the only thing repo/membership logic reads) purely to dual-write `item.team_id = Some(team_id)` before `repo.create`. The caller (`project_items::create_project_item`) already has this value for free from its own `project.team_id` match, so nothing extra is fetched. `update_team_item` also takes a `team_id: &str` parameter, but only for `log_activity`'s still-`NOT NULL` column — no dual-write inside it, since there is nothing there for `team_id` to write into.
2. **`resolve_assignee` had no other callers anywhere in the codebase** once its two call sites inside `create_team_item`/`update_team_item` were repointed to `resolve_project_assignee` — so, per this section's own instruction, it was deleted outright rather than "kept for now." **`require_active_member` could not be deleted** the same way: `service::templates.rs` (Stage 5's own target), `service::activity_log::undo_activity_log_entry` (permanently `team_id`-keyed, see Stage 6), and the legacy `json_api::team_templates`/`json_api::activity_log` handlers all still call it directly. It stays defined in `team_items.rs`, just no longer called by that file's own `create_team_item`/`update_team_item`/`delete_team_item`.

Also folded into this stage (not explicitly called out above, but a natural consequence of `project_id` becoming the primary key): `update_team_item`'s points-award path no longer needs the `get_by_team` fallback that used to resolve `award_project_id` for pre-B2 rows with `item.project_id: NULL` — `params.project_id` is now always known, so points award directly against it. This is, concretely, the exact class of bug this whole plan exists to fix.

### Stage 5 as implemented — two corrections to the plan below

1. **The plan's own text never accounted for `json_api::team_templates::create_team_template`, a still-live legacy Smithy operation.** `CreateTeamTemplate` (`POST /teams/{teamId}/templates`, `model/src/main/smithy/team.smithy`) only carries `teamId` on its input — there is no `projectId` field on that Smithy shape, and no plan to add one (out of scope; the Smithy surface itself isn't part of this plan). Once `service::templates::create_team_template` became `project_id`-primary, this handler (`src/json_api/team_templates.rs`) needed a way to get from the `team_id` it actually has to the `project_id` the service function now requires. Fixed by adding a `server::Extension<Arc<dyn ProjectRepo>>` param to the handler (the extension is already globally layered onto the smithy service in `src/main.rs`, so no wiring changes needed there) and resolving via `projects.get_by_team(&input.team_id)` before calling in — `None` maps to `not_found()`, the same shape every other "the id you gave me doesn't resolve" case in this handler already uses. `json_api::team_templates::list_team_templates` (the sibling `ListTeamTemplates` operation) is unaffected — it reads `repo.list_team_templates(team_id)` directly, a genuinely `team_id`-keyed repo method untouched until Stage 6, so it doesn't call into `service::templates.rs` at all.
2. **`create_team_template` needs the same `team_id`-dual-write correction Stage 4 made for `create_team_item`, for the same reason.** `ItemRepo::list_team_templates` (the repo method backing the legacy `ListTeamTemplates` operation above) queries by the `team_id` column directly, and isn't migrated off it until Stage 6. Since `update_by_project`'s `UPDATE` never touches `team_id` (confirmed in `src/storage/sqlite/items.rs`) but `create`'s `INSERT` does, this is a create-only concern, exactly like `create_team_item`'s. Fixed the same way: `team_id: &str` threaded into `create_team_template` as a plain parameter (not a `CreateTeamTemplateParams` field), used only to set `item.team_id` after building the row via `Item::new_project_item`. `update_team_template` needs no equivalent — it never calls `repo.create`, and `update_by_project` doesn't touch the column, so there's nothing for a `team_id` parameter to dual-write into (matching `update_team_item`'s own Stage 4 finding).

Also folded into this stage, per the "Bonus simplification" note below: `create_project_template`'s old two-write shape (create, then re-fetch-and-update just to backfill `project_id`) is gone — both `create_template` (given an optional new `project_id` param) and `create_team_template` (via `Item::new_project_item`) now set `project_id` on the row as part of their single `repo.create` call.

### Stage 5 — Rewrite `service::templates.rs`'s team-template twin

`src/service/templates.rs` has a second, independent copy of the same `team_id`-keyed pattern (`create_team_template`/`update_team_template`, using `Item::new_team_item`, `repo.get_team_item`, `repo.update_team_item`, and importing `team_items::require_active_member` directly) — load-bearing on the same field/methods, so it must move in lockstep or the column can never be dropped.

- `CreateTeamTemplateParams.team_id`/`UpdateTeamTemplateParams.team_id` → `project_id: String`.
- `require_active_member(teams, ...)` → the Stage 3 check (also drops this file's direct import from `team_items.rs`).
- `repo.get_team_item`/`update_team_item` → `repo.get_by_project`/`update_by_project`.
- `list_project_templates`'s current `match project.team_id { Some(team_id) => repo.list_team_templates(&team_id), None => repo.list_templates(...) }` → single `repo.list_templates_by_project(project_id)` call (Stage 1).
- Bonus simplification riding along: `create_project_template`'s current two-write shape (create, then re-fetch + update just to backfill `project_id`) becomes unnecessary once `create_team_template`/`create_template` accept and set `project_id` directly on the constructed `Item` before the single `repo.create` call.
- **Verify:** port every `create_team_template_*`/`update_team_template_*`/`list_project_templates_*` test the same way as Stage 4. New test confirming `list_project_templates` calls `list_templates_by_project` and correctly excludes non-templates and out-of-project templates.

### Stage 6 — Fix the external-API read sites, then drop the field/column

Combine the remaining read-site fixes with the final cleanup, since by this point they're the only things left referencing `team_id`.

**Read-site fixes:**
- `src/json_api/items.rs::list_items_due`: `DueItemSummary.teamId` currently reads `di.item.team_id.clone()` directly — resolve via `di.item.project_id` → `projects.get(project_id).await?.team_id` instead (add a `ProjectRepo` extension to the handler; dedup lookups per unique `project_id` in the result set to avoid N+1 queries).
- `src/json_api/items.rs::list_assigned_items`: same fix for `AssignedItemSummary`'s `i.user_id.clone().or(i.team_id.clone())` owner fallback.
- `src/web_ui/assigned_items.rs::AssignedItemRow::from_item`: drop the `item.team_id.as_ref()?` gate — `list_assigned` already filters by assignment, and the remaining `item.project_id.clone()?` gate is sufficient.
- **Verify:** `src/json_api/items.rs`'s test module is currently empty — add real coverage here (mock `ItemRepo`+`ProjectRepo`, assert `teamId`/`ownerUserId` resolve correctly for personal and team-backed rows). Manual smoke test for the web UI assigned-items page.

**Final cleanup (only after Stages 1–5 and the read-site fixes above are merged and confirmed):**
- `src/domain/item.rs`: remove `team_id: Option<String>` and `new_team_item`. Sweep `grep -rn "\.team_id\|team_id:" --include=*.rs src/` — expect only test-fixture references left by this point; update/delete them.
- `ItemRepo` trait + `src/storage/sqlite/items.rs`: remove `get_team_item`, `list_team_items`, `update_team_item`, `list_due_team_items`, `list_team_templates` and their impls; remove `team_id` from `ITEM_SELECT`, `create`'s INSERT, and row-mapping.
- New migration `src/storage/migrations/drop_items_team_id.rs`: `ALTER TABLE items DROP COLUMN team_id`, guarded with `column_exists` (follow `drop_team_member_points.rs`'s precedent exactly). Update the baseline `CREATE TABLE IF NOT EXISTS items` in `src/storage/sqlite/mod.rs` in the same change so a fresh DB matches. No index to drop (`team_id` has none).
- **Verify:** migration idempotency test (run `up` twice); full `cargo test` clean; real-snapshot check — copy the actual `todo.db`, run the migration against the copy, start the server against it, manually round-trip a team-backed project's tasks/events/templates over HTTP, confirm `activity_log`/`ListTeamActivityLog`/`UndoActivityLogEntry` still work (untouched, separate column), confirm `prl`/MCP item tools work end to end.

### Stage 6 as implemented — two corrections to the plan above

1. **The read-site-fixes bullet list above missed one live caller of `ItemRepo::list_team_templates`.** `json_api::team_templates::list_team_templates` (the handler backing the still-live legacy `ListTeamTemplates` Smithy operation — see Stage 5's own correction #1 about its sibling `create_team_template`) called `repo.list_team_templates(&input.team_id)` directly. Removing that repo method per the "Final cleanup" bullet without first repointing this handler would have broken it. Fixed the same way Stage 5 fixed `create_team_template`: added a `ProjectRepo` extension to the handler, resolved `input.team_id` → its backing project via `ProjectRepo::get_by_team`, and called `repo.list_templates_by_project(&project.id)` instead. `list_team_templates` (the repo method) only became genuinely dead — and safe to delete — once this call site was gone.
2. **Removing the `items.team_id` baseline column exposed a real, unrelated latent bug in `backfill_projects.rs` (migration version 11) that would have crashed every fresh install.** That migration's `items.project_id` backfill step unconditionally ran `UPDATE items SET project_id = (...) WHERE team_id IS NOT NULL AND project_id IS NULL` — safe as long as `items.team_id` was guaranteed to exist whenever this migration executed, true for any *upgrading* DB (version 11 always runs before version 14 in sequence) but false for a **fresh** DB: `create_pool()`'s baseline schema (no `team_id`, per this stage's own change) plus an empty `_migrations` table means `run_migrations` runs every migration from scratch on first boot, including this one. Caught by the plan's own "real-snapshot check" verification step below — specifically by `storage::migrations::tests::is_a_noop_against_a_db_already_on_the_current_schema`, whose fixture models exactly this "fresh baseline, no migration history" case. Fixed by guarding that one `UPDATE` with `column_exists(conn, "items", "team_id")` — on a DB where the column is already gone, there's nothing to backfill from anyway. This is the same category of gap this whole plan exists to close (an assumption about column presence that stopped being true), just one commit later than expected.

Both fixes are covered by existing/new automated tests (`json_api::items::tests`, `storage::migrations::tests`, `storage::migrations::backfill_projects::tests`) plus a manual real-`todo.db`-copy round-trip: migrated cleanly (versions 13→14 applied, `items.team_id` column gone, all 60 rows kept a non-null `project_id`), then a live server against the copy correctly served `GET /users/{userId}/due-items`, `GET /users/{userId}/assigned-items` (both resolving `teamId`/`ownerUserId` via the item's `project_id`), `GET /teams/{teamId}/templates` (the migrated `ListTeamTemplates` handler), and `GET /web/assigned-items`.

---

## Open, non-blocking question — resolved

Once Stage 4 landed, `team_items.rs` had nothing team-specific left in its *storage keying* — only in its *business rules* (points, assignment, team-completion guard). Raised with the user after Stage 6 shipped, as a choice between renaming the file, merging it into `items.rs`, or leaving it as-is. **Decided: leave as-is.** The file still owns genuinely distinct business rules that only apply to team-backed projects, so a merge would blur two still-different rule sets into one ~3,000-line file (both files are already large, mostly tests); a rename was judged not worth the ~12-file/26-reference mechanical churn for a naming-only change. No further action expected here.

## Critical files

- `src/service/team_items.rs`, `src/service/project_items.rs`, `src/service/templates.rs`, `src/service/items.rs`, `src/service/projects.rs`
- `src/storage/sqlite/items.rs`, `src/storage/sqlite/projects.rs`, `src/storage/sqlite/mod.rs`
- `src/domain/item.rs`
- `src/json_api/items.rs`, `src/web_ui/assigned_items.rs`
- `docs/project-abstraction-plan.md` (historical context)
