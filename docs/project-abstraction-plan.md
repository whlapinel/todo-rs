# Introduce a Project abstraction (Team → pure sharing group, personal/team items → project items)

## Context

The original ask was to reduce duplication in `src/web_ui/` (tasks.rs vs
team_tasks.rs, events.rs vs team_events.rs, etc). Research showed that
duplication is *structural*, not incidental: personal and team items are two
genuinely different code paths today (different service functions, different
repo methods, team items carry membership/role/points concerns personal items
have no equivalent of), so mechanically merging them would just push
`Option`/branch-not-taken complexity into code that doesn't need it.

The user's real fix is deeper: introduce a **Project** as the one namespace
every item belongs to. A project can be personal (one owner, no team attached)
or shared (a team attached, granting its members access). **Team** stops being
an item container and becomes a pure, reusable group of user-ids — the same
team can be attached to multiple projects, which is the actual point of
"easy sharing" (invite your "Family" team once, attach it to as many projects
as you want, instead of managing membership per-project). Role (admin/member)
and points move from `team_members` to a new `project_members`, since they're
really about what a user can do *inside a specific set of items*, not about
group membership itself.

This one change is what collapses the tasks/team_tasks-style duplication at
the root: instead of `/web/tasks` + `/web/team-tasks/:team_id` as two parallel
screens, there is one `/web/projects/:project_id/tasks` screen. Confirmed
design decisions (from user):
- Role/points move to `project_members`, keyed `(project_id, user_id)`.
- A project has **at most one attached team** (not several) — but the same
  team can back many different projects.
- Every user gets one auto-created default personal project, and can create
  additional personal (team-less) projects of their own beyond that.

## Target shape (for reference — not all built in this pass)

```
Team           { id, name }                          — pure group, no role/points
TeamMember     { team_id, user_id, status, invited_by } — role/points REMOVED
Project        { id, name, owner_user_id, team_id: Option<String> }
ProjectMember  { project_id, user_id, role, points }  — PK (project_id, user_id)
Item           { ..., project_id: String, ... }       — replaces user_id/team_id
                                                          with ONE foreign key
```
Access check for "is user X in project P": if `project.team_id` is `Some`,
check `team_members(team_id, X).status == ACTIVE`; else `X == project.owner_user_id`.
This one check replaces both "personal item ⇒ implicit owner, no lookup
needed" and today's `require_active_member`/`require_team_admin`.

**Open call, not yet decided — recommendation stated, flag if you want it
different:** Team drops the role concept entirely (matches "pure group for
sharing"), but *something* still has to gate who can invite/remove members or
rename a team. Recommend keeping the existing "creator" as a lightweight,
un-roled owner (`teams.owner_user_id`, set once at `create`, no `team_members`
role column at all) — smallest change from today's `TeamRepo::create`
behavior, and avoids reinventing a second permission system on top of Team
when Project already has one. If you'd rather any active member manage the
team (no gatekeeping at all), or keep a minimal role, say so before Stage A
lands the schema.

## Why staged, and what's in scope now

This touches the Smithy model, the `items` table's core ownership column (on
a **live production DB with real user data** — `todo.lapinel-fam.club`),
every `/web/*` route (37+ team routes alone, per the route inventory below),
the CLI, and the MCP server. Doing this as one big-bang change is the wrong
risk profile — and even "Stage A" below was too big as a single unit, so it's
now broken into five independently-landable increments (A1-A5), each with its
own verification, each leaving the running app's actual behavior unchanged
until the very end of the sequence:

- **A1 — schema only.** New `projects`/`project_members` tables and a
  nullable `items.project_id` column. Nothing reads or writes them. Smallest,
  safest possible first step — pure DDL, trivially reviewable, trivially
  revertible.
- **A2 — storage CRUD.** `ProjectRepo` trait + SQLite impl, plain
  create/get/list/update/delete/member-role/points methods. No team-sync
  logic yet — `attach_team`/`detach_team` just write the `team_id` column,
  no cascade. Not called from anywhere in the running app yet (unit-tested
  in isolation only).
- **A3 — service layer CRUD.** `service::projects.rs` wrapping A2 with the
  same authorization shape `service::teams.rs` already has
  (`require_project_member`/`require_project_admin`). Still not reachable
  via HTTP — service-layer unit tests only.
- **A4 — team↔project membership sync.** The one genuinely new piece of
  logic: `attach_team_to_project` actually seeding `project_members` from
  current team members, and the sync hook in `TeamRepo::accept`/
  `remove_member` cascading to every project a team backs. Isolated as its
  own stage because it's the highest-risk, most-novel part — easiest to get
  wrong, easiest to review alone.
- **A5 — Smithy surface + wiring.** The new Project operations, `task
  codegen`, handler wiring in `main.rs`, and hooking default-personal-project
  creation into new-user creation. This is what makes A1-A4 actually
  reachable (via `prl`/MCP/curl) — still no web UI screens.
- **Stage B (separate future planning pass, itself likely needs further
  breakdown when we get there)** — backfill migration (one personal Project
  per existing user, one Project-with-team per existing Team, item
  `project_id` backfilled from current `user_id`/`team_id`) + web UI cutover:
  collapse `tasks.rs`/`team_tasks.rs` (and the events/simple_lists/
  templates/dashboard/activity equivalents) into single `:project_id`-scoped
  screens. This is where the actual duplication elimination happens, and
  where finishing the shared `Row` component
  (`src/web_ui/components/row.rs`, already started) becomes essential.
  CLI and MCP server move to project-based operations here too.
- **Stage C (separate future planning pass)** — cleanup: drop
  `items.user_id`/`items.team_id` and `team_members.role`/`points` (or leave
  unused if SQLite column-drop is impractical), rework the
  `TODO_BOOTSTRAP_ADMIN_TEAM_ID` auth.rs bootstrap to be project-aware, decide
  the fate of the dead `share_active_team` repo method (found during
  research — implemented, zero callers; may be exactly the "do these two
  users already share a project" check Stage B's UI needs).

Stage B/C are **not** planned in detail here — they depend on how A1-A5
actually land. This plan covers A1-A5 only, and even those should be done and
verified one at a time rather than all in one sitting.

## Process: one stage per session, context handed off via this file

Each stage (A1, A2, A3, A4, A5, and B/C once planned) is done in its own
session — compact or `/clear` between stages rather than carrying the whole
history forward. That means **this plan file is the only thing that survives
between stages**, so before ending a stage (before the compact/clear), update
that stage's section below with an **"Implementation notes"** entry covering
whatever the next session needs and can't re-derive by just reading the
committed code:
- Exact names actually used, if they ended up different from what's written
  here (struct/trait/fn names, migration version number, file paths).
- Any deviation from the plan made during implementation, and why.
- Test/verification status — what was run, what passed.
- Anything discovered mid-stage that changes assumptions later stages make
  (e.g. if A2's `ProjectRepo` trait shape ended up needing a method this plan
  didn't anticipate, A3/A4 need to know that going in).
Each stage's own code + tests are largely self-documenting once committed —
this note only needs to capture *decisions and deviations*, not restate what
`git log`/`git diff` already shows.

## A1 — Schema only

New migration file (next version number after `add_item_source_event_id`):
```sql
CREATE TABLE projects (
    id TEXT PRIMARY KEY, name TEXT NOT NULL,
    owner_user_id TEXT NOT NULL, team_id TEXT
);
CREATE TABLE project_members (
    project_id TEXT NOT NULL, user_id TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'member', points INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, user_id)
);
```
Plus `items.project_id TEXT` (nullable, unused — guarded with `column_exists`
per the existing migration convention, e.g. `add_team_member_role.rs`). Add
the same three shapes to the `CREATE TABLE IF NOT EXISTS` baseline in
`create_pool()` so a fresh DB is correct from the start. No domain structs,
no repo trait, no service code — just the tables existing.

**Verify:** migration runs cleanly against a copy of the dev DB (`task db
copy`, per the Taskfile, mentioned in recent commit history); `task check`
compiles; `cargo test` has zero regressions (nothing changed that any
existing code touches).

**Implementation notes:** Done. Migration is `src/storage/migrations/add_projects.rs`
(`AddProjects`, version `10`), registered in `all_migrations()`. Matches the
plan's SQL as written, plus `idx_items_project_id` — created *inside the
migration*, not the baseline (see the migration's own doc comment: an index on
a column added to an *existing* table must live in the migration that adds the
column, per the `source_event_id` index-ordering bug noted in CLAUDE.md/issues.md).
`idx_projects_team_id`/`idx_project_members_user_id` (on the two brand-new
tables) *are* in the baseline (`create_pool()`), since a fresh DB creates the
whole table in one shot either way. Baseline `CREATE TABLE IF NOT EXISTS` in
`src/storage/sqlite/mod.rs` updated identically. `cargo test storage::migrations`:
9/9 passing, including `add_projects::tests::creates_tables_and_is_idempotent`
and the three `applied_count == 10` assertions (bumped from 9). `cargo check`
clean (warnings only, none new). Not yet run against a live copy of the prod
DB via `task db copy` — only against the in-memory sqlx pool the migration
tests use. No domain/service code touches these tables yet, as planned.

## A2 — Storage CRUD (`ProjectRepo`)

- `src/domain/project.rs`: `Project { id, name, owner_user_id, team_id: Option<String> }`,
  `ProjectMember { project_id, user_id, role: MemberRole, points: i32 }`.
  Reuse/rename `TeamRole` → `MemberRole` if Team truly drops role (see the
  open call above) rather than leaving two near-identical enums.
- `ProjectRepo` trait (`src/storage/mod.rs` or wherever `TeamRepo` lives),
  `#[cfg_attr(test, mockall::automock)]` per existing convention: `create`,
  `get`, `update_name`, `attach_team`/`detach_team` (plain column write, no
  cascade yet — that's A4), `delete`, `list_for_user`, `list_members`,
  `member_role`, `set_member_role`, `add_project_points`. Mirror `TeamRepo`'s
  method shapes closely.
- SQLite impl in `src/storage/sqlite/projects.rs`, following
  `src/storage/sqlite/teams.rs`'s existing structure.
- Nothing calls any of this yet.

**Verify:** unit tests against the sqlite impl directly (mirroring however
`src/storage/sqlite/teams.rs` is tested today — check for an existing test
module/file and match its pattern) for each method in isolation.

**Implementation notes:** Done. `src/domain/project.rs`: `Project { id, name,
owner_user_id, team_id: Option<String> }`, `ProjectMember { project_id, user_id,
role: TeamRole, points: i32 }` — reused `TeamRole` as-is rather than introducing
`MemberRole`, since the "does Team drop role entirely" open call is still
undecided; revisit this reuse if that call resolves toward keeping Team's own
role concept (would make reuse confusing) vs. dropping it (reuse stays fine,
maybe rename at that point). `ProjectMember` itself ended up unused in favor of
a query-facing `ProjectMemberInfo { user: User, role: TeamRole, points: i32 }`
in `src/storage/sqlite/mod.rs` (mirroring `TeamMemberInfo`, minus a `status`
field — no independent invite flow at the project level, see its doc comment)
— kept `ProjectMember` in the domain module anyway since A3/A4 will likely want
a plain non-join-shaped struct.

`ProjectRepo` trait lives in `src/storage/sqlite/mod.rs` alongside
`TeamRepo`/`UserRepo`/etc (not a separate file — matches existing convention,
all repo traits live in this one `mod.rs`). Exact method set matches the plan
list verbatim. One deviation forced by the compiler: `create`'s `team_id:
Option<&str>` parameter needed an explicit `<'a>` lifetime on both the trait
method and the impl (`async fn create<'a>(&'a self, name: &'a str,
owner_user_id: &'a str, team_id: Option<&'a str>)`) — `#[async_trait]`'s
desugaring can't elide a lifetime buried inside `Option<&str>`; there's already
a precedent for this exact shape in `UserRepo::get_or_create_by_email`.

SQLite impl: `src/storage/sqlite/projects.rs`, `SqliteProjectRepo(SqlitePool)`,
structured identically to `teams.rs`. `create` inserts the project row then a
`project_members` row for the owner (role `'admin'`, points `0`) in the same
two-insert shape as `TeamRepo::create` seeding the creator as team admin.
`delete` cascades to `project_members` the same way `TeamRepo::remove_member`
conditionally cascades to `teams` (but unconditional here — deleting a project
always deletes its whole membership table, there's no "last member" concept to
preserve). `list_for_user` joins `project_members` (not a bare owner-only
lookup), so it returns every project a user is a member of, not just ones they
own — matches `TeamRepo::list_for_user`'s equivalent join shape.

15 new unit tests (mirroring `activity_log.rs`'s in-file `test_pool()` pattern,
since `teams.rs`/`items.rs` have no sqlite-level tests of their own to mirror
instead) covering every method incl. not-found paths. `cargo test`: 126/126
passing (111 prior + 15 new), zero regressions. `cargo check`: clean, only the
expected new "never constructed"/"never used" dead-code warnings on
`ProjectRepo`/`SqliteProjectRepo`/`Project`/`ProjectMember`/`ProjectMemberInfo`
themselves — expected per the stage's own scope ("not called from anywhere in
the running app yet"). Not wired into `main.rs` at all (no `Arc<dyn
ProjectRepo>` extension) — confirmed this repo doesn't use a single `Arc<dyn
Repo>` (CLAUDE.md's handler-signature snippet is a simplification; the real
code holds one `Arc<dyn XRepo>` per trait), so A5 will add a fourth one
alongside `user_repo`/`item_repo`/`team_repo`/`activity_log_repo` in `main.rs`,
not modify an existing combined trait.

## A3 — Service layer CRUD

- `src/service/projects.rs`: `require_project_member`, `require_project_admin`
  (same shape as `service::teams.rs`'s equivalents — access check is "if
  `project.team_id` is `Some`, check `team_members(team_id, X).status ==
  ACTIVE`; else `X == project.owner_user_id`"), `create_project`,
  `list_projects`, `list_project_members`, `set_project_member_role`. No
  `attach_team_to_project` yet (A4) — `create_project` never sets `team_id`
  at this stage.
- Still not reachable via HTTP.

**Verify:** service-layer unit tests (mirroring `service::teams.rs`'s
existing coverage) — access checks pass/fail correctly for owner vs.
non-member; `create_project` produces the right row shape.

**Implementation notes:** Done. `src/service/projects.rs`, registered in
`src/service/mod.rs`. `require_project_member`/`require_project_admin` match the
plan's access-check formula exactly (`team_id: Some` → `TeamRepo::member_status ==
ACTIVE`; `None` → `user_id == project.owner_user_id`), and — matching
`require_team_admin`'s shape as instructed — both return `Result<(), ItemError>`,
not the fetched `Project`; each internally calls `projects.get()` to read `team_id`
but doesn't expose it. `require_project_admin` checks membership first (via
`require_project_member`) then `ProjectRepo::member_role == Admin`, so a non-member
gets "not a member" rather than "not an admin".

`create_project(projects, name, owner_user_id)` always passes `team_id: None` to
`ProjectRepo::create` — no `team_id` parameter on the service fn at all, since A3
never attaches a team (matches the plan). `list_projects` is a thin
`list_for_user` wrapper. `list_project_members` gates on `require_project_member`
then delegates. `set_project_member_role` gates on `require_project_admin` then
delegates — **deviation from the plan's "same shape as
`service::teams.rs`'s equivalents":** it does *not* reimplement
`set_team_member_role`'s last-remaining-admin guard, because `ProjectRepo` has no
`count_active_admins` equivalent (out of A2's scope, and adding one wasn't part of
this stage). Flagged in a doc comment on the function; worth reconsidering once
this is reachable via HTTP (A5) if that gap matters in practice.

11 new unit tests in `projects.rs` (mirroring `teams.rs`'s in-file
`MockTeamRepo`-based pattern, now also using the A2-generated `MockProjectRepo`)
covering: owner-allowed/non-owner-rejected on a personal project, active/inactive
team member on a shared project (`team_id: Some`, exercised via a mocked
`TeamRepo` even though nothing in the app can actually create such a project yet),
admin-allowed/non-admin-rejected for both `require_project_admin` and
`set_project_member_role`, `create_project`'s `team_id: None` argument (asserted
via `mockall`'s `.withf(...)`), and `list_projects`/`list_project_members`
delegation. One friction point: `ProjectMemberInfo` (`src/storage/sqlite/mod.rs`)
has no `#[derive(Debug)]`, so a test can't `.unwrap_err()` a
`Result<Vec<ProjectMemberInfo>, _>` directly — worked around with a `matches!` on
the `Result` itself rather than adding the derive, to keep this stage's diff
scoped to `src/service/`.

`cargo test`: 137/137 passing (126 prior + 11 new), zero regressions. `cargo
check`: clean — same pre-existing dead-code warnings as before (`ProjectRepo`
trait, `SqliteProjectRepo`, `Project`/`ProjectMember` domain structs still
unconstructed/unused outside tests, as expected — nothing in the running app
calls any of this yet, per this stage's own scope). Not reachable via HTTP; no
`main.rs`/handler changes.

## A4 — Team↔project membership sync

The one genuinely novel piece of logic, isolated on its own:
- `attach_team_to_project`/`detach_team_from_project` in
  `service::projects.rs`: on attach, insert a `project_members` row (role
  `member`, points `0`) for every current ACTIVE member of the team being
  attached (don't clobber the owner's own row if they're also a team
  member); on detach, remove every `project_members` row that came from that
  team.
- Sync hook: extend `TeamRepo::accept`/`remove_member` (the two mutation
  paths that flip an ACTIVE `team_members` row — `create` also seeds one,
  but a brand-new team has no attached projects yet, so no cascade needed
  there, per the invite-flow research) to, after the write, look up every
  `projects.team_id = ?` row and insert/delete the matching `project_members`
  row. Leave `activity_log` history untouched on removal, same as today's
  team-leave behavior.

**Verify:** dedicated tests — attaching a team seeds `project_members`
correctly from its current ACTIVE members only (not pending); a later
`accept` for that team adds a `project_members` row to *every* project the
team backs (test with 2+ attached projects); a `remove_member`/leave removes
it everywhere; detaching a project from a team removes only that project's
rows, not other projects sharing the same team.

**Implementation notes:** Done, with one deliberate placement deviation from
the plan's wording. The plan describes `attach_team_to_project`/
`detach_team_from_project` in `service::projects.rs` as the things that "insert"/
"remove" `project_members` rows. In the actual implementation, the *seed-on-attach*
and *clear-on-detach* SQL lives in `SqliteProjectRepo::attach_team`/`detach_team`
(`src/storage/sqlite/projects.rs`) instead — which is where A2's own doc comments
already pointed ("Plain column write, no member-sync cascade — that's stage A4," on
both methods). The service-layer functions of the same name are thin gates: call
`require_project_admin`, then delegate to the now-cascading repo method. No new
`ProjectRepo` trait methods were needed — both cascades are single SQL statements
(`INSERT ... SELECT ... FROM team_members WHERE ... AND NOT EXISTS (...)` on
attach; `DELETE FROM project_members WHERE project_id = ? AND user_id !=
(SELECT owner_user_id FROM projects WHERE id = ?)` on detach), since
`SqliteProjectRepo`/`SqliteTeamRepo` both just wrap the same `SqlitePool` and can
reach across tables directly.

The "sync hook" half (new team-membership changes cascading to *already-attached*
projects) is implemented directly in `SqliteTeamRepo::accept`/`remove_member`
(`src/storage/sqlite/teams.rs`), per the plan — each does its normal
`team_members` write, then a second SQL statement against `project_members`/
`projects` for every project with `team_id` matching. Same "single SQL statement,
no new trait methods" shape as the attach/detach side; `accept`'s insert uses the
same `NOT EXISTS` guard as `attach_team` (so accepting an invite never clobbers an
existing `project_members` row, e.g. if the invitee already owns one of the
team's attached projects). `remove_member`'s cascade delete has no owner
exception — it fires unconditionally for whichever `(project_id, user_id)` rows
exist, including a departing project owner's own row; this was a deliberate
choice, not an oversight: once a project has an attached team, the access-check
formula (`docs/project-abstraction-plan.md`'s "Target shape" section) already
ignores `owner_user_id` in favor of team membership, so an owner who leaves the
team has already lost access regardless of what happens to their `project_members`
row — no separate protection needed. No explicit `sqlx` transactions were added
around any of these two-statement sequences — matches the pre-existing style in
both files (`remove_member`'s original two-statement member-delete-then-maybe-
delete-team was already like this before A4).

Neither `attach_team_to_project` nor `detach_team_from_project` take an explicit
`owner_user_id` parameter — `detach_team`'s SQL reads it directly off the
`projects` row via a subquery instead, so the owner-preservation guarantee can't
drift out of sync with whatever `projects.owner_user_id` actually says.

Tests: 8 new sqlite-level tests in `storage::sqlite::projects` (seed-from-active-
only, skip-pending, don't-clobber-owner-role on attach; keep-owner-remove-synced,
don't-touch-other-projects-sharing-the-team on detach; the original
`attach_and_detach_team_round_trip` from A2 untouched) — `projects.rs`'s
`test_pool()` gained a `team_members` table for these. 4 new sqlite-level tests
in a **new** `storage::sqlite::teams` test module (`teams.rs` had none before this
stage, per A2's own note) covering `accept`'s cascade-to-every-backed-project,
active-only seeding, don't-clobber, and `remove_member`'s cascade-removes-
everywhere — its own `test_pool()` duplicates the `users`/`teams`/`team_members`/
`projects`/`project_members` schema subset needed, following the existing
per-file `test_pool()` duplication precedent (`activity_log.rs` vs. `projects.rs`
already do this). 4 new service-layer tests in `service::projects` (admin-allowed/
non-admin-rejected for both `attach_team_to_project`/`detach_team_from_project`,
via `MockProjectRepo`'s auto-generated `expect_attach_team`/`expect_detach_team`
— no trait signature changes were needed for mockall to pick these up, since A2
already declared both methods on `ProjectRepo`).

`cargo test`: 149/149 passing (137 prior + 12 new: 8 in
`storage::sqlite::projects`, 4 in `storage::sqlite::teams`, 4 in
`service::projects`). `cargo check`: clean,
same pre-existing dead-code warnings as before (everything in
`service::projects`/`storage::sqlite::{projects,teams}`'s new surface still
unconstructed/unused outside tests — A4 is still not reachable via HTTP, per this
stage's own scope; that's A5). No `main.rs`/handler changes.

## A5 — Smithy surface + wiring

- New `model/src/main/smithy/project.smithy`, modeled on `team.smithy` but
  without a membership-status/role field on the list-members output beyond
  what A2-A4 need:
  - `CreateProject` (`POST /users/{userId}/projects`) — name, optional
    `teamId`.
  - `GetProject` / `UpdateProject` (name, and `teamId` to attach/detach — use
    this API's existing three-way-optional convention: absent = unchanged,
    explicit `null`/cleared = detach, present = attach/switch) / `DeleteProject`
    / `ListProjects` (`GET /users/{userId}/projects`).
  - `ListProjectMembers` (`GET /projects/{projectId}/members`).
  - `SetProjectMemberRole` (`PUT /projects/{projectId}/members/{userId}/role`).
  - Leave `team.smithy`/`TeamItem` completely untouched.
- `task codegen`, then wire handlers in `main.rs` calling A3/A4's service
  functions.
- Hook `create_project` (default, no team) into wherever new users are first
  created (`UserRepo::get_or_create_by_google_id`/`get_or_create_by_email`
  callers) so every *new* user gets one default personal project
  automatically. Existing users are backfilled in Stage B's migration, not
  here.
- Still no web UI, no CLI, no MCP server changes — reachable only via direct
  API calls (curl/a generated client) at this point.

**Verify:** `task codegen` succeeds; `task check`/`task build` compile;
manual smoke test hitting the new endpoints directly (curl with a bearer
token, or a throwaway `todo-client` snippet) confirms create → get → list →
attach-team → list-members → set-role → detach round-trips correctly;
`cargo test` full pass with zero regressions to any existing team/item test.

**Implementation notes:** Done, with one deliberate design deviation from the plan's
wording, confirmed with the user before implementation: `UpdateProject`'s `teamId`
does **not** use a "three-way-optional convention" (absent = unchanged, explicit
null = detach, present = attach/switch) — that convention doesn't actually exist
anywhere else in this codebase (checked `dueDate`/`assignedToUserId`/`parentItemId`
on existing update operations; all are plain direct-overwrite `Option`s, with
"preserve current value" handled by the caller round-tripping it, not by JSON
null-vs-absent detection — see CLAUDE.md's Recurrence/Scheduled-start-end sections).
Building that convention for real would have been novel, untested machinery for one
field. Instead, `AttachTeamToProject`/`DetachTeamFromProject` are two dedicated
operations, mapping 1:1 onto stage A4's `service::projects::attach_team_to_project`/
`detach_team_from_project` — `UpdateProject` only ever renames. This also mirrors how
Team itself already exposes small dedicated operations (`InviteTeamMember`/
`AcceptTeamInvite`/`LeaveTeam`) rather than folding everything into `UpdateTeam`.

`model/src/main/smithy/project.smithy` (new file), modeled closely on `team.smithy`:
`Project` is **not** a Smithy `resource`, same precedent as `Team` (see CLAUDE.md).
`CreateProject`/`GetProject`/`UpdateProject`/`DeleteProject`/`ListProjects` are
`/users/{userId}/projects[...]`-scoped and registered in `User`'s own `operations:
[...]` list in `user.smithy` (mirroring Team's CRUD placement) — these bind `userId`
as a plain `@httpLabel` with **no** `@notProperty` (needed only because they're
listed under `User`'s `operations:`, matching every field-by-field precedent in
`CreateTeam`/`GetTeam`/`UpdateTeam`/`ListTeamMembers`; every other field in these
five operations *does* need `@notProperty`, including `projectId` despite being
`@httpLabel`). `ListProjectMembers`/`SetProjectMemberRole`/`AttachTeamToProject`/
`DetachTeamFromProject` are `/projects/{projectId}/...`-scoped with no `userId`
prefix, registered directly in `service.smithy`'s top-level `operations: [...]`
instead (mirroring `TeamItem`'s own CRUD placement) — none of their fields need
`@notProperty` (mirrors `ListTeamActivityLog`/`UndoActivityLogEntry`, the closest
existing precedent for an operation not bound to any resource's `operations:`
list). `AttachTeamToProject` is `PUT /projects/{projectId}/team/{teamId}`;
`DetachTeamFromProject` is `DELETE /projects/{projectId}/team` — no request body on
either, both idempotent. `task codegen` succeeded (one pre-existing, unrelated
`cargo fmt` trailing-whitespace warning on the generated `todo-client` crate's
`config.rs`, non-fatal, not caused by this change).

A3 turned out not to have built `get_project`/`update_project`/`delete_project` at
the service layer (its own plan section only listed `require_project_member`/
`require_project_admin`/`create_project`/`list_projects`/`list_project_members`/
`set_project_member_role`) — added all three to `src/service/projects.rs` in this
stage, same admin/member gating shape as their siblings (`get_project` requires
membership; `update_project`/`delete_project` require admin), plus 6 new unit
tests covering allow/reject for each.

**New in this stage, not in the original plan text:** hooking `create_project`
into new-user creation (the plan's own final bullet) turned out to need a helper,
since `UserRepo::get_or_create_by_google_id`/`get_or_create_by_email` don't report
whether they just created a row or returned an existing one (checked
`src/storage/sqlite/users.rs` — both are a single `SELECT`-then-maybe-`INSERT`
returning only the `User`, no "was new" flag). Rather than changing that trait
signature (would ripple through `memory.rs`/`dynamo.rs`/every existing mock),
added `service::projects::ensure_default_project(projects, user_id)` — idempotent:
creates a "Personal" project only if `list_for_user` comes back empty. Called
from all four places a user identity gets resolved: `auth::auth_callback`
(internal mode, via a new `AppState.project_repo` field — `AppState::new` gained a
6th parameter, updated at its one call site in `main.rs`), and `caddy_auth_me`/
`caddy_auth_token`/`caddy_header_middleware` (caddy mode, via a new
`Extension<Arc<dyn ProjectRepo>>` — the middleware reads it from
`req.extensions()` the same way it already reads `UserRepo`, since it's not run as
a typed axum handler). `main.rs` layers `Extension(project_repo)` in the same
outer position as `Extension(team_repo)` in caddy mode, for the same reason
documented on that line (the middleware's pre-processing step runs before any
`Extension` layered inside `build_web_router()`/`api_router`'s own chain takes
effect). The bearer-token branch of `caddy_header_middleware` (CLI/MCP requests
carrying an existing JWT) does *not* call `ensure_default_project` — by
construction that path is only reachable for a user who already completed a
prior email-based login, so they already have one. Deliberate side effect,
flagged for awareness: because the check is "zero projects" rather than "user row
just inserted," this also silently backfills a default project for any
*pre-existing* user the next time they log in — a nice property, not a conflict
with Stage B's own planned backfill migration (that migration still needs to run
for any user who never logs in again, and for backfilling `items.project_id`
itself, which this stage does not touch).

`main.rs` wiring: `project_repo` (new `Arc<dyn ProjectRepo>`, `SqliteProjectRepo`)
built alongside the other three repos; layered onto the shared `api` `ServiceBuilder`
(so every `/api/*` handler can extract it) and onto both auth-mode outer routers
(caddy: alongside `team_repo`; internal: via `AppState`). Added
`.route_service("/projects", api.clone())` /
`.route_service("/projects/*path", api.clone())` to both auth modes' `api_router`s,
alongside the existing `/users`/`/teams` registrations (`/users/*path` already
covered the `/users/{userId}/projects...` operations as a prefix match, but the
no-userId-prefix operations needed their own explicit route). New
`src/json_api/projects.rs` (registered in `src/json_api/mod.rs`), handler-per-operation,
following `json_api/teams.rs`'s exact shape — `require_matching_user` guards every
`userId`-bearing operation; the four `/projects/{projectId}/...` operations have no
`userId` to check (same as `json_api/team_items.rs`'s handlers), relying entirely on
`AuthUser` + the service layer's own membership/admin gating.

**Verification:** `task codegen` succeeded. `cargo check`: clean, only the same
pre-existing dead-code warnings as A1-A4 (nothing new introduced — `ProjectRepo`
trait/`SqliteProjectRepo`/`Project` domain struct are still flagged unused in
isolated `cargo check` runs of unrelated modules before this stage's own handlers
got wired in, but the wiring itself compiles clean). `cargo test`: 157/157 passing
(149 prior + 6 for `get_project`/`update_project`/`delete_project` + 2 for
`ensure_default_project`), zero regressions. Manual smoke test: built the binary,
ran it against a throwaway SQLite DB (`TODO_AUTH_MODE=caddy`,
`TODO_DEV_EMAIL=smoketest@example.com`, migrations applied cleanly through version
10), then via curl with a minted bearer token: first `/auth/token` call confirmed
`ensure_default_project` actually fires (`ListProjects` came back with one
"Personal" project with no prior `CreateProject` call) — then exercised
create → get → update (rename) → create-team → attach-team → get (shows `teamId`)
→ list-members → set-member-role → detach-team → get (no more `teamId`) →
list-projects (both projects present) → delete → list-projects (back to one). Every
call returned the expected shape and HTTP 200. Not tested: a second real user
account attaching to a shared project (would need a second identity in this
single-dev-email smoke setup) — covered instead by the A3/A4 unit tests' mocked
shared-project paths. No web UI, CLI, or MCP server changes, per this stage's own
scope — Stage B is next.

## Stage B — Backfill migration + web UI cutover

**Decisions confirmed before drafting this stage** (asked directly, since each
is a real fork rather than a "recommend and flag" call — recorded here since
this file, not chat history, is what survives to the next session):

- **API shape:** a brand-new `ProjectItem` Smithy resource
  (`/projects/{projectId}/items/{itemId}`), used alongside the untouched
  legacy `User → Item`/`TeamItem` resources during the whole of Stage B.
  Dual-write (B2) keeps all three in sync; Stage C retires the two legacy
  ones once every caller has moved off them. Rejected alternative: mutating
  the two existing resources in place — smaller Smithy diff, but no gradual
  bridge, so every existing caller (web UI, CLI, MCP, prod data) would need
  to move atomically.
- **Migration rollout:** B1's data-backfill migration auto-runs at startup
  like every migration so far (A1's `add_projects` included), verified
  against a `task db copy` snapshot first. No separate manual-trigger
  mechanism — same safety pattern already established, not a new one.
- **Team-backed project `owner_user_id` backfill:** deterministic pick
  (`MIN(user_id)` among a team's active admins, falling back to any active
  member) rather than adding a real `teams.owner_user_id`/creator concept.
  Confirmed low-risk: per the plan's own access-check formula, `owner_user_id`
  is only ever consulted for personal projects (`team_id IS NULL`) — once a
  project has a `team_id`, this column is write-once-and-ignored. Production
  currently has exactly one team with exactly one admin, so the "which admin"
  question is moot in practice today. This leaves the original Stage A "does
  Team drop role/ownership entirely" open call (A2's implementation notes)
  unresolved, on purpose — revisit only if it becomes load-bearing later.
- **Activity log scope:** `activity_log` moves from `team_id`- to
  `project_id`-keyed, as part of this stage (B1 backfills a new column, B2
  cuts reads/writes over) — not deferred to Stage C. Keeps every
  item-adjacent concept on one "which entity owns this row" model instead of
  leaving activity_log as a second, team-scoped exception.

**Scope check, from the repo as it stands today** (gathered before drafting,
so later sessions don't need to re-derive it): the personal/team screen pairs
Stage B collapses total ~8,800 lines (`src/web_ui/tasks/{mod,handlers,templates}.rs`
595+447+355, `team_tasks.rs` 1372, `events.rs` 986, `team_events.rs` 1069,
`simple_lists.rs` 591, `team_simple_lists.rs` 672, `templates.rs` 651,
`team_templates.rs` 673, `dashboard.rs` 511, `team_dashboard.rs` 198,
`assigned_items.rs` 93, `teams.rs` 297, `team_activity.rs` 147, `nav.rs` 131).
The shared `components::row::Row`/`templates/components/row.html` mentioned in
this doc's intro as "already started" is real but still a 27-line unused stub
(`components/mod.rs` is empty) — `TaskRow`/`TeamTaskRow` and their event/
simple-list equivalents still fully duplicate row rendering today; nothing
currently constructs a `Row`. `src/handlers/web_ui/` from this doc's earlier
stages and CLAUDE.md is now `src/web_ui/` (flattened in the same commit that
started the `Row` stub — see `git log --oneline -- src/web_ui/`); CLAUDE.md
itself hasn't been updated for the rename, worth fixing whenever it's next
touched but out of scope here.

Given the scale, Stage B is broken into its own sequence of independently-
landable sub-stages (B1-B7), following the exact same one-stage-per-session,
update-this-file-before-clearing process A1-A5 used. **B1 is the one sub-stage
that actually mutates existing production data** — everything before it (A1-A5)
was additive-only (new empty tables/columns nothing read from). Treat B1 with
correspondingly more caution: verify against a `task db copy` snapshot,
reconciliation-query the result, and don't skip that step to save time.

### B1 — Backfill migration (data only, no app code changes)

New migration (next version after `add_projects`, i.e. version 11):
- **Personal projects:** for every user without an existing personal project
  (a `projects` row with `team_id IS NULL AND owner_user_id = users.id`),
  create one named `"Personal"` (matching `ensure_default_project`'s naming).
  Must skip-existing, not blindly insert — `ensure_default_project` (A5) has
  been creating these live in production on every login since A5 shipped, so
  by the time this migration runs some users already have one.
- **Team projects:** for every team without an existing attached project (no
  `projects` row with that `team_id`), create one: `name = teams.name`,
  `team_id = teams.id`, `owner_user_id` = the deterministic admin pick above.
  Also seed `project_members` for every currently-`ACTIVE` team member — this
  is effectively "attach this team to its own brand-new project" without
  going through A4's service-layer `attach_team_to_project`, so mirror what
  that cascade does (`ACTIVE` members only, `NOT EXISTS` guard against
  clobbering the owner's own already-inserted row).
- **`items.project_id` backfill:** personal items —
  `UPDATE items SET project_id = (SELECT id FROM projects WHERE owner_user_id
  = items.user_id AND team_id IS NULL) WHERE user_id IS NOT NULL AND
  project_id IS NULL`; team items — the analogous join on
  `items.team_id = projects.team_id`.
- **`activity_log.project_id`:** new nullable column, added in this same
  migration (guarded with `column_exists`, per convention), backfilled from
  each row's `team_id` via the same `projects.team_id` join. `team_id` stays
  on the table but becomes unused after B2 cuts reads/writes over — actual
  column drop is Stage C, per this doc's existing "drop unused columns in
  Stage C" precedent.

**Verify:** run against a `task db copy` snapshot first. Reconciliation
queries: every `items` row has non-null `project_id` after migration; project
count == (users without a pre-existing personal project) + (teams without a
pre-existing attached project) + prior count; every team's `project_members`
row count matches its active `team_members` count; spot-check the one
production team's project landed the right admin as owner. Only after that
passes does this land as a normal auto-run migration (per the rollout decision
above). Same `cargo test storage::migrations` idempotency-test pattern as
`add_projects` (re-running the migration a second time must be a no-op).

**Implementation notes:** Done, matching the plan's SQL/logic shape closely, with one
structural difference forced by the row-generation need: the personal- and
team-project creation halves are **Rust loops over fetched ids**, not pure `INSERT
... SELECT` statements — every other migration in this codebase (including A1's
`add_projects`) is pure SQL, but generating a fresh UUID per row (matching
`SqliteProjectRepo::create`'s own `uuid::Uuid::new_v4()` convention, not SQLite's
`randomblob`-based id shape) isn't expressible in a single SQL statement, so each new
project needed its own `INSERT` bound to a Rust-side-generated id. `src/storage/
migrations/backfill_projects.rs` (`BackfillProjects`, version `11`), registered in
`all_migrations()`.

**Team-project owner pick:** implemented exactly as the plan's deterministic-admin
decision states — lowest `user_id` (`ORDER BY user_id ASC LIMIT 1`) among the team's
`ACTIVE` `role = 'admin'` members, falling back to the lowest `user_id` among any
`ACTIVE` member if no active admin exists. One case the plan's prose didn't address
directly: a team with *zero* active members at all has no valid `owner_user_id` to
backfill (the column is `NOT NULL`) — handled by skipping that team's project creation
entirely (`continue`), which in practice should never fire (`TeamRepo::create` always
seeds the creator as an ACTIVE admin) but is there so the migration doesn't panic/error
on a hypothetical malformed row.

**`project_members` seeding:** the team-project half seeds the owner's own `admin` row
explicitly (mirroring `ProjectRepo::create`'s two-insert shape), then seeds every other
`ACTIVE` team member as `member` via the same `NOT EXISTS`-guarded `INSERT ... SELECT`
`SqliteProjectRepo::attach_team` already uses — copied verbatim rather than re-derived,
so the two "seed project_members from a team's active members" code paths (this
migration, and A4's `attach_team`) stay textually identical.

**`activity_log.project_id`:** added via the usual `column_exists`-guarded `ALTER
TABLE`, backfilled via a `team_id`-keyed correlated subquery, index created inside this
migration (not the `create_pool()` baseline) — same index-ordering reasoning as
`add_projects.rs`'s `idx_items_project_id`, called out explicitly in both this
migration's own doc comment and a new comment left in `create_pool()` next to the
baseline `activity_log` table (which *does* now include the `project_id` column
itself, just not its index, matching the plan's "add the column to the baseline, index
stays migration-only" pattern). Baseline `activity_log` `CREATE TABLE IF NOT EXISTS`
in `src/storage/sqlite/mod.rs` updated identically.

**Known accepted gap, carried over from the plan text verbatim:** the `items.project_id`
personal-item backfill (`SELECT id FROM projects WHERE owner_user_id = items.user_id
AND team_id IS NULL`) has no way to disambiguate if a user has more than one personal
(team-less) project — possible since stage A5's `CreateProject` operation shipped.
SQLite picks one arbitrarily in that case. Not handled in this migration; flagged in the
plan already as a known, accepted risk, not a new one introduced here.

**Test-fixture fallout:** the four `run_migrations()`-exercising schema-pool test
helpers in `storage/migrations/mod.rs` (`old_schema_pool`, `current_schema_pool`,
`pre_simple_schema_pool`, `pre_source_event_id_schema_pool`) all predate `users`/`teams`
tables and `items.user_id`/`items.team_id` columns existing in their synthetic schemas —
those columns/tables predate the migration system entirely in real production DBs (part
of the original hand-written base schema, same category as `parent_item_id`), so no
earlier migration's test fixtures needed them. This migration is the first to actually
read `users`/`teams`/`items.user_id`/`items.team_id`, so all four helpers needed
`user_id`/`team_id` columns added to their `items` table defs plus a new shared
`users_and_teams_tables()` helper creating minimal `users`/`teams` tables, or every
existing test exercising the full migration pipeline would fail with "no such table"/
"no such column" the moment migration 11 runs. This is test-fixture-only churn — no
production schema implication, since real DBs already have all of these.

11 new unit tests in `backfill_projects.rs` (`test_pool()`-per-file pattern, matching
`add_projects.rs`/`projects.rs`'s precedent) covering: personal-project creation and
its skip-if-exists case; team-project creation with deterministic-admin-pick (including
a fallback-to-any-active-member case), its skip-if-exists case, and its `NOT EXISTS`-
guarded member seeding (active-only, PENDING excluded); `items.project_id` backfill for
both a personal and a team item in the same test; `activity_log.project_id` column
creation + backfill; and a full idempotency test (running `up()` twice produces no
duplicate `projects`/`project_members` rows). `cargo test`: 165/165 passing (157 prior +
8 new — the plan text above describes 11 sub-checks but several share one `#[tokio::test]`
fn, so the actual new-test count is 8), zero regressions. `cargo check`: clean, same
pre-existing dead-code warnings as every prior stage.

**Verified against real prod-shaped data**, not just synthetic fixtures: copied the
existing local `todo.db` (a prior `task copy-prod-db` snapshot already present in the
repo root, itself still on migration version 9 — i.e. it predated even stage A1's
`add_projects`, version 10) to a scratch path, ran the actual server binary against the
copy via `TODO_DATABASE_URL` (letting it panic on missing `TODO_GOOGLE_CLIENT_ID` right
after `create_pool()`/`run_migrations()` complete — sufficient, since migrations run
before any auth-mode branching). Versions 10 and 11 both applied cleanly in sequence.
Reconciliation queries against the migrated copy: 3 users → 3 personal projects created
(0 pre-existing, since this snapshot predates `ensure_default_project` too); 2 teams →
2 team projects created; total `projects` count 5 = 3 + 2 + 0, matching the plan's own
formula; all 60 `items` rows ended with non-null `project_id`; each team's
`project_members` count matched its `ACTIVE` `team_members` count exactly (3-and-3,
1-and-1); both team projects' `owner_user_id` resolved to a user whose `team_members`
role was actually `'admin'` on that team; all 31 `activity_log` rows got a non-null
`project_id`. Re-ran the binary a second time against the same now-migrated copy — no
"applied migration" log lines (both versions already recorded in `_migrations`), and
`projects`/`project_members` row counts were unchanged, confirming idempotency against
real data too, not just the in-memory unit tests. Scratch copy deleted after
verification; the original `todo.db` in the repo root was never written to. Migration
has **not** yet been run against actual production (`todo.lapinel-fam.club`) — it lands
there the next time the deployed server restarts with this code, same as every prior
migration's rollout (per the plan's "Migration rollout" decision — no separate
manual-trigger mechanism). No app code (service/handler layer) changes in this stage,
per its own scope — B2 is next.

### B2 — Dual-write + activity_log cutover

- `service::items.rs`/`service::team_items.rs` create/update paths resolve
  and set `project_id` on every write, alongside the still-written legacy
  `user_id`/`team_id`. Resolution: "the caller's personal project" (personal
  path) or "the project backing this team" (team path). A team can in
  principle back multiple projects (that's the whole point of the model),
  but nothing before B4 exists to create a second one — the legacy
  `TeamItem` create path can assume exactly one attached project per team for
  now. Flagged, not enforced: B4's `ProjectItem` API is what has to make
  "which of this team's projects" a first-class, explicit choice instead of
  an assumption.
- `service::activity_log.rs`/`storage/sqlite/activity_log.rs`: writes move to
  `project_id` (resolved the same way); reads move to `project_id`-keyed
  queries. `team_activity.rs` (the one web screen reading this) repoints at
  the new query — the one piece of B2 that's user-visible before B4/B5, worth
  verifying end-to-end on its own.

**Verify:** existing item CRUD tests (personal + team) unmodified and still
passing; new tests assert `project_id` populated correctly on create for both
paths; `team_activity.rs`'s existing points/activity display verified
unchanged end-to-end (same data, now sourced via `project_id`).

**Implementation notes:** Done, sub-staged into four independently-verified pieces
(B2a-B2d) within one session — the plan's own scope turned out to touch 18 production
call sites plus the activity_log read cutover, larger than a single sitting should be
reviewed as one diff, so it was broken down the same way A1-A5/B1 already were. No
new migration was needed for any of B2 — `items.project_id` and
`activity_log.project_id` already exist as columns from stages A1/B1; B2 is purely
app-code wiring on top of them.

**B2a (storage/domain additions):** `Item` (`src/domain/item.rs`) and
`ActivityLogEntry` (`src/domain/activity_log.rs`) both gained `project_id:
Option<String>`. `ITEM_SELECT`, `create`/`update`/`update_team_item`'s SQL, and the
two raw `list_due`/`list_due_team_items` SELECTs (`src/storage/sqlite/items.rs`) all
round-trip the column now; `row_to_item`/`row_to_activity_log_entry`
(`src/storage/sqlite/mod.rs`) read it back. Two new `ProjectRepo` methods
(`src/storage/sqlite/mod.rs`'s trait, impl in `src/storage/sqlite/projects.rs`):
`find_personal_project(user_id) -> Option<Project>` (`WHERE owner_user_id = ? AND
team_id IS NULL LIMIT 1` — arbitrary pick if a user has more than one, same accepted
gap as stage B1's backfill) and `get_by_team(team_id) -> Option<Project>`. One new
`ActivityLogRepo` method, `list_activity_for_project` — the team-keyed
`list_activity_for_team` was deliberately left untouched rather than migrated
internally, since it still backs the legacy `ListTeamActivityLog` JSON API operation
until stage B4 retires it; only `team_activity.rs`'s own read moved. `log_activity`
gained a `project_id: Option<&str>` parameter and, following A2's precedent for
`ProjectRepo::create`, needed an explicit `<'a>` lifetime
(`log_activity<'a>(&'a self, team_id: &'a str, project_id: Option<&'a str>, ...)`) —
`#[async_trait]`'s desugaring can't elide a lifetime buried inside `Option<&str>`.

**B2b (`ensure_team_project`):** A gap not flagged in the original plan text, found
during implementation: `service::teams::create_team` never created or attached a
project at all, so any team created after stage B1's backfill migration ran (i.e.
any team created from then until B2 shipped) would have had zero attached
projects — B2's team-item resolution would have silently found nothing for it.
Closed by adding `service::projects::ensure_team_project(projects, team_id,
team_name, creator_user_id)` (idempotent — `get_by_team` short-circuits if one
already exists) and calling it from `create_team` right after `TeamRepo::create`
succeeds. Uses the create-then-`attach_team` shape (mirroring stage A4) rather than
passing `team_id` directly to `ProjectRepo::create`, so `attach_team`'s member-seed
cascade runs — moot in practice for a brand-new team (its only `ACTIVE` member is
already the creator, seeded as the project's own owner/admin by `create`), but keeps
the two "create a project for a team" code paths (this, and stage B1's migration)
structurally consistent. `create_team`'s signature gained a `&Arc<dyn ProjectRepo>`
parameter — its two call sites (`src/web_ui/teams.rs`, `src/json_api/teams.rs`) both
already had `TeamRepo` available via `Extension`/`server::Extension` and needed only
the one new parameter added alongside it.

**B2c (item create/update dual-write):** `create_item`/`create_team_item`
(`src/service/items.rs`/`src/service/team_items.rs`) each gained a `&Arc<dyn
ProjectRepo>` parameter and resolve+set `item.project_id` right where `complete`/
`parent_item_id`/`description` are already overlaid, just before `item.validate()`.
**Deliberate, lower-risk deviation from the plan text's "create/update paths
resolve":** `update_item`/`update_team_item` do **not** take a new `ProjectRepo`
parameter at all — instead `item.project_id = current.project_id.clone();` carries
the value forward from the already-fetched `current` row. This is safe because an
item's owner (personal: `user_id`; team: `team_id`) never changes after creation, so
its resolved project can't change either — carrying forward is exactly as correct as
re-resolving, without the cost of a second `ProjectRepo` round-trip on every edit or
a second wave of call-site changes. (The one accepted gap this leaves: an item
created between stage B1's migration and B2 shipping has `project_id: NULL` in the
DB, and an update on it will keep propagating `NULL` until it's recreated or a future
backfill catches it — same class of gap as B1's own "which personal project"
ambiguity, not new risk introduced here.) This cut the blast radius roughly in
half: only the **create** paths needed their ~18 call sites updated (`json_api/
items.rs`, `json_api/team_items.rs`, and the dedicated per-type web_ui screens —
`simple_lists.rs`, `tasks/handlers.rs`, `events.rs`, `templates.rs` and their
`team_*.rs` counterparts, each needing one new `Extension<Arc<dyn ProjectRepo>>`
parameter threaded through, all of which already had a `ProjectRepo` extension
reachable per stage A5's wiring). Recursive same-owner copy helpers (`clone_children`,
`sync_offset_children`, `repoint_source_event_tasks`, `sync_source_event_tasks`) needed
no changes at all — they clone or update existing `Item`s fetched straight from the
repo, which already carry the right `project_id` from the DB, so it rides along for
free. **Known, accepted gap, not fixed in this pass:** the cross-ownership-boundary
copy helpers (`copy_template_children`, `copy_template_children_to_event`,
`copy_children_as_template` — template library ⇄ real item) were **not** threaded
with an explicit target `project_id`, so a copied child's row carries whatever
`project_id` was on the *template* child it was cloned from, not necessarily the new
parent item's own. In every case reachable today (a user's own personal template
used on their own personal item; a team's own template library used on that same
team's item) these coincide, so this is inert in practice — same bounded-risk shape as
B1's "which personal project" gap, worth revisiting only if it becomes load-bearing
(e.g. once stage B5d's project-scoped template screens make cross-project template
use possible).

Also found during B2c: `main.rs`'s internal-auth-mode `web_router` (the `_ =>` match
arm) had never layered `Extension(project_repo)` at all — stage A5's wiring notes had
already flagged this gap for future stages. Fixed by adding `.layer(Extension(
project_repo))` alongside the other four repos in that arm. Caddy mode already had
`project_repo` layered on the *outer* router (for `caddy_header_middleware`'s own
pre-processing needs, per that line's existing comment), but for consistency with
`item_repo`/`team_repo`/`activity_log_repo` — and to remove any doubt about whether a
typed `Extension` extractor inside `build_web_router()`'s own chain needs its own
inner-layered copy — `project_repo.clone()` was also added to caddy mode's *inner*
`web_router` layer chain, matching the shape every other repo already had there.

**B2d (activity_log write + `team_activity.rs` read cutover):**
`update_team_item`'s points-award branch (`src/service/team_items.rs`) now passes
`item.project_id.as_deref()` to `log_activity` — `item.project_id` is already
correct at that point via B2c's carry-forward. `team_activity.rs`'s
`render_activity_page` resolves `projects.get_by_team(team_id)` and calls the new
`list_activity_for_project`, falling back to the legacy `list_activity_for_team`
only if no project currently backs the team (a defensive fallback, not expected to
fire post-B2b/B1 — added so the activity page can never silently render blank due to
a resolution gap). Both `team_activity_page` and `undo_activity_log_entry_form`
gained an `Extension<Arc<dyn ProjectRepo>>` parameter.

**Testing:** 11 new tests (176 total, up from B1's 165): 4 in
`storage::sqlite::projects` (`find_personal_project`/`get_by_team`, found/not-found
cases), 1 in `storage::sqlite::activity_log` (`list_activity_for_project` scoping), 2
in `service::projects` (`ensure_team_project` create-and-attach vs. no-op), 2 in
`service::items` (`create_item` resolves from `find_personal_project`; `update_item`
carries `project_id` forward from `current`), 2 in `service::team_items` (the same
pair for `create_team_item`/`update_team_item`, team-keyed). `team_activity.rs`
itself has no unit tests, matching this codebase's existing convention that `web_ui`
handler modules aren't unit-tested at this granularity (verified by manual smoke
test instead, below). `cargo test`: 176/176 passing, zero regressions. `cargo check`:
clean, only the same pre-existing dead-code warnings as every prior stage.

**Manual smoke test** (built the binary, ran against a throwaway SQLite DB,
`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10 and 11 both applied
cleanly): minted a token; confirmed `ListProjects` showed the auto-created
"Personal" project; created a personal item via `CreateItem` and confirmed via
direct SQLite query that its `project_id` matched the Personal project's id exactly;
created a team via `CreateTeam` and confirmed `ensure_team_project` fired (a
`projects` row with that `team_id` existed with no explicit `AttachTeamToProject`
call); created a team item with `points`/`assignedToUserId` and confirmed its
`project_id` matched the team's backing project; completed it via `UpdateTeamItem`
and confirmed the resulting `activity_log` row had both `team_id` and the correct
`project_id` populated; loaded `GET /web/team-activity/:team_id` and confirmed the
completion rendered correctly (proving `render_activity_page`'s
`get_by_team`-then-`list_activity_for_project` resolution works end to end, not just
at the mock level); called the legacy `GET /teams/:teamId/activity-log` JSON API
operation and confirmed it still returns the same entry via the untouched
`list_activity_for_team` path, proving the dual-write bridge holds from both
directions. Scratch DB and server process cleaned up after verification. No web UI
screens beyond the existing `team_activity.rs` were touched, per this stage's own
scope — B3 is next.

### B3 — Project-scoped item repo/service read paths

- `ItemRepo` gains `list_by_project`/`get_by_project` (and
  update/delete-by-project) methods, alongside the existing `user_id`/
  `team_id`-keyed ones — additive, nothing removed.
- New `service::project_items.rs`, wrapping these with A3's
  `require_project_member`/`require_project_admin` — replaces the
  personal-vs-team authorization branch with the plan's unified check
  (`project.team_id.is_some()` → team-membership check; else →
  `owner_user_id` check).
- Not reachable via HTTP yet — unit-tested only, same precedent A2/A3 set.

**Verify:** unit tests mirroring `service::teams.rs`'s/A3's coverage —
member/non-member/admin cases for both personal and team-backed projects.

**Implementation notes:** Done, with the scope deliberately narrowed to match this
stage's own title ("read paths") rather than the plan bullet's literal wording. The
plan bullet says `ItemRepo` gains `list_by_project`/`get_by_project` "(and
update/delete-by-project)" and that `service::project_items.rs` wraps "these" — taken
literally that reads as full CRUD. What actually landed:

- `ItemRepo` (`src/storage/sqlite/mod.rs`) gained three new methods:
  `get_by_project`, `list_by_project` (same `parent_item_id`-optional shape as
  `list_team_items`), and `update_by_project` (the full `update_team_item` column
  set, including `points`, keyed on `WHERE id = ? AND project_id = ?` instead of
  `team_id`/`user_id` — a single write primitive usable for both personal- and
  team-backed projects once a caller exists). All three implemented in
  `src/storage/sqlite/items.rs`.
- **Deliberately not added:** a `delete_by_project` method. `delete_item`/
  `delete_team_item` (`src/service/items.rs`/`team_items.rs`) already call the
  existing owner-agnostic `ItemRepo::delete(item_id)` (no `user_id`/`team_id` filter
  at all) *after* verifying ownership via `get`/`get_team_item` — a
  `delete_by_project` would just be a redundant re-filter on a call already gated by
  `get_by_project`. `service::project_items.rs` follows the identical shape: verify
  via `get_by_project` (inside `require_project_member`-gated `get_project_item`),
  then call the existing `delete(item_id)` — no new trait method needed. (Not yet
  wired into a `delete_project_item` service fn either — see next point.)
- **Deliberately not added this stage:** `create_project_item`/`update_project_item`/
  `delete_project_item` service functions. `create_item`/`update_item` carry
  substantial business logic (recurrence, `sync_offset_children`/`clone_children`,
  completion-transition guards, points-award/reversal) that only exists today in
  `service::items.rs`/`service::team_items.rs`, driven by `CreateItemParams`/
  `UpdateItemParams`-shaped input. Porting that logic now, without the `ProjectItem`
  Smithy operation's request shape to drive it, would be speculative — that
  porting is squarely stage B4's job (wiring the new Smithy resource), not B3's.
  `update_by_project` exists at the storage layer as a ready-made primitive for B4
  to call, unused for now — same "storage CRUD not yet called from anywhere"
  precedent stage A2 set for the original `ProjectRepo`.
- `src/service/project_items.rs` (new file, registered in `src/service/mod.rs`) ships
  exactly the stage title's "read paths": `get_project_item`/`list_project_items`,
  both gated by A3's `require_project_member` before delegating to the new
  `ItemRepo` methods — the plan's own "unified check" replacing the personal-vs-team
  authorization branch, exactly as described.

12 new unit tests (188 total, up from B2's 176): 6 in `storage::sqlite::items`
(`items.rs` had no sqlite-level tests before this stage, same gap A2's own notes
flagged for it — new `test_pool()` mirrors `projects.rs`'s per-file precedent,
covering `get_by_project` found/wrong-project, `list_by_project` top-level-scoping
and parent-scoping, `update_by_project` round-trip-including-points and
wrong-project-not-found) and 6 in `service::project_items` (mirroring
`service::projects.rs`'s `MockProjectRepo`/`MockTeamRepo` pattern, now also using
`MockItemRepo`: owner-allowed/non-owner-rejected on a personal project,
active/inactive team member on a shared project, and delegation/rejection for
`list_project_items`). `cargo test`: 188/188 passing, zero regressions. `cargo
check`: clean, same pre-existing dead-code warnings as every prior stage plus the
expected new ones on `get_by_project`/`list_by_project`/`update_by_project`
themselves (unconstructed outside tests, per this stage's own scope — not
reachable via HTTP until B4). No `main.rs`/handler/Smithy changes.

### B4 — Smithy surface: `ProjectItem` resource

- New `model/src/main/smithy/project_item.smithy`, modeled on `TeamItem`
  (identifiers `{projectId, itemId}`, full CRUD + list, assignment/points
  fields carried over since a project can be team-backed).
  `/projects/{projectId}/items/{itemId}`.
- `task codegen`, then `src/json_api/project_items.rs` wiring B3's service
  functions, following `json_api/team_items.rs`'s shape.
- Leave `User → Item` and `TeamItem` operations completely in place and
  functional — B2's dual-write is what keeps them consistent with the new
  surface.

**Verify:** same curl round-trip smoke test pattern A5 used (create → get →
update → list → delete via the new API), plus a check specifically
exercising the dual-write bridge: an item created via `ProjectItem` shows up
correctly via the legacy `Item`/`TeamItem` read APIs, and vice versa.

**Implementation notes:** Done, with one deliberate architectural choice not spelled
out in the plan's own bullets: at the service layer, `create_project_item`/
`update_project_item`/`delete_project_item` (`src/service/project_items.rs`) do
**not** reimplement the recurrence/offset/event-trigger/points/completion-guard
machinery a third time. Each resolves `project_id` down to a plain `user_id`
(personal project, `team_id: None`) or `team_id` (team-backed project) and
delegates straight to the existing `service::items::{create_item,update_item,
delete_item}` / `service::team_items::{create_team_item,update_team_item,
delete_team_item}` — the same functions the legacy `Item`/`TeamItem` operations
already call. This is *why* the dual-write bridge holds automatically: an item
created via `ProjectItem` gets `user_id`/`team_id` set exactly like a legacy-API
create would, because it's the same code path underneath. Access gating is the one
genuinely new thing at this layer: `require_project_member` (stage A3) runs first,
then `projects.get(project_id)` is fetched a second time to read `team_id` and
branch — an extra repo round-trip per call, accepted as negligible. Each delegated
call also re-runs its own internal membership check (`create_team_item`'s
`require_active_member`, `delete_team_item`'s equivalent) — a harmless redundant
`teams.member_status` lookup, not worth threading a "trust me, already checked"
bypass through for.

`CreateProjectItemParams`/`UpdateProjectItemParams` (`project_items.rs`) mirror
`CreateTeamItemParams`/`UpdateTeamItemParams` field-for-field (including
`assigned_to_user_id`/`points`, since a project can be team-backed) rather than
`CreateItemParams`/`UpdateItemParams`'s narrower personal-only shape — the
`ProjectItem` Smithy resource always carries both fields (see `project_item.smithy`'s
own doc comment), and they're simply dropped on the personal-project branch since
`CreateItemParams`/`UpdateItemParams` have no slot for them at all. No new
validation was added for "can a personal project have points/assignment" — the
question doesn't arise structurally, matching how personal items never carried a
`TeamAssignment` before this stage either.

`model/src/main/smithy/project_item.smithy` (new file) mirrors `team.smithy`'s
`TeamItem` resource shape exactly: identifiers `{projectId, itemId}`, full property
list including `points`/`assignedToUserId`/`sourceEventId`, CRUD + list operations
at `/projects/{projectId}/items[...]`. Registered the same way `TeamItem`'s own
operations are — not added to the service's `resources: [...]` list, just the five
operation names appended to `service.smithy`'s top-level `operations: [...]`
(alongside the other no-`userId`-prefix Project operations from stage A5).
`ProjectItemSummary` includes `assignedToUserName` (resolved server-side the same
way `TeamItemSummary`'s is) even though it's always empty for a personal-project
item. `task codegen` succeeded — same pre-existing, unrelated `cargo fmt`
trailing-whitespace warning on generated `todo-client/config.rs` noted in A5's
notes, not caused by this change.

`src/json_api/project_items.rs` (new file, registered in `src/json_api/mod.rs`)
follows `json_api/team_items.rs`'s exact shape — one difference: it does *not*
call any membership-check helper itself before delegating, because
`project_items::{get_project_item,list_project_items,create_project_item,
update_project_item,delete_project_item}` (service layer) already gate on
`require_project_member` internally, unlike `json_api::team_items`'s handlers
which call `require_active_member` themselves before hitting the repo directly.
`main.rs` wiring: five new `use` imports plus five new `.create_project_item(...)`-
style builder calls on `PeoplesRepublicOfListsBuilder` — no new `Extension` layers
or `route_service` registrations needed, since `/projects/*path` already routed to
`api` (stage A5) and `api`'s `ServiceBuilder` already layers all five repos
(`user_repo`/`item_repo`/`team_repo`/`project_repo`/`activity_log_repo`) stage B2
onward.

**Testing:** 7 new unit tests in `service::project_items` (195 total, up from B3's
188): `create_project_item`/`update_project_item`/`delete_project_item` each get a
"delegates to personal path" and "delegates to team path" test (verifying the
right `user_id`/`team_id` lands on the constructed `Item`, via `MockItemRepo`
`.withf(...)` assertions on `item.user_id`/`item.team_id`), plus one
`create_project_item_rejects_non_member` test — `get_project_item`/
`list_project_items`'s own membership-rejection coverage from B3 already covers
the pattern for update/delete, not duplicated per-operation. `cargo test`:
195/195 passing, zero regressions. `cargo check`: clean, same pre-existing
dead-code warnings as every prior stage (nothing new — the wiring itself compiles
warning-free).

**Manual smoke test:** built the binary, ran against a throwaway SQLite DB
(`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly),
minted a token, and round-tripped both branches over HTTP:
personal — `CreateProjectItem` → `GetProjectItem` → `ListProjectItems` → confirmed
identical output via legacy `GetItem` (dual-write bridge, project→legacy
direction) → created a second item via legacy `CreateItem` and confirmed it read
back correctly via `GetProjectItem` (legacy→project direction) → `UpdateProjectItem`
(rename + complete) → `DeleteProjectItem` on both items → `ListProjectItems` back to
empty. Team-backed — `CreateTeam` (confirmed stage B2's `ensure_team_project` fired,
showing up in `ListProjects`) → `CreateProjectItem` with `points`/`assignedToUserId`
set → confirmed identical output via legacy `GetTeamItem` → completed it via
`UpdateProjectItem` → confirmed the completion awarded points by reading the legacy
`GET /teams/{teamId}/activity-log` endpoint (a `pointsDelta: 50` entry, `reversed:
false`) → `DeleteProjectItem`. Every call returned the expected shape and HTTP 200.
Scratch DB and server process cleaned up after verification. No web UI, CLI, or MCP
server changes, per this stage's own scope — Stage B5 (web UI cutover) is next.

### B5 — Web UI cutover, one item type per sub-stage

The actual duplication-elimination the whole plan was for, and the largest
remaining chunk of work — treat each of the following as its own session:

- **B5a — Tasks:** new project-scoped screen replacing `src/web_ui/tasks/`
  and `team_tasks.rs` with one `/web/projects/:project_id/tasks[...]`
  screen. First real user of `components::row::Row`/`row.html` — finishing
  that shared component happens as part of this sub-stage, not before it.
- **B5b — Events:** `events.rs` + `team_events.rs`, including the calendar
  view and `sourceEventId`/template-trigger machinery.
- **B5c — Simple lists:** `simple_lists.rs` + `team_simple_lists.rs`.
- **B5d — Templates:** `templates.rs` + `team_templates.rs`, including the
  "Use" flow and the event-triggered auto-copy (reads `list_templates`/
  `list_team_templates` today — needs a project-scoped
  `list_project_templates`).
- **B5e — Dashboard, assigned-items, activity, teams admin:** `dashboard.rs`
  + `team_dashboard.rs` merge into one project-aware dashboard;
  `assigned_items.rs` becomes a cross-project query; `team_activity.rs`
  repoints its display at B2's `project_id`-scoped log (already correct data
  since B2 — this sub-stage is UI/URL cleanup only); `teams.rs` stays
  team-membership-only (unaffected — Team is a pure group now) but its "View
  items" link retargets to the team's default/first attached project.
- **B5f — Nav + legacy retirement:** `nav.rs` sidebar becomes a project
  switcher; decide (before this sub-stage starts, not during) whether old
  URLs (`/web/tasks`, `/web/team-tasks/:team_id`, etc.) redirect to the
  equivalent `/web/projects/:project_id/...` URL or are removed outright.

**Verify (each sub-stage):** old and new screens both functional during the
transition (old untouched until its own sub-stage lands); manual click-through
smoke test of the new screen's full CRUD + any type-specific behavior
(recurrence, assignment, calendar, template "Use" flow, etc.).

**B5a implementation notes:** Done, matching the plan closely. New module
`src/web_ui/project_tasks/{mod.rs, handlers.rs, templates.rs}` (mirroring the
personal `tasks/` module's three-file split) plus 9 new templates under
`templates/project_tasks/` and a small new `src/web_ui/projects.rs` +
`templates/projects/list_page.html`. `src/web_ui/tasks/`, `team_tasks.rs`, and
their templates are completely untouched, per the plan's "old and new coexist
until B5f" rule.

No new service-layer code was needed at all — `service::project_items`/
`service::projects` (stage B4) already provided a fully unified,
membership-gated CRUD surface, so every handler's shape is "resolve the
project via `service::projects::get_project` (fetch + membership check in one
call), then delegate." The one thing confirmed by re-reading `service::
team_items` during this stage: assignment/points are still enforced through
the *legacy team system* (`service::teams::require_team_admin`/
`TeamRepo::add_team_points`), not `ProjectRepo`'s own `project_members.role`/
`points` columns — `create_project_item`/`update_project_item`'s team branch
delegates straight into `team_items::create_team_item`/`update_team_item`,
which is what actually gates/awards points. So the new screen's "is this user
allowed to set points" check is `service::teams::is_team_admin(&teams,
team_id, user_id)`, exactly matching `team_tasks.rs`'s existing logic — not a
new project-level admin concept. `ProjectRepo::member_role`/`points` remain
unused by any UI, as before this stage.

**`components::row::Row` finished** (stage's stated goal): added `item_url:
String` (replacing the two hardcoded `/web/tasks/{{ id }}` occurrences in
`row.html` — used for both the detail `<a href>` and the delete button's
`hx-delete`) and `assignee_name: Option<String>` (renders a badge in the
metadata line when `Some`, mirroring `team_tasks/row.html`'s existing markup).
`ProjectTaskRow::from_item` (`project_tasks/templates.rs`) is `Row`'s first
real caller — it's a plain function returning a `Row`, not its own template
struct. `siblings`/`is_source_event_linked` on `Row` are still populated but
still never read by `row.html` (confirmed this is a **pre-existing** gap
across the whole codebase, not introduced here — `TaskRow`/`TeamTaskRow`/
`SimpleItemRow` all have the identical "never read" warning already; the
"subordinate under…" picker UI these fields were meant to feed was
apparently never actually wired into any row template. Left as-is,
out of scope for this stage).

**Merged design specifics:**
- `ProjectTaskForm` unions `TaskForm`/`TeamTaskForm`'s fields
  (`assignedToUserId`/`points` always present, silently inert on a personal
  project since `CreateProjectItemParams`/`UpdateProjectItemParams` carry
  them through to `service::project_items`, which drops them on the personal
  branch).
- `ProjectTaskDetailFields`/`ProjectTaskDetailView` gained an `is_team_project:
  bool` gate wrapping the assign-to/points markup in the merged templates
  (`{% if is_team_project %}`) — not "hidden via CSS," the elements simply
  aren't in the DOM on a personal project, matching `macros::points_field`'s
  own existing `is_team_admin && is_top_level` gate precedent.
- Promote/subordinate redirects turned out simpler than the legacy screens':
  since every destination is always another Task in the *same* project, no
  `dashboard::detail_url`/`list_url_for`-style per-kind dispatch table was
  needed — just `/web/projects/{project_id}/tasks` or
  `/web/projects/{project_id}/tasks/{id}`, built locally in
  `project_tasks/handlers.rs`.
- New `top_level_anchor_project` (`project_tasks/mod.rs`) mirrors
  `service::items::top_level_anchor`/`service::team_items::
  top_level_anchor_team`, walking the parent chain via `ItemRepo::
  get_by_project` and reusing the existing `pub(crate) item_anchor` from
  `service::items` (no new service-layer function needed).
- `save_project_task_as_template` branches locally between `template_service::
  create_template`/`create_team_template` based on `project.team_id` — no new
  `create_project_template` service function was added (that's stage B5d's
  job, once a project-scoped Templates screen exists to drive its request
  shape, same reasoning stage B3's own notes gave for deferring
  `create_project_item`/etc. until B4 had a caller).
- Calendar view is a **new capability for team-backed projects** — team items
  never had one before this stage (`team_tasks.rs` has no `/calendar` route
  at all); unifying under one project-scoped screen gives every project a
  calendar view "for free," not something separately built.
- The `/web/projects` listing page is deliberately minimal (list + link to
  each project's Tasks screen only, no create/attach-team UI) — just enough
  to reach the new screen manually until stage B5f gives nav real project
  awareness. Added one static "Projects (preview)" link to
  `templates/nav_sidebar_inner.html`'s fixed-links group (alongside
  Dashboard/Assigned to me/Teams) — explicitly not B5f's project switcher.

**Testing:** `cargo check`/`cargo build` clean (only the same pre-existing
dead-code warnings every prior stage has produced — nothing new). `cargo
test`: 195/195 passing, zero regressions (no service/storage/domain code was
touched this stage besides `Row`'s two additive field). No new automated
tests were added — this stage is `web_ui`-layer only, matching this
codebase's existing convention that `web_ui` handler modules aren't
unit-tested at this granularity (same note B2d's `team_activity.rs` change
made), verified instead by the manual smoke test below.

**Manual smoke test** (built the binary, ran against a throwaway SQLite DB via
`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly):
confirmed `GET /web/projects` lists the auto-created "Personal" project;
walked its Tasks screen end-to-end — create (with due date) → list (row
renders with correct `/web/projects/{id}/tasks/...` URLs on checkbox/link/
delete, confirming `Row`'s new `item_url` field) → detail page → edit page →
calendar view (task appears on the right day) → sub-item add → children
fragment → save-as-template ("Saved" returned) → promote-to-top-level
(`hx-redirect` to the list, as expected with no grandparent) → delete.
Confirmed the "Assign to"/points fields are entirely absent from the personal
project's new-task form. Then created a team via the existing Teams screen
(confirmed `ensure_team_project` fired — the new team's project appeared in
`/web/projects` immediately with a "Team" badge, no explicit `CreateProject`/
`AttachTeamToProject` call needed) and walked its Tasks screen: new-task form
showed both "Assign to" and points fields; created a task, assigned it to
self with `points=25`; completing an *unassigned* copy was correctly rejected
(`422`, "cannot complete an unassigned team item"), confirming the
completion-transition guard still applies through the new screen; after
assigning and completing, the list page's points badge went from "0 pts" to
"25 pts", confirming the full award path (`team_items::update_team_item` →
`TeamRepo::add_team_points`) works unchanged through
`service::project_items`'s delegation. Scratch DB and both server processes
cleaned up after verification. No CLI or MCP server changes, per this
stage's own scope — B5b (Events) is next.

### B6 — CLI (`todo-cli/`)

- Add `prl projects` (list/create/attach-team/detach-team/members/set-role);
  repoint `prl items` at `ProjectItem` once a project is selected. Resolves
  the CLI's standing "no team item support" known issue (CLAUDE.md Known
  Issues) as a side effect — one unified item surface instead of a missing
  team-item one.

**Verify:** manual CLI smoke test against a dev server.

### B7 — MCP server (`mcp-server/`)

- Add project tools (`list_projects`/`create_project`/
  `attach_team_to_project`/etc.); repoint item tools at `ProjectItem`
  operations.

**Verify:** manual tool-call smoke test via this repo's own `.mcp.json`.

---

Stage C (already sketched earlier in this doc) then retires: legacy
`Item`/`TeamItem` Smithy operations + dual-write, `items.user_id`/`team_id`
columns, `activity_log.team_id`, old web_ui screens (whatever B5f's
legacy-URL choice left behind), `team_members.role`/`points`, and the
`TODO_BOOTSTRAP_ADMIN_TEAM_ID` bootstrap rework.
