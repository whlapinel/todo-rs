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

**B5b implementation notes:** Done, matching the plan closely and following B5a's own
structure precedent exactly. New module `src/web_ui/project_events/{mod.rs, handlers.rs,
templates.rs}` (mirroring `project_tasks/`'s three-file split) plus 9 new templates under
`templates/project_events/`. `src/web_ui/events.rs`, `team_events.rs`, and their templates
are completely untouched, per the plan's "old and new coexist until B5f" rule. No new
service-layer code was needed — same as B5a, `service::project_items`/`service::projects`
already provided a fully unified, membership-gated CRUD surface; every handler is "resolve
the project via `service::projects::get_project`, then delegate."

**`Row` extended rather than a new template built**, continuing `components::row::Row`'s own
doc comment ("Tasks first, Events/Simple Lists to follow in later B5 sub-stages"). Added two
fields: `scheduled_end_date: Option<String>` (paired with `scheduled_date` to render a
`start–end` window) and `event_type: Option<String>` (renders a `(type)` badge, matching
legacy `events/row.html`'s exact markup). `ProjectEventRow::from_item`
(`project_events/templates.rs`) is `Row`'s second real caller — a plain function, not its own
template struct, same shape as `ProjectTaskRow::from_item`. Wiring the two new fields into
`ProjectTaskRow::from_item` too was a deliberate small scope extension beyond "just Events":
`scheduled_end_date` was previously invisible at the Task row level (only shown in
`ProjectTaskDetailView`), and `event_type` is always `None` for a Task (`Item::validate`
restricts it to `Event`/`Template`) so wiring it there is inert but keeps both callers
symmetric rather than one passing `None`/`event_type: None` by hand.

**Events are simpler than Tasks at the `Row` level, confirmed by re-reading
`service::items::create_item`/`create_team_item`'s parent-kind check** (`"Events cannot have
children; link a task to it via sourceEventId instead"`) rather than trusting CLAUDE.md's
older prose (which describes template-triggered children as "parented under the event
itself" — stale; the actual mechanism is and remains `sourceEventId`, confirmed by reading
`events.rs`/`team_events.rs`'s own `create_event_child_form`/`create_team_event_child_form`
before writing this stage's equivalent). `ProjectEventRow::from_item` therefore hardcodes
`has_children: false`, `offset_label: None`, `assignee_name: None`, `siblings: Vec::new()`,
`is_source_event_linked: false`, and `expanded_row: true` unconditionally (legacy
`events/row.html` always renders its metadata line regardless of content, so `true`
reproduces that rather than gating on field presence like `ProjectTaskRow` does).

**"Linked tasks" fragment reuses `project_tasks`, not a new renderer**: added
`pub(crate) async fn render_source_event_fragment` to `project_tasks/handlers.rs` (not
`project_events`) — it needs `ProjectTaskRow`/`ProjectTaskRowsFragmentTemplate` to render the
*tasks* referencing an event, and `project_tasks` already owns those. This mirrors the
existing cross-module precedent exactly: `events.rs` calls
`tasks::handlers::render_source_event_fragment`, `team_events.rs` calls
`team_tasks::render_source_event_fragment`; `project_events` calling into
`project_tasks::handlers::render_source_event_fragment` is the same shape, one level up.

**`resolve_linked_event`'s bridge closed**: `project_tasks/templates.rs`'s
`resolve_linked_event` (added in B5a with an explicit "still links to the legacy detail URL
... since there's no project-scoped Events screen yet (that's stage B5b)" comment) now builds
`/web/projects/{project_id}/events/{id}` directly instead of calling
`dashboard::detail_url`. Safe without an extra lookup because the event is already fetched
via `get_by_project(project_id, &event_id)` scoped to the same project. Removed the now-dead
`use crate::web_ui::dashboard::detail_url;` import from that file as part of this change.
Verified end-to-end in the smoke test below (a task's "Linked event" link on the project
Tasks screen points at the new project Events URL, not the legacy one).

**Calendar view is a new capability for team-backed projects here too** (same as B5a's Tasks
calendar) — `team_events.rs` already had its own calendar route, but a *project*-scoped one
unifies personal and team-backed under one screen "for free," consistent with B5a's framing.

**Recurrence/repoint behavior confirmed working transparently through the new screen, and
found to differ from CLAUDE.md's description**: completing a recurring event does **not**
delete the old item and create a successor under a fresh id (CLAUDE.md's Recurrence section
says this); the actual code (`service::items::update_item`, `src/service/items.rs` ~line
343) creates the successor via `repo.create`, then calls `archive_recurrence(&mut item)` and
updates the **original** item in place (kept, marked complete) rather than deleting it —
confirmed by reading the code directly rather than trusting the doc, then verified live via
the smoke test below. One consequence for every recurring-completion handler in the
codebase, not just this stage's new one: the existing `Err(RepoError::NotFound) =>
hx-refresh` branch that `events.rs`/`team_events.rs`/`project_tasks`/this stage's
`update_project_event_form` all carry (re-fetching the old id after a recurring completion,
expecting it to be gone) appears to be dead code under current service-layer behavior — the
old id remains fetchable. Not fixed here: this is a pre-existing, repo-wide quirk predating
this stage (every screen has carried the identical branch since before B5a), not something
introduced by `project_events`, and fixing it is a cross-cutting cleanup outside one screen's
own scope. `project_events` reproduces the existing convention verbatim for consistency
rather than diverging from every other screen unilaterally.

Also confirmed live: an Event's recurrence correctly repoints any task linked to it via
`sourceEventId` onto the new successor event's id (`repoint_source_event_tasks`,
`src/service/items.rs`) — a linked task's `sourceEventId` updated automatically when its
event recurred, with no code in this stage touching that path at all; it rides along for
free through the shared `service::project_items` → `service::items` delegation, same as
B2c's implementation notes predicted for same-owner copy/update helpers.

**Testing:** `cargo check`/`cargo build`/`task codegen` clean (no Smithy changes this stage —
`service::project_items` already had everything needed, same as B5a). `cargo test`: 195/195
passing, zero regressions (no service/storage/domain code touched besides `Row`'s two
additive fields and `ProjectTaskRow::from_item`'s two additive assignments — same "web_ui
layer only, no automated tests added" precedent B5a/B2d established, verified instead by the
manual smoke test below).

**Manual smoke test** (built the binary, ran against a throwaway SQLite DB via
`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly): personal
project — confirmed `GET /web/projects/:id/events` renders the empty list; created a
recurring event (`recurrence: "every year"`, `recurrenceBasis: SCHEDULED_DATE`,
`eventType: "outdoor"`) — row rendered with the `(outdoor)` badge (confirming `Row`'s new
`event_type` field); detail page showed scheduled/repeat fields; calendar view showed the
event on the right day; added a linked task via the "Linked tasks" form
(`dueOffsetDays: -2`) — rendered correctly and, per `ItemRepo::list_by_source_event`, is
reachable from the event regardless of which screen created it; "Save as template" returned
"Saved"; completed the recurring event — confirmed via the JSON API that the **original**
event row persisted (`complete: true`, contradicting CLAUDE.md's delete-and-recreate
description as noted above) alongside a **new** successor row for next year, and that the
linked task's `sourceEventId` had been repointed from the old event's id to the new one;
confirmed the default (non-`showComplete`) list only shows the new occurrence. Then created a
team (confirmed `ensure_team_project` fired, per B5a's precedent) and walked its Events
screen: new-event form confirmed to have **zero** assign-to/points fields (Events never carry
either, personal or team — unlike Tasks, there's no `is_team_project`-gated markup to hide at
all, since none was ever written); created and deleted a team event via the new screen, and
loaded its calendar view. Scratch DB and server process cleaned up after verification. No
CLI or MCP server changes, per this stage's own scope — B5c (Simple lists) is next.

**B5c implementation notes:** Done, matching the plan closely and following B5a/B5b's structure
precedent exactly. New module `src/web_ui/project_simple_lists/{mod.rs, handlers.rs,
templates.rs}` plus 6 new templates under `templates/project_simple_lists/`. `src/web_ui/
simple_lists.rs`, `team_simple_lists.rs`, and their templates are completely untouched, per the
plan's "old and new coexist until B5f" rule (confirmed live — `GET /web/team-simple-lists/:id`
still renders correctly after the new screen shipped). No new service-layer code was needed —
same as B5a/B5b, `service::project_items`/`service::projects` already provided a fully unified,
membership-gated CRUD surface; every handler is "resolve the project via
`service::projects::get_project`, then delegate."

**Simplest of the three B5 sub-stages so far, confirmed by re-reading
`team_simple_lists.rs`'s own doc comments before writing this stage's equivalent**: Simple
items carry *no* optional `Row` fields at all — no dates, no recurrence, no offset, no
`sourceEventId`, and (unlike Tasks) no assignment/points even on a team-backed project
(`team_simple_lists.rs`'s own comment: "Simple items never carry assignment/points either
(Task-only...)"). `ProjectSimpleItemRow::from_item` (`project_simple_lists/templates.rs`) is
`Row`'s third real caller (after `ProjectTaskRow`/`ProjectEventRow`) and is the simplest of the
three: `expanded_row: false` unconditionally (no metadata line ever has anything to show,
matching legacy `simple_lists/row.html`'s single-line-only markup) and every other optional
field is `None`/`false`. No `is_team_project` gating was needed anywhere in this stage's forms
or templates — unlike `ProjectTaskForm`/`ProjectTaskDetailFields`, there is no
assign-to/points markup to conditionally hide in the first place, so `NewProjectSimpleItemPageTemplate`/
`ProjectSimpleItemDetailFields` carry no such field at all. Confirmed live: the new-item form on
a team-backed project has zero `assignedToUserId`/`points` inputs, same as the legacy
`team_simple_lists.rs` screen.

**No `detail_view.html`/three-fragment split, matching the legacy module's own shape rather
than `ProjectTaskDetailView`/`ProjectEventDetailView`'s pattern**: Simple items have no
`complete` concept at all (`Item::validate` rejects `complete: true` for `ItemType::Simple`),
so — exactly like `simple_lists.rs`/`team_simple_lists.rs` before this stage —
`project_simple_lists/detail_page.html` **is** the read-only view directly (no separate
`detail_view.html` fragment, no complete-toggle checkbox to put in one), and
`update_project_simple_item_form`'s non-close response is the two-fragment `{row}{fields}`
shape, not three. This isn't a simplification introduced by this stage; it's the same
"template children are the one variant without the toggle" exception CLAUDE.md's Web UI
section already documents for the row-editing convention, just naturally recurring here for a
different reason (no completion concept vs. no completion concept).

**`points_label` included on the list page, diverging from B5b's precedent, not B5a's**: `project_tasks_page`
computes and shows `points_label` (the viewer's own running team-points balance, not
task-specific); `project_events_page` does not (checked — `ProjectEventsListPageTemplate` has
no such field, and `project_events/list_page.html` has no badge markup at all, even though
`team_events/list_page.html` — the screen it replaces — does show one). Since
`team_simple_lists.rs`'s existing list page *does* show this badge (viewer's own balance,
independent of whether Simple items themselves ever carry points), `project_simple_lists_page`
was written to match B5a's precedent and `team_simple_lists.rs`'s own current behavior rather
than B5b's — computing `points_label` via `team_service::member_points` the same way
`project_tasks_page` does. This is flagged as a possible pre-existing gap in B5b (dropping a
badge every other team-backed list screen had) rather than a deliberate B5c divergence to copy
forward; not fixed here since B5b is already shipped and out of this stage's scope, but worth a
follow-up pass whenever `project_events` is next touched.

**Testing:** `cargo check`/`cargo build` clean (only the same pre-existing dead-code warnings
every prior stage has produced — nothing new introduced by this stage's files). `cargo test`:
195/195 passing, zero regressions (no service/storage/domain code touched at all this stage —
purely additive `web_ui` + templates, matching B5a/B5b's "web_ui layer only, no automated tests
added" precedent, verified instead by the manual smoke test below).

**Manual smoke test** (built the binary, ran against a throwaway SQLite DB via
`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly): personal
project — confirmed `GET /web/projects/:id/simple-lists` renders the empty list; created a
top-level item ("Buy milk") via the standalone new-item form (`redirect=1` →
`hx-redirect` back to the list) — row rendered with the correct
`/web/projects/{id}/simple-lists/{id}` URL; detail page showed Edit/Back links and the
sub-item form; edit page pre-filled the name; added a sub-item ("Whole milk") via the
detail page's inline form — appeared in both the live children fragment and a direct
`GET .../children` call; renamed the parent via the edit form's non-close save — confirmed
the response was the two-fragment `{row}{fields}` shape with **no** `type="checkbox"`
anywhere in it (no complete concept, as expected); list page showed the `▶` child-indicator
arrow next to the renamed parent; promoted the child to top-level via its "Promote to sibling
of parent" button — `hx-redirect`ed to the list as expected (no grandparent); deleted both
items, confirmed the list returned to "No items yet." Then created a team (confirmed
`ensure_team_project` fired — the new team's project appeared in `ListProjects` immediately)
and walked its Simple Lists screen: list page showed a "0 pts" badge (per the `points_label`
note above); new-item form confirmed to have zero assign-to/points fields; created and listed
a team item, confirming the badge and row both rendered correctly. Finally, loaded the
**legacy** `GET /web/team-simple-lists/:team_id` screen for the same team and confirmed it
still renders HTTP 200 unaffected, proving the old and new screens genuinely coexist per this
stage's own scope. Scratch DB and server process cleaned up after verification. No CLI or MCP
server changes, per this stage's own scope — B5d (Templates) is next.

**B5d implementation notes:** Done, matching the plan closely, plus one real bug found and
fixed mid-stage that the plan's own text had flagged as a risk but not resolved (see below).
New module `src/web_ui/project_templates/{mod.rs, handlers.rs, templates.rs}` (same three-file
split as B5a-c) plus 11 new templates under `templates/project_templates/` mirroring
`templates/templates/`+`templates/team_templates/` merged with an `is_team_project` gate on the
row's "Use…" form (same gating shape `ProjectTaskDetailFields`'s assign-to/points markup already
established) rather than two separate row templates. `src/web_ui/templates.rs`,
`team_templates.rs`, and their templates are completely untouched and confirmed still returning
HTTP 200 live — same "old and new coexist until B5f" rule as B5a-c.

**The `list_project_templates`/project_id gap the plan flagged, resolved by delegation rather
than backfill:** `templates::create_template`/`create_team_template` build an `Item` and call
`repo.create` directly — they never routed through `items::create_item`/`team_items::
create_team_item`, so (unlike every real item) a template's row never got `project_id` set by
B2's dual-write at all. Two options existed: backfill `project_id` onto every template row (a
migration, plus every existing call site), or resolve a project's templates through its already-
known `owner_user_id`/`team_id` instead of the `project_id` column, mirroring exactly how stage
B4's `create_project_item`/`update_project_item`/`delete_project_item` already resolve a project
down to a plain `user_id`/`team_id` and delegate. Went with the latter — no migration, no schema
change, always correct regardless of whether a given template row happens to carry `project_id`.
Landed as three new functions in `src/service/templates.rs`: `list_project_templates` (delegates
to `repo.list_templates(owner_user_id)`/`repo.list_team_templates(team_id)`),
`create_project_template`/`CreateProjectTemplateParams` and `update_project_template`/
`UpdateProjectTemplateParams` (delegate to the four existing personal/team functions), all three
gated by `require_project_member` first, matching every other stage B3/B4 service function's
shape. `project_tasks::save_project_task_as_template`/`project_events::
save_project_event_as_template` (B5a/B5b's own local personal-vs-team branching, each flagged in
its own doc comment as "no `create_project_template` service function exists yet — that's stage
B5d's job") were repointed to call this new function, deleting the duplicated branching in both
handlers.

**One real bug found via manual smoke test, not caught by unit tests, fixed before this stage
shipped:** the resolve-by-owner approach above is correct for *listing* a project's templates,
but every other project-scoped lookup in this new screen (children fragment, detail/edit
redisplay, the "Use" form's template fetch) was written using `repo.get_by_project(project_id,
template_id)` — the same lookup every other B5 screen uses, since a real item's `project_id`
*is* reliably set. A template's `project_id` being permanently `None` meant every one of those
lookups 404'd immediately after creating a template through the new screen (confirmed live: POST
`/templates` succeeded and returned a row, but the very next `GET .../items` children-fragment
request came back "not found"). Fixed by having `create_project_template` do one extra
read-modify-write immediately after delegating to `create_template`/`create_team_template`:
fetch the just-created row via `repo.get`/`repo.get_team_item` (owner-scoped, always works),
set `item.project_id = Some(project_id)`, and persist via `repo.update`/`repo.update_team_item`
(both already had `project_id` in their `SET` clause from stage B2, so no storage-layer change
was needed — confirmed by reading `src/storage/sqlite/items.rs` before assuming so). This makes
every `get_by_project`-based lookup elsewhere in this screen work like any other project-owned
row, at the cost of one extra write per template creation. Legacy `templates.rs`/
`team_templates.rs`-created templates (and any created before this fix, during this same
session) still carry `project_id: NULL` — an accepted, bounded gap in the same class as B1/B2's
own "pre-existing row won't have this field until touched again" notes, not fixed by a migration
here. The two `create_project_template_delegates_to_*` unit tests were updated to mock the
follow-up `get`/`update` (or `get_team_item`/`update_team_item`) pair this introduced.

**Children creation/update reuses `service::project_items` directly, no template-specific
service code needed:** `create_project_template_child_form`/`update_project_template_child_form`
call `project_item_service::create_project_item`/`update_project_item` with `parent_item_id:
Some(template_id)` (create) or explicit `item_type: Some(ItemKind::Task)` (update) — verified by
reading `service::items::create_item`/`team_items::create_team_item` before assuming this would
work: `create_item`'s parent-is-Template auto-detection upgrades a personal child to `Template`
kind automatically, while `create_team_item` has no such detection and leaves a team child as
plain `Task` kind — a pre-existing asymmetry between the two legacy functions that `templates.rs`/
`team_templates.rs` already lived with (their own child forms round-trip through the same
functions), reproduced here verbatim rather than "fixed," since fixing it is a cross-cutting
change to `create_team_item` outside this stage's scope. Because children are created via the
normal `create_item`/`create_team_item` path (not the direct-`repo.create` path templates
themselves use), they get `project_id` set automatically at creation — no equivalent backfill
needed for children, confirmed live (children fragment worked immediately, before the
project_id-backfill fix above was even applied to the template root). Template *deletion*
(`delete_project_template_form`, whole template) and child deletion both reuse
`project_item_service::delete_project_item` unchanged — safe regardless of the `project_id` gap,
since that path's underlying `items::delete_item`/`team_items::delete_team_item` fetch by
`repo.get`/`repo.get_team_item` (owner-scoped), never `get_by_project`.

**"Use" flow unified across personal/team in one code path**, simpler than the legacy pair it
replaces: `use_project_template_form` always calls `project_item_service::create_project_item`
with `assigned_to_user_id` passed through unconditionally — dropped automatically on the personal
branch (`CreateItemParams` has no slot for it), exactly as `project_tasks`/`project_simple_lists`
already established for the same field. No `is_team_project` branch needed in the handler itself,
only in the row template's assignee `<select>`. Redirects to
`/web/projects/{project_id}/tasks/{new_item_id}` (the new project-scoped Tasks screen), not the
legacy `/web/tasks/{id}`/`/web/team-tasks/{team_id}/{id}` legacy `use_template_form`/
`use_team_template_form` redirect to.

**Testing:** 7 new unit tests in `service::templates` (202 total, up from B2's/B5c's 195):
`list_project_templates` delegates-personal/delegates-team/rejects-non-member,
`create_project_template` delegates-personal/delegates-team (each asserting both the delegated
`create` call *and* the follow-up `get`+`update`/`get_team_item`+`update_team_item` pair),
`update_project_template` delegates-personal/delegates-team. `cargo test`: 202/202 passing, zero
regressions. `cargo check`/`cargo build`: clean, only the same pre-existing dead-code warnings
every prior stage has produced.

**Manual smoke test** (built the binary, ran against a throwaway SQLite DB via
`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly): personal project —
created a template with `eventType=rain` via the new screen's inline form; detail page showed the
"Auto-triggered by event type: rain" line; added a child with `dueOffsetDays=-3`; children
fragment rendered correctly (this is what caught the `project_id` bug above — first attempt
404'd, fixed, retried, passed); renamed the template via its edit form; used it with an explicit
due date and confirmed via the JSON API that the new task's child ("Pack boxes") landed with a
due date exactly 3 days before the parent's, `dueOffsetDays: -3` preserved. Then confirmed
`project_tasks::save_project_task_as_template` (repointed this stage) correctly creates a
project-scoped template findable via the new screen. Team-backed project — confirmed
`ensure_team_project`'s project appeared with a "Team" badge; created a team template, confirmed
the row's "Use…" form showed the assignee `<select>` (personal project's did not); added a child;
created a team Event via the `ProjectItem` API with a matching `eventType: "rain"` and confirmed
the auto-trigger fired end-to-end through the new unified surface — the template's child appeared
as a new top-level task with `sourceEventId` pointing at the new event, no code in this stage
touching that trigger path at all (it rides along for free through `service::project_items`' →
`service::items`/`team_items` delegation, same as B5b's notes predicted for a different feature);
exercised child edit (rename + offset change, "Save and close"), child delete, and template
delete. Confirmed both legacy `GET /web/templates` and `GET /web/team-templates/:team_id` still
return HTTP 200 unaffected, proving old and new coexist per this stage's own scope. Scratch DB
and server process cleaned up after verification. No CLI or MCP server changes, per this stage's
own scope — B5e (Dashboard, assigned-items, activity, teams admin) is next.

**B5e implementation notes:** Done, matching the plan closely, with the two ambiguous plan
bullets ("dashboard.rs + team_dashboard.rs merge into one project-aware dashboard" and
"team_activity.rs repoints its display ... UI/URL cleanup only") both resolved toward *new*
project-scoped screens (following B5a-d's own "old and new coexist until B5f" precedent)
rather than editing the legacy modules in place — `dashboard.rs`, `team_dashboard.rs`, and
`team_activity.rs` are all completely untouched and confirmed still returning HTTP 200 live.

**New `src/web_ui/project_dashboard.rs`** (`/web/projects/:project_id/dashboard[...]`)
literally merges `dashboard.rs` (combined Task/Event `dashboard_date` semantics, preset
date-window filtering, calendar view) with `team_dashboard.rs` (assignee-name display) into
one screen that handles both personal and team-backed projects — `is_team_project` (from
`project.team_id.is_some()`) gates `can_delete` the same way the two legacy screens
implicitly did (`dashboard.rs`'s rows were always the caller's own and always deletable;
`team_dashboard/row.html` never had a delete affordance at all). Small per-screen helpers
(`dashboard_date`/`dashboard_has_time`/`type_symbol`) were duplicated from `dashboard.rs`
rather than shared — same "duplicated rather than widening a legacy module's visibility"
precedent `project_tasks/mod.rs` already set for its own form-parsing helpers.
`preset_range`/`PRESETS` *were* reused via `use super::dashboard::{preset_range, PRESETS}`
(already `pub(crate)`, already reused by `team_dashboard.rs` itself) since they're pure
date-window math with no personal/team coupling to duplicate.

**One new storage-layer method was needed, unlike B5a-d (which needed zero):**
`ItemRepo::list_due_by_project` (`src/storage/sqlite/mod.rs` trait +
`src/storage/sqlite/items.rs` impl), the direct `project_id`-keyed analog of
`list_due_team_items` — necessary because no existing method returns a due-date-annotated,
`parent_name`/`has_children`-joined item set scoped to a project. Toggle-complete
(`/web/projects/:project_id/dashboard/items/:item_id`, PUT) delegates to stage B4's
`service::project_items::update_project_item` after building `UpdateProjectItemParams` from
the current row — same merge-of-`dashboard.rs`'s-and-`team_dashboard.rs`'s-own
`toggle_item_complete`/`toggle_team_item_complete` shape, including carrying forward the
same (likely-dead per B5b's own finding) `Err(RepoError::NotFound) => empty` branch for
consistency with every other screen in the codebase.

**New `src/web_ui/project_activity.rs`** (`/web/projects/:project_id/activity[...]`) is a
thin new screen on top of already-`project_id`-keyed data — `list_activity_for_project` and
`team_activity.rs`'s own `project_id` resolution were both already built in stage B2d; this
stage only adds the project-scoped URL/screen calling that same query directly (no
team_id-then-project fallback needed, since the query has been correct on `project_id`
alone since B2). A personal project simply renders "No activity yet." (points only exist on
team items, per CLAUDE.md) rather than being hidden or special-cased. The undo route
(`PUT .../activity/:entry_id/undo`) still calls the existing, still team_id-keyed
`service::activity_log::undo_activity_log_entry` (points stay `team_members`-keyed, not
`project_members`-keyed, unchanged by this stage) by resolving `project.team_id` first and
404ing if the project has none — defensively unreachable in practice since a personal
project can never have an entry to undo.

**`assigned_items.rs` updated in place** ("becomes a cross-project query" — it already was
one at the data-fetch level, via `repo.list_assigned(user_id)` querying across every team;
what changed is `detail_url` now builds `/web/projects/:project_id/...` links when
`item.project_id` is set, falling back to the legacy `/web/team-tasks/:team_id/...`-style
URL otherwise — same accepted, bounded gap (items created between B1 and B2, or never
touched since) B2c's own implementation notes already flagged, not new here). Verified live:
an assigned team task's row (checkbox PUT and detail link both) now points at
`/web/projects/{project_id}/tasks/{item_id}`.

**`teams.rs`/`templates/teams/detail_page.html`'s "View items" link retargeted**, exactly as
scoped by the plan (Dashboard/View activity links were deliberately left pointing at the
legacy `/web/team-dashboard`/`/web/team-activity` screens — the plan named only "View items"
for this stage, and those two legacy screens still fully work, so retargeting them isn't
required to avoid a broken page, just left for a future pass). `render_team_detail` gained a
`&Arc<dyn ProjectRepo>` parameter (all four call sites already had one reachable via
`Extension`, no new route wiring needed) and resolves `projects.get_by_team(team_id)`,
building `/web/projects/{project_id}/tasks` when found or falling back to the legacy
`/web/team-tasks/{team_id}` URL — same defensive-fallback style `team_activity.rs`'s own
project resolution already established.

**Testing:** `cargo check`/`cargo build`/`task web-styles` all clean (only the same
pre-existing dead-code warnings every prior stage has produced — the new
`list_due_by_project` trait method is flagged unused only until this stage's own handler
wires it up, same as every other storage-layer addition in this plan). `cargo test`:
202/202 passing, zero regressions — no new automated tests were added (this stage is
`web_ui`-layer plus one new storage method with no branching logic of its own to unit-test,
matching every prior B5 sub-stage's "web_ui layer only" precedent).

**Manual smoke test** (built the binary, ran against a throwaway SQLite DB via
`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly, exercised via
curl with a minted bearer token): personal project — created a due-today Task and a
scheduled-today Event via `CreateProjectItem`; `GET /web/projects/:id/dashboard` rendered
both rows (confirming the merged Task/Event `dashboard_date` semantics) with delete buttons
present; `.../dashboard/calendar` returned HTTP 200; `?preset=All` still showed both;
toggling the task complete via `PUT .../dashboard/items/:id` correctly struck it through;
`GET /web/projects/:id/activity` correctly rendered "No activity yet." for a project with no
team. Team-backed project — created a team (confirmed `ensure_team_project` fired, its
project appeared in `ListProjects` immediately); confirmed `GET /web/teams/:id`'s "View
items" link now reads `/web/projects/{project_id}/tasks` instead of the legacy
`/web/team-tasks/{team_id}`; created a task with `points=50` assigned to self; the project
dashboard showed the assignee name and **no** delete button (matching
`team_dashboard/row.html`'s own precedent); completing it via the dashboard's toggle route
awarded the points, confirmed via `GET /web/projects/:id/activity` showing a `+50 pts` row
with an Undo button; undid it via `PUT .../activity/:entry_id/undo` and confirmed the row
flipped to "reversed". Cross-project — `GET /web/assigned-items` showed the same team task's
checkbox and detail link both pointing at `/web/projects/{project_id}/tasks/{item_id}`
rather than the legacy team URL. Confirmed the legacy `/web/team-tasks/:team_id`,
`/web/team-dashboard/:team_id`, and `/web/dashboard` screens all still return HTTP 200
unaffected, proving old and new coexist per this stage's own scope. Scratch DB and server
process cleaned up after verification. No CLI or MCP server changes, per this stage's own
scope — B5f (Nav + legacy retirement) is next.

**B5f implementation notes:** Done. **Decision confirmed with the user before this stage
started** (the one open call the plan flagged): legacy URLs are **removed outright**, not
redirected — every `/web/tasks`, `/web/team-tasks/:team_id`, `/web/events`,
`/web/team-events/:team_id`, `/web/simple-lists`, `/web/team-simple-lists/:team_id`,
`/web/templates`, `/web/team-templates/:team_id`, `/web/dashboard`, `/web/team-dashboard/:team_id`,
and `/web/team-activity/:team_id` route now 404s via the existing `web_not_found` fallback,
same as any other unmatched `/web/*` path.

**Deleted outright** (source, not just routes): `src/web_ui/{tasks/, team_tasks.rs, events.rs,
team_events.rs, simple_lists.rs, team_simple_lists.rs, templates.rs, team_templates.rs,
dashboard.rs, team_dashboard.rs, team_activity.rs}` and their `templates/{tasks,team_tasks,
events,team_events,simple_lists,team_simple_lists,templates,team_templates,dashboard,
team_dashboard,team_activity}/` directories, plus the corresponding 11 `pub mod` lines in
`src/web_ui/mod.rs`, all `use web_ui::<legacy>::*` imports and route blocks in `src/main.rs`.
`teams.rs` and `assigned_items.rs` were kept and updated in place, exactly as the plan scoped
("Team is a pure group now"/"already a cross-project query") — neither was superseded by a
project-scoped equivalent.

**`nav.rs` rewritten as a project switcher**, not a mechanical rename of the old Personal/Team
switcher: `ActiveContext` is now `Project(String) | None` (previously `Personal | Team(String)`)
— `None` covers every page with no single natural project (the `/web/projects` list itself,
`/web/assigned-items`, and the teams list), where the plan's own "no single natural section"
precedent (`SidebarSection::None`) already existed for an analogous case. `build_nav_html` now
takes `&Arc<dyn ProjectRepo>` instead of `&Arc<dyn TeamRepo>` — the top switcher's pills are
every project the requesting user belongs to (`service::projects::list_projects`), not
Personal-plus-teams. One deliberate simplification not spelled out in the plan text: the sidebar's
4 section links (Tasks/Events/Simple Lists/Templates) and two new fixed-position Dashboard/Activity
links are all wrapped in a project-presence check (`ActiveContext::Project(id)` vs `None`) — on a
page with no active project these are omitted entirely rather than guessing a target, which
`templates/nav_sidebar_inner.html` implements via two `{% if let Some(href) = ... %}` blocks
(`dashboard_href`/`activity_href`, new `NavTemplate` fields) around the fixed-links group. This is
new behavior the old switcher never needed (Personal was always a valid fallback context before;
there's no equivalent "default project" now that Team dropped the item-container role). The
logo link and `nav_sidebar_inner.html`'s own always-present "Dashboard" fixed link both moved off
`/web/dashboard` — the logo now points at `/web/projects`, and the old always-present "Dashboard"
entry became the new conditional one described above (there's no project-agnostic dashboard to
send the logo to anymore).

**Every surviving handler's `active_context(&project.team_id)` helper simplified to
`active_context(project_id: &str)`** (`project_tasks/handlers.rs`, `project_events/handlers.rs`,
`project_simple_lists/handlers.rs`, `project_templates/handlers.rs`, `project_dashboard.rs` — 5
files, each with its own local copy of the helper per existing per-screen-duplication precedent) —
now a trivial `ActiveContext::Project(project_id.to_string())` wrapper, since `ActiveContext` keys
on project id directly rather than being derived from the project's `team_id`. `project_activity.rs`
had the equivalent logic inlined rather than as a named helper; simplified the same way. This
mechanical change had a side effect worth flagging: several handlers had fetched `project` (via
`service::projects::get_project`, which doubles as the membership-check gate) *solely* to read
`.team_id` for the old `active_context` call — with `project_id` now passed directly, those
bindings became compiler-flagged unused variables. Renamed each to `let _project = ...` (18 call
sites across `project_events/handlers.rs`, `project_simple_lists/handlers.rs`, and
`project_templates/handlers.rs`) rather than deleting the fetch — the call's membership-check side
effect is still required, only its return value became unused. Every handler that still reads
`project.team_id` for something else (points labels, assignee dropdowns, names-for-team lookups)
kept its binding named `project` unchanged.

**`preset_range`/`PRESETS` duplicated into `project_dashboard.rs`** (task 4, called out separately
in the plan since it was flagged as the one real compile-blocker): both were `pub(crate)` in the
now-deleted `dashboard.rs` and imported via `use super::dashboard::{preset_range, PRESETS}` —
copied verbatim into `project_dashboard.rs` as private items, matching the "duplicate per-screen
helpers rather than widen a legacy module's visibility" precedent every prior B5 sub-stage already
established (most recently B5e's own `dashboard_date`/`dashboard_has_time`/`type_symbol`).

**Two dead-link fallbacks fixed** (both previously pointed at a legacy URL that no longer exists
after this stage's deletions):
- `teams.rs`'s `render_team_detail`: `view_items_href`'s `None` branch (no project backs the team
  yet — defensively unreachable in practice per B2b's `ensure_team_project`, but the `Option` still
  exists in the type) used to fall back to `/web/team-tasks/{team_id}`; now falls back to
  `/web/projects` (the list page) for all three of `view_items_href`/`dashboard_href`/
  `activity_href`, computed together from one `projects.get_by_team(team_id)` call rather than
  three separate ones.
- `assigned_items.rs`'s `detail_url`: previously branched on `item.project_id` being `Some`/`None`,
  falling back to a legacy `/web/team-tasks/{team_id}/...`-style URL for a pre-B2 item with no
  `project_id`. That legacy target no longer exists, so `AssignedItemRow::from_item` now requires
  `project_id` to be `Some` (`item.project_id.clone()?`, alongside the pre-existing `team_id`
  requirement) and silently omits the row via the same `filter_map` otherwise — an accepted,
  bounded gap in the same class as the one B2c/B5e notes already flagged for this exact scenario,
  now resolved by omission (a row disappears) rather than by linking somewhere that 404s.

**`templates/teams/detail_page.html`'s Dashboard/Activity links retargeted** — B5e's own notes had
explicitly deferred this ("left for a future pass") since the legacy targets still worked at the
time; this stage's deletions made deferring no longer an option. `TeamDetailPageTemplate` gained
`dashboard_href`/`activity_href` fields (alongside the existing `view_items_href`, all three
resolved together as described above) and the template's two hardcoded `/web/team-dashboard/{{ id
}}`/`/web/team-activity/{{ id }}` `<a href>`s became `{{ dashboard_href }}`/`{{ activity_href }}`.

**Verification:** `cargo check`/`cargo build`: clean after the deletions and rewiring — the only
compile errors encountered mid-stage were the expected transient ones from legacy files not yet
deleted (each resolved by either finishing that file's nav-call update or, for the legacy files
themselves, deleting them outright) plus a handful of `active_context`'s newly-unused-`project`
warnings (see above), all resolved before the final check. `cargo test`: 202/202 passing, zero
regressions — this stage touched no service/storage/domain code, matching every prior B5
sub-stage's "web_ui layer only" precedent. `task web-styles`: clean.

**Manual smoke test** (built the binary, ran against a throwaway SQLite DB via
`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly, exercised via curl
with a minted bearer token): confirmed `GET /` now 303s to `/web/projects` (not `/web/dashboard`);
confirmed every legacy URL (`/web/tasks`, `/web/dashboard`, and the rest of the list above) now
404s; confirmed every project-scoped and unaffected screen (`/web/projects`,
`/web/projects/:id/{tasks,dashboard,activity}`, `/web/teams`, `/web/assigned-items`) still returns
HTTP 200. Inspected the rendered nav HTML directly: on the Personal project's Tasks page, the top
switcher showed a single active "Personal" pill and the sidebar showed both the conditional
Dashboard/Activity links and the 4 section links; created a team via `CreateTeam` (confirming
`ensure_team_project` still fires) and confirmed `ListProjects` returned both projects; on
`/web/projects` itself (no active project) confirmed the top switcher showed both project pills
with neither active and the sidebar's conditional group was empty (no Dashboard/Activity entries,
only the always-present Assigned to me/Teams/Projects links) — confirming `ActiveContext::None`
correctly suppresses the project-scoped links rather than guessing a target. Loaded the team's
`/web/teams/:id` detail page and confirmed all three of "View items"/"Dashboard"/"View activity"
now point at `/web/projects/{project_id}/tasks`/`.../dashboard`/`.../activity` respectively (not
any legacy URL). Created a team task with `points` via the `ProjectItem` API and confirmed it
rendered correctly on the project-scoped Tasks page with the "Family" pill shown active. Grepped
the entire `templates/`/`src/` tree for every legacy URL pattern post-deletion and confirmed zero
remaining references anywhere in the codebase. Scratch DB and server process cleaned up after
verification. No CLI or MCP server changes, per this stage's own scope — B6 (CLI) is next.

### B6 — CLI (`todo-cli/`)

- Add `prl projects` (list/create/attach-team/detach-team/members/set-role);
  repoint `prl items` at `ProjectItem` once a project is selected. Resolves
  the CLI's standing "no team item support" known issue (CLAUDE.md Known
  Issues) as a side effect — one unified item surface instead of a missing
  team-item one.

**Verify:** manual CLI smoke test against a dev server.

**Implementation notes:** Done, matching the plan closely, plus one real bug found and
fixed mid-stage. New `todo-cli/src/projects.rs` (registered in `main.rs`/`commands.rs`
alongside `items`/`teams`/`users`), `ProjectsCommand`: exactly the six subcommands the
plan lists (`list`/`create`/`members`/`attach-team`/`detach-team`/`set-role`) — no
`get`/`rename`/`delete` added beyond that, matching the plan's own enumeration rather
than mirroring `prl teams`' full surface. `list`/`create` take `user_id` (the
`/users/{userId}/projects...`-scoped operations, per stage A5's Smithy notes);
`members`/`attach-team`/`detach-team`/`set-role` don't (the no-`userId`-prefix
`/projects/{projectId}/...` operations), matching the generated client builders exactly
— confirmed by reading each operation's fluent builder before wiring rather than
guessing which ones needed `.user_id(...)`.

**`prl items` repointed via an additive `--project <project-id>` flag, not a
mode-switching global one:** added to `list`/`add`/`done`/`delete`/`get` (5 of 7
subcommands) as a per-variant `Option<String>` field, mirroring how every other
optional CLI flag in this file is already declared per-variant rather than globally.
`due`/`assigned` deliberately got no `--project` variant — both are inherently
cross-project queries ("what's due", "what's assigned to me across every team") with
no project-scoped Smithy operation to route to in the first place (`ProjectItem`'s
Smithy surface, stage B4, is create/get/update/delete/list only — no due-window or
assigned-items equivalent). Each of the 5 repointed subcommands branches early
(`if let Some(project_id) = project { ...; return; }`) into a parallel block calling
the `ProjectItem` operations, falling through to the original `user_id`-scoped legacy
code unchanged when `--project` is omitted — chosen over unifying the two branches
into one generic helper, since the legacy and `ProjectItem` fluent builders are
distinct generated types with only partially-overlapping setters (`ProjectItem`'s add
`assigned_to_user_id`/`points`; legacy personal `Item` has neither), so a shared helper
would need its own abstraction over two different builder types for little benefit at
this call-site count.

**`add` gained `--assign <user-id>`/`--points <n>`, rejected client-side without
`--project`** (mirroring the existing `--event-type`-without-`--item-type event`
rejection precedent already in this file) — `error: --assign/--points require
--project — assignment and points only exist on team-backed projects`. This is the
piece that actually resolves the CLI's "no team item support" gap: unlike legacy
`CreateItemInput`, `CreateProjectItemInput` has `assigned_to_user_id`/`points` setters
(stage B4's Smithy surface always carries both, dropped server-side on the personal
branch — see B4's own implementation notes), so passing them through was a direct,
no-new-machinery wire-up once `--project` selected the right builder.

**`done`'s round-trip extended to forward `assignedToUserId`/`points` too, on the
`--project` branch only** — `if let Some(a) = item.assigned_to_user_id() { req =
req.assigned_to_user_id(a); }` / same for `points()`, added alongside the existing
`dueOffsetDays`/`eventType`/scheduled-fields round-trip this function already did.
Legacy `done` (no `--project`) is unchanged — `UpdateItemInput` has no such fields to
round-trip in the first place.

**Real bug found via the manual smoke test below, not caught until then: `done`
panicked on any item with no due date**, in both the legacy and new `--project`
branches — `.due_date(item.due_date().cloned().unwrap())` (pre-existing in the code
this stage started from) panics whenever `due_date` is `None`, which is the common
case for a team item (assignment/points are far more likely on a bare task than a
`--due`-dated one, and the smoke test's own first team item had no due date at all).
Confirmed via `todo-client`'s generated types that `due_date` is `Option<DateTime>` on
both `UpdateItemInput` and `UpdateProjectItemInput` with no `build()`-time
required-field error, so the fix was a straight swap to the non-panicking equivalent:
`.set_due_date(item.due_date().cloned())` (takes the `Option` directly) in both the
legacy and `--project` branches — same direct-overwrite semantics as before (a `None`
due date stays `None` after the round-trip), just without the crash. This was a
latent bug in the code this stage started from, not introduced by B6, but B6 is what
first exercised it in a realistic path (a team item with no due date), so fixing it
was in scope rather than deferring — `prl items done` on a due-date-less item would
otherwise panic today, `--project` or not.

**Docs updated to match:** `docs/prl-user-guide.md` gained a new "Projects" section
(list/create/members/attach-team/detach-team/set-role, placed after Teams) and every
`items` subcommand example that changed gained a `--project` variant; the stale "`prl
items assign`/`unassign` have been removed... CLI support for team items is planned"
note was replaced with one pointing at `--project`/`--assign` on `add`; the Teams
section's "team items are not yet manageable from `prl`" note was removed (no longer
true). `CLAUDE.md`'s CLI section gained a paragraph on `prl projects`/`--project`
(replacing the now-stale "No `prl items assign`/`unassign`" paragraph), and the Known
Issues section's "CLI has no team item support" bullet was replaced with an
"MCP server has no Project support" bullet — the equivalent gap on that side, since
B7 (next) is what closes it there; `mcp-server/src/index.ts` confirmed to have zero
references to "project" before writing that bullet, not assumed.

**Testing:** no automated tests exist in `todo-cli/` (a thin CLI crate wrapping the
generated `todo-client`, with no prior test precedent to match — confirmed by
grepping for `#[test]`/`#[tokio::test]` before concluding this). `cargo build`:
clean, both before and after the `due_date` fix (the panic is a runtime bug, not a
compile error). Verified instead by a manual smoke test, same pattern as A5/B1-B5's
own precedent: built the server binary and `prl`, ran the server against a throwaway
SQLite DB (`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL` + `TODO_JWT_SECRET` + explicit
`TODO_BIND`, migrations 10/11 applied cleanly), minted a token via `/auth/token`, and
exercised: `prl projects list` (showed the auto-created "Personal" project) →
personal-project item lifecycle via `--project` (`add` → `list` → `get` → `done`,
including the `ASSIGNED` list column and `assigned`/`points` `get` fields) →
`prl teams create` (confirmed `ensure_team_project` fired — the new team's project
appeared in `prl projects list` immediately, no explicit `attach-team` needed) →
`prl projects members` on it → `prl items add --project ... --assign ... --points 25`
(a plain personal-item flag combination rejected first, confirming the client-side
guard) → `prl items done --project ...` on that assigned/points item with no due
date (this is what surfaced the panic above; fixed, rebuilt, retried, passed) →
`prl teams activity`/`teams members` confirmed the 25-point award landed and
`teams undo-activity` correctly reversed it back to 0 → `prl projects detach-team`
then `attach-team` round-tripped correctly (project's `TEAM ID` column went to `-`
and back) → `prl projects set-role ... admin` succeeded → deleted both items via
`--project` → confirmed the plain (no `--project`) legacy `add`/`list`/`get`/`done`/
`delete`/`due`/`assigned` commands are all still fully functional, unchanged. Scratch
DB and server process cleaned up after verification. No MCP server changes, per this
stage's own scope — B7 is next.

### B7 — MCP server (`mcp-server/`)

- Add project tools (`list_projects`/`create_project`/
  `attach_team_to_project`/etc.); repoint item tools at `ProjectItem`
  operations.

**Verify:** manual tool-call smoke test via this repo's own `.mcp.json`.

**Implementation notes:** Done, matching the plan closely and following B6's own
additive-parameter precedent rather than replacing anything. `mcp-server/src/index.ts`
(the only source file — this server has no service/storage layer of its own, it's a
thin fetch wrapper over `/api`) gained nine new tools mirroring `prl projects`' exact
six-subcommand surface plus the three CRUD operations `prl projects` deliberately
didn't expose (`get`/`update`/`delete` — the plan's "list/create/attach-team/
detach-team/members/set-role" enumeration was CLI-specific per B6's own notes, not a
ceiling on what the MCP surface should offer, so all nine `project.smithy` operations
got a tool): `list_projects`/`create_project`/`get_project`/`update_project`/
`delete_project`/`list_project_members`/`set_project_member_role`/
`attach_team_to_project`/`detach_team_from_project`. Each is a direct one-call `api()`
wrapper, no client-side logic beyond URL construction — same shape as every existing
tool in this file.

**Item tools repointed via an additive `projectId` parameter, not new parallel
tools** — deliberately mirroring B6's `--project` flag choice (additive per-variant
parameter, existing behavior unchanged when omitted) rather than the plan bullet's
literal "repoint" wording, which would have meant either a breaking change to five
existing tool schemas' semantics or a second parallel set of `project_*` tools.
`list_items`/`get_item`/`create_item`/`update_item`/`delete_item` (the same five `prl
items` subcommands B6 repointed, not the seven-command full list) each gained an
optional `projectId: string` property; the `switch` case for each now does
`args.projectId ? api(..., /projects/${projectId}/items...) : api(..., /users/${userId}/items...)`
— `userId` stays `required` on every schema (unchanged) even though it's ignored on
the `projectId` branch, since removing it would be a breaking schema change for the
personal-item call shape these tools already support. `list_items_due`/
`list_assigned_items` got no `projectId` variant, matching `prl items due`/`assigned`'s
own precedent exactly (both are inherently cross-project queries with no
`ProjectItem`-surface equivalent — see B4's own scope notes).

**`create_item`/`update_item` gained `assignedToUserId`/`points`, rejected client-side
without `projectId`** — same guard shape as B6's `--assign`/`--points`-require-
`--project` check, thrown as a plain `Error` (caught by the existing top-level
`try`/`catch` in the `CallToolRequestSchema` handler, which every tool call already
routes through, so no new error-handling path was needed): `"assignedToUserId/points
require projectId — assignment and points only exist on team-backed projects"`. Both
fields are only added to the request body when `args.projectId` is truthy — omitted
entirely on the personal-item branch, matching `CreateItemInput`/`UpdateItemInput`
having no such fields at all (same reasoning B6 gave for why this is what actually
resolves the "no team item support" gap on this side too).

**Manual smoke test performed as direct `/api` HTTP calls (curl with a minted bearer
token), not through an actual MCP client** — sufficient because `mcp-server/src/
index.ts` has zero logic beyond URL/body construction per tool (confirmed by reading
the diff before deciding this was adequate: every new `case` is a one-line `api(...)`
call or a straight ternary between two, so exercising the exact HTTP calls each tool
issues verifies the tool's behavior completely; an actual stdio MCP round-trip would
only additionally exercise the SDK's request/response framing, which every existing
tool in this file already relies on unchanged). Built the server binary (already
current — B7 touches no Rust/Smithy code, `task codegen` not re-run), ran it against a
scratch copy of the repo-root `todo.db` (`TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`,
migrations 10/11 applied cleanly), minted a token, and exercised: `GET
/users/:id/projects` (`list_projects`'s call) confirmed the pre-existing "Personal"
project; `create_project` → new personal project; `create_item`/`create_item`-with-
`points` on it via `POST /projects/:id/items` — confirmed `points` is silently dropped
on a personal-project item (no `points` field in the `GetProjectItem` response),
matching `service::project_items`' documented personal-branch behavior; **found that
`list_project_items`/`get_project_item` came back "not found" for these items** —
tracked down to the already-documented, pre-existing gap in B2c/B1's own implementation
notes ("`find_personal_project` — arbitrary pick if a user has more than one personal
project"): `create_project_item`'s delegation into `service::items::create_item`
resolves `project_id` via `find_personal_project(user_id)`, which doesn't know or care
which of a user's *several* personal projects the caller actually asked for on the
`ProjectItem` surface — it silently landed the item on the user's original
auto-created "Personal" project instead of the newly-created one. Confirmed this is
not a B7-introduced bug (re-read `service::project_items::create_project_item` and
`service::items::create_item` — neither has changed since B2c/B4) by re-running the
same create/list/get sequence scoped to the *original* "Personal" project id instead,
where everything worked correctly (items appeared in `ListProjectItems`, `GetProjectItem`
succeeded). Not fixed here — same accepted-gap class as every prior stage's own
"which personal project" notes, out of scope for an MCP-wrapper stage; flagged here
since this is the first time it was actually hit through a real multi-personal-project
scenario rather than reasoned about in the abstract.

Continued smoke test on the original "Personal" project: `update_item` (rename +
complete) and `delete_item` via `projectId` both round-tripped correctly. Team-backed
path: `create_team` (confirmed `ensure_team_project` fired — its project appeared in
`list_projects` immediately); `create_item` with `assignedToUserId`+`points=25` on the
team project → `get_item` confirmed both landed; `update_item` completing it → checked
the legacy `GET /teams/:id/activity-log` (this MCP server has no `project_id`-scoped
activity endpoint of its own — `list_team_activity_log`/`undo_activity_log_entry` are
still legacy-`teamId`-keyed tools, unchanged by this stage and out of its scope) and
confirmed a `pointsDelta: 25, reversed: false` entry, proving the award path works
identically through the new `projectId` branch. `list_project_members` correctly
showed `points: 0` for the same user — expected, not a bug: B5a's own implementation
notes already established that points are tracked via the legacy `team_members`/
`activity_log` system, not `project_members.points`, so this MCP tool faithfully
reflects a column that's real but not currently written to by anything.
`set_project_member_role`/`attach_team_to_project`/`detach_team_from_project` were
each verified on a **freshly created** project+team pair (not the one already exercised
above) after an initial false-positive: a first attempt against the already-used
project hit `"you are not an active member of this project's team"` on `attach`, traced
to a shell-scripting mistake in the smoke test itself (`$teamId` was unset in that
particular script invocation, so the PUT silently hit `/projects/:id/team/` with an
empty path segment, writing `team_id = ''` instead of a real id or `NULL` —
confirmed by direct `sqlite3` inspection showing `typeof(team_id) = 'text', length = 0`
on the corrupted row) — not a server-side bug; re-run with correctly-scoped shell
variables on a clean project round-tripped attach → get (teamId present) → detach →
get (teamId absent, real `NULL` per `typeof`) → re-attach → get (teamId present again)
with zero errors, confirming `AttachTeamToProject`/`DetachTeamFromProject` work
correctly end-to-end via the exact HTTP calls these two new tools issue. Scratch DB and
server process cleaned up after verification.

`cargo test`/`cargo check`: not run — this stage touched no Rust code. `npm run build`
(`mcp-server/`): clean, `tsc` reported zero errors. `CLAUDE.md` updated: the MCP Server
section gained a paragraph on the nine new project tools and the repointed item tools
(mirroring the CLI section's own `prl projects`/`--project` paragraph), and the "MCP
server has no Project support" Known Issues bullet — the last entry in that section —
was removed along with the now-empty "## Known Issues" heading itself, since this was
its only remaining item.

This closes out Stage B (B1-B7) in full. Stage C (retiring the legacy `Item`/`TeamItem`
surface and dual-write, dropping now-superseded columns, reworking the
`TODO_BOOTSTRAP_ADMIN_TEAM_ID` bootstrap) is next, not yet planned in detail per this
doc's own "Stage B/C are not planned in detail here" note from the top of the file.

---

## Stage C — Retire legacy surface, migrate role/points authority, cleanup

**Decision confirmed with the user before drafting this stage** (the one real fork,
asked directly since — per this doc's own "decisions confirmed before drafting" process
established at B1/B5f — it's a genuine break in behavior, not a recommend-and-flag call):
once the legacy `Item`/`TeamItem` Smithy operations are removed, `prl items`/the MCP
item tools **hard-require** `--project`/`projectId` — omitting it becomes a client-side
error, not a silent default-personal-project resolution. Every script or habit relying on
the old no-`--project` form must update explicitly.

**Scope correction found while drafting, not assumed from the plan's original sketch:**
the plan's very first "Target shape" section (top of this file) describes `items.user_id`/
`team_id` as replaced by "ONE foreign key" (`project_id`), and the end-of-B7 sketch above
lists them as something Stage C "drops." Neither is accurate anymore now that Stage B is
actually built. Per B4's own implementation notes, `service::project_items::{create,
update,delete}_project_item` don't reimplement item business logic — they resolve
`project_id` down to a plain `user_id`/`team_id` and **delegate straight into the existing
`service::items`/`service::team_items` functions**, which is where recurrence, offset-child
sync, completion-transition guards, and points-award all actually live, all keyed on
`user_id`/`team_id`. Those columns are not dual-write leftovers to be dropped in a cleanup
pass — they're the load-bearing primary key of the entire item service layer, `project_id`
is the bolted-on secondary one. Actually dropping `items.user_id`/`team_id` would mean
rewriting every one of those service functions to be `project_id`-primary instead — a
rearchitecture on the scale of Stage B itself, not a Stage C cleanup item. Recommendation,
applied below without a separate question since the alternative (a Stage D-sized rewrite)
isn't a real cleanup-stage option: **leave `items.user_id`/`team_id` in the schema
permanently**, unused only in the sense that no *external* API exposes them once C3 lands.
This still matches CLAUDE.md's existing "(or leave unused if SQLite column-drop is
impractical)" hedge on this exact point — just for an architectural reason, not a literal
SQLite `DROP COLUMN` limitation (confirmed separately that SQLite 3.35+, which this
project's bundled `libsqlite3-sys` satisfies, does support real `DROP COLUMN` — so the
hedge in CLAUDE.md was itself imprecise, but the conclusion it reaches is still correct for
different reasons). What Stage C *can* honestly retire are the columns that really are
dual-write leftovers with no remaining reader once C1 lands: `activity_log.team_id` and
`team_members.role`/`points` (see C1/C4 below) — `items.project_id` staying as a genuine
second key, not a replacement.

Five independently-landable sub-stages, same one-stage-per-session process as A/B:

- **C1 — Points/role authority migration to `project_members`.** No schema or API
  change, business-logic-only. `service::team_items.rs`'s points-award branch
  (currently `require_team_admin` for the assign/points gate, `TeamRepo::
  add_team_points` for the award, keyed off `params.team_id`) moves to
  `require_project_admin`/`ProjectRepo::add_project_points` (already built and unit-
  tested in A2/A4, unused by any caller since) keyed off the item's own
  `project_id`. `service::activity_log.rs`'s automatic (checkbox un-complete) and
  manual (`undo_activity_log_entry`) reversal paths both currently call
  `TeamRepo::add_team_points(&entry.team_id, ...)` to reverse — switch both to
  `ProjectRepo::add_project_points(&entry.project_id, ...)`, using the `project_id`
  `ActivityLogEntry` has carried since B2a (falling back to resolving it via
  `ProjectRepo::get_by_team(&entry.team_id)` for any pre-B2 row that still has
  `project_id: NULL`, the same defensive-fallback shape `team_activity.rs`'s own
  B2d read-path cutover already used). This is the piece that makes
  `team_members.role`/`points` genuinely unread by the time C4 drops them — every
  other Stage-C sub-stage depends on this one landing first.
  **Verify:** existing `service::team_items`/`service::activity_log` unit tests
  updated to mock `ProjectRepo` (`expect_add_project_points`) instead of `TeamRepo`
  for the points calls; a new test confirms a *personal* project's items still
  can't reach this path at all (points remain team-backed-project-only, matching
  CLAUDE.md's existing "Points exist only on team items" invariant — `Project`
  doesn't change that, it just changes which repo method records the balance).
  Manual smoke test: award points via both the legacy `TeamItem` completion path
  and the `ProjectItem` completion path (both still live at this point, C3 hasn't
  run yet) and confirm both land in `project_members.points`, not
  `team_members.points`; confirm `prl teams members`/`prl projects members` show
  the same, moved balance; confirm undo (both automatic and manual) correctly
  reverses it there too.

  **Implementation notes:** Done, matching the plan closely, plus one scope
  correction to this stage's own bullet text and one real bug found and fixed
  mid-stage that the plan hadn't anticipated.

  **Scope correction — "role" in this stage's title means item/points authority
  only, not Team's own admin-management authority.** `service::team_items.rs`'s
  points-admin gate (on both `create_team_item` and `update_team_item`) now calls
  `require_project_admin` instead of `require_team_admin`, exactly as planned.
  But `service::teams.rs`'s *own* admin-gated operations — inviting a member,
  renaming a team, `set_team_member_role` itself — still call `require_team_admin`
  and still read `team_members.role`, untouched by this stage. These are a
  genuinely different authority question ("can you manage this team's
  membership/identity" vs. "can you set points/assignment on this team's items"),
  and CLAUDE.md's own "Per-team roles & admin bootstrap" section already
  describes them as live, current behavior — C1 was never going to touch that
  without also resolving Stage A's still-open "does Team drop role entirely"
  call (A2/B1's implementation notes flagged this unresolved, on purpose). One
  concrete consequence for C4 below: `team_members.role` is **not** fully dead
  after C1 the way `team_members.points` is — see C4's own corrected bullet.
  `is_team_admin` (`service::teams.rs`) was deleted (not left as unused
  dead code) since its only caller was the now-repointed points-field
  display gate in `web_ui/project_tasks/handlers.rs` (three call sites, all
  switched to a new `service::projects::is_project_admin`, mirroring
  `is_team_admin`'s own non-erroring shape); `require_team_admin` itself is
  untouched and still very much alive for `service::teams.rs`'s own use.

  **Real bug found via manual smoke test, not caught by unit tests, fixed
  before this stage shipped:** `TeamRepo::list_members`'s SQL
  (`src/storage/sqlite/teams.rs`) selected `team_members.points` directly —
  since nothing writes that column anymore after this stage's own change, every
  consumer built on `list_members` (`service::teams::member_points`, which
  `project_tasks/handlers.rs`'s **current, actively-used** points badge calls;
  the legacy `ListTeamMembers` JSON API operation; `prl teams members`;
  `web_ui/teams.rs`'s own member-listing display) would have silently shown a
  frozen, increasingly-wrong balance instead of the live one now sitting in
  `project_members.points`. This is not a legacy-surface-only problem — the
  project-scoped Tasks screen's own badge is squarely in scope, so it was fixed
  as part of this stage rather than deferred: `list_members`'s query now `LEFT
  JOIN`s through `projects`/`project_members` (keyed on the team's backing
  project) and selects `COALESCE(project_members.points, 0)` instead, leaving
  `role` sourced from `team_members.role` unchanged (see the scope correction
  above — team-management role wasn't part of this migration). The `COALESCE`
  covers a team with no backing project, or a member not yet synced into
  `project_members` (e.g. still `PENDING`) — same "no balance yet" default
  `team_members.points`'s own `NOT NULL DEFAULT 0` gave for free. Two new
  `storage::sqlite::teams` tests cover both the project-sourced-not-frozen case
  and the no-backing-project fallback.

  `service::activity_log::reverse_entry` takes `&Arc<dyn ProjectRepo>` in place
  of `&Arc<dyn TeamRepo>` (a new `resolve_reversal_project_id` helper covers the
  same pre-B2-row `project_id: NULL` fallback `team_activity.rs`'s own B2d
  cutover established, via `ProjectRepo::get_by_team`); `undo_activity_log_entry`
  gained a `projects: &Arc<dyn ProjectRepo>` parameter alongside its existing
  `teams` one (still needed for the membership check, which stays team-based —
  another instance of the role-vs-points scope split above). `service::
  team_items::UpdateTeamItemContext` gained a `projects: Arc<dyn ProjectRepo>`
  field (`create_team_item`/`delete_team_item` still take a plain `&Arc<dyn
  ProjectRepo>` positional parameter, unchanged, since `create_team_item`
  already had one from stage B2). Every call site threading these through
  (`json_api::team_items::update_team_item`, `json_api::activity_log::
  undo_activity_log_entry`, `service::project_items::update_project_item`,
  `web_ui::project_activity::undo_project_activity_log_entry_form`) already had
  a `ProjectRepo` extension reachable, so no new `Extension`/route wiring was
  needed anywhere — same "already wired since A5/B2" precedent every points-
  adjacent change in this plan has hit.

  `create_team_item`'s points-admin check now resolves `project_id` once, up
  front (before the `team_assignment` block), shared by both the points gate
  and the existing dual-write `item.project_id` assignment below it — a small
  restructure from the plan's own description, not a second `ProjectRepo`
  round-trip. `update_team_item`'s points-award/reversal paths resolve their
  project id off `item.project_id`/`current.project_id` first, falling back to
  `ProjectRepo::get_by_team` only for a pre-B2-shaped item that's never
  round-tripped `project_id` — same accepted-gap shape B2c/B1 already
  documented, not new here.

  **Testing:** 3 new tests (205 total, up from B5d's 202 — the last stage that
  changed this count; B5e/B5f/B6/B7 were all web_ui/CLI/MCP-only and added none):
  2 in `storage::sqlite::teams` (the `list_members` fix above) and 1 in
  `service::activity_log` (`reverse_entry`'s `get_by_team` fallback path). Every
  existing `service::team_items`/`service::activity_log` unit test touching
  points was updated in place to mock `ProjectRepo` instead of `TeamRepo` for
  the points call (`expect_add_project_points` in place of
  `expect_add_team_points`), plus new helper fns (`project_with_role`,
  `ctx_with`) added alongside the existing `no_backing_project`/`ctx` ones
  rather than replacing them, since most non-points-focused tests in that file
  still construct items with no `project_id` at all (the points-admin gate is a
  no-op in that case, so `no_backing_project()`/`ctx()` stay valid defaults for
  them). `cargo test`: full suite passing, zero regressions to any prior
  stage's tests. `cargo check`: clean — `TeamRepo::add_team_points` is now
  flagged unused dead code (expected: this stage removed its only production
  caller; the trait method itself stays for now, see C4's corrected note on
  removing it alongside `team_members.points` itself).

  **Manual smoke test** (built the binary, ran against a throwaway SQLite DB via
  `TODO_AUTH_MODE=caddy` + `TODO_DEV_EMAIL`, migrations 10/11 applied cleanly):
  created a team (`ensure_team_project` fired); created a team item via the
  **legacy** `POST /api/teams/:id/items` with `points`/`assignedToUserId`,
  completed it via legacy `PUT`, confirmed `project_members.points` (not
  `team_members.points`) went to 50 via direct `sqlite3` inspection; undid it
  via the legacy `PUT .../activity-log/:entryId/undo`, confirmed the balance
  went back to 0 and `team_members.points` stayed 0 throughout; separately
  created and completed an item via the **new** `POST /api/projects/:id/items`
  path, confirmed the same `project_members.points` award; confirmed `GET
  /web/projects/:id/activity` renders the award; confirmed the new-task form's
  points input still renders (proving `is_project_admin`'s display gate works).
  In a second run, confirmed (after the `list_members` fix above) that the
  legacy `GET /api/users/:id/teams/:id/members` JSON API response, the
  project-scoped Tasks page's points badge, and the legacy `teams.rs` team
  detail page's own member listing all show the same, live, project-sourced
  balance (75) after a completion — not three different numbers. Scratch DBs
  and server processes cleaned up after verification. No Smithy/CLI/MCP
  changes, per this stage's own scope — C2 is next.

- **C2 — `TODO_BOOTSTRAP_ADMIN_TEAM_ID` → project-aware bootstrap.** Depends on C1
  (the promotion this env var triggers is meaningless once role has moved off
  `team_members`). `caddy_header_middleware` (`src/auth.rs`) currently promotes a
  user to `team_members.role = 'admin'` on `TODO_BOOTSTRAP_ADMIN_TEAM_ID`, gated on
  that team currently having zero active admins. Rework to resolve the team's
  backing project via `ProjectRepo::get_by_team` and gate/promote on
  `project_members.role` there instead — same "fires at most once, goes inert
  after a deliberate in-app demotion" semantics, same env var name (it still
  names a *team*, since that's what identifies the trusted group in
  `x-token-user-roles`/caddy-security's own model — only the write target moves
  to that team's project). If the team has no backing project yet (shouldn't
  happen post-B2b's `ensure_team_project`, but the code shouldn't assume), skip
  promotion the same way the current code implicitly can't act on a
  nonexistent team.
  **Verify:** unit test mirroring whatever coverage `caddy_header_middleware`
  already has for this branch (check first — this logic may currently only be
  covered by the doc comment's own described behavior, not a test); manual
  verification against a throwaway DB with `TODO_BOOTSTRAP_ADMIN_TEAM_ID` set,
  confirming a fresh admin-flagged login promotes `project_members.role`, not
  `team_members.role`, and that a second login is a no-op.

  **Implementation notes:** Done, matching the plan closely, plus one gap
  found and closed mid-stage (`ProjectRepo` had no `count_active_admins`
  equivalent — `service::projects.rs`'s `set_project_member_role` doc comment
  had flagged this exact gap back at A3/A5 and explicitly deferred it).
  `ProjectRepo::count_active_admins(project_id) -> Result<i64, RepoError>`
  added to the trait (`storage/sqlite/mod.rs`) and `SqliteProjectRepo`
  (`storage/sqlite/projects.rs`) — no `status` filter, unlike
  `TeamRepo::count_active_admins`'s `status = 'ACTIVE'` clause, since
  `project_members` has no status column at all: row presence *is* active
  membership (the invariant A4's `attach_team`/`accept`-sync already
  established). `set_project_member_role`'s own missing last-admin guard is
  **not** fixed by this stage — that doc comment is now only half-stale (the
  method it said didn't exist, now does), left as a note for whoever picks
  that up rather than expanded into this stage's scope.

  The old inline bootstrap block in `caddy_header_middleware` was extracted
  into a standalone `async fn sync_bootstrap_project_admin(projects:
  &Arc<dyn ProjectRepo>, bootstrap_team_id: &str, user_id: &str)` (private to
  `src/auth.rs`) purely so it could be unit-tested with `MockProjectRepo` —
  no test scaffolding existed for anything in `auth.rs` before this (confirmed
  per this stage's own "check first" note), so 4 new tests were added in a new
  `#[cfg(test)] mod tests` at the bottom of the file: promotes when the
  project has zero admins and the user is a member; does not promote when an
  admin already exists; does not promote a non-member; is a no-op when the
  team has no backing project. `TeamRepo` import dropped from `auth.rs`
  entirely — this bootstrap block was its only remaining use in that file.
  `member_status`'s team-side "PENDING vs ACTIVE" check has no
  `ProjectRepo` equivalent to call (see the no-status-column note above); the
  rework substitutes `member_role(..).is_some()` (row exists → active member)
  for that check, which is the same substitution `require_project_member`
  already made back in A3.

  **Testing:** 6 new tests (211 total, up from C1's 205): 4 in `auth::tests`
  (above) and 2 in `storage::sqlite::projects::tests`
  (`count_active_admins_counts_only_admin_rows`,
  `count_active_admins_returns_zero_for_project_with_no_admins`). `cargo
  test`: full suite passing, zero regressions. `cargo check`: clean.

  **Manual smoke test** (built the binary, ran against a throwaway SQLite DB
  via `TODO_AUTH_MODE=caddy` + explicit `x-token-user-email` headers per
  request rather than `TODO_DEV_EMAIL` — needed two distinct identities in
  the same run — `TODO_JWT_SECRET` set, migrations 10/11 applied cleanly):
  created a team
  (`ensure_team_project` backed it with a project, creator auto-admin on
  both); demoted the creator to project `member` via `PUT
  /api/projects/:id/members/:userId/role` (exercising the still-open
  no-last-admin-guard gap noted above — it allowed demoting the *only* admin,
  same pre-existing gap, not a new one); invited and accepted a second user
  onto the team (confirmed via `accept`'s existing project-member-row sync
  that they landed in `project_members` as `member`), leaving the project
  with zero admins; restarted the server with `TODO_BOOTSTRAP_ADMIN_TEAM_ID`
  set to that team's id; had the second user hit any route with
  `x-token-user-roles: authp/admin` and confirmed via direct `sqlite3`
  inspection that `project_members.role` for that user flipped to `admin`
  while `team_members.role` stayed untouched (`ACTIVE`/`admin` for the
  original creator, `ACTIVE`/`member` for the promoted user — team-management
  role is a separate axis, per C1's own scope note); confirmed the promotion
  log line fired; had the same user hit another route with the same header
  again and confirmed no change (already-admin no-op, matching the "goes
  permanently inert" semantics). Scratch DB and server process cleaned up
  after verification. No Smithy/CLI/MCP changes, per this stage's own scope —
  C3 is next.

- **C3 — Remove legacy `Item`/`TeamItem` Smithy surface + dual-write; hard-require
  `--project`/`projectId` client-side.** The user-facing break confirmed above.
  Two halves, land together (splitting them would leave the CLI/MCP calling
  operations that no longer exist):
  - `todo-cli/src/items.rs`: the 5 repointed subcommands' `if let Some(project_id)
    = project { ...; return; }` branch becomes the *only* branch — the
    `--project`-omitted fallthrough is deleted and replaced with a client-side
    error (`error: --project is required — the legacy personal Item API has been
    retired`), mirroring the existing `--assign`/`--points`-require-`--project`
    error's own wording style. `mcp-server/src/index.ts`: the `args.projectId ?
    ... : ...` ternaries in the 5 repointed tools become `if (!args.projectId)
    throw new Error(...)` the same way. `due`/`assigned` are unaffected (no
    `ProjectItem` equivalent exists for either — see B4/B6/B7's own repeated
    notes on this; they don't call the legacy `Item`/`TeamItem` CRUD operations
    being removed here in the first place, only `list_due`/`list_assigned`,
    which stay).
  - `model/src/main/smithy/item.smithy`'s `Item` resource + its 5 operations, and
    `model/src/main/smithy/team.smithy`'s `TeamItem` resource + its 4 operations
    (create/get/update/delete — `TeamItem`'s templates operations
    `CreateTeamTemplate`/`ListTeamTemplates` are a *different* resource concern,
    unaffected, still needed by `project_templates`'s team branch per B5d), all
    removed. `task codegen`. `src/json_api/items.rs`/`team_items.rs` deleted;
    `src/main.rs`'s builder wiring for the removed operations deleted;
    `src/service/items.rs`/`team_items.rs` **stay** (per the scope-correction
    above — `service::project_items` still delegates into them), only their
    `json_api`-facing callers go away. `ListTeamActivityLog`/
    `UndoActivityLogEntry` (still legacy-`teamId`-keyed per B7's own note) are
    untouched here — they're not part of the `Item`/`TeamItem` resource, and
    `project_activity.rs`'s own undo route already resolves through them via
    `project.team_id`, so removing them isn't implied by this sub-stage's scope
    (revisit only if a `project_id`-native activity-log surface is ever built).
  **Verify:** `task codegen` succeeds; `cargo check`/`cargo build` compile
  clean; `cargo test` full pass (existing `service::items`/`team_items` unit
  tests are untouched since those modules survive; only `json_api`-layer
  wiring is removed, and neither of those two service modules has
  `json_api`-layer tests to begin with, matching every prior stage's "web_ui/
  json_api handler modules aren't unit-tested at this granularity" note); manual
  smoke test confirming the removed endpoints now 404/reject at the HTTP layer,
  and that `prl`/the MCP tools' hard-require errors fire correctly and their
  `--project`/`projectId`-bearing calls still work end to end.

- **C4 — Drop the genuinely dead columns.** Depends on C1 (frees
  `team_members.points`, confirmed genuinely dead by C1's own implementation
  notes — nothing reads or writes it anymore, `list_members` was repointed to
  `project_members` as part of that stage) and C3 landing first is *not*
  required (nothing in C3 touches these columns), but doing it last matches
  the doc's own "verify against a live snapshot" caution for any migration
  that mutates production schema — safer to have the rest of the retirement
  already proven out first. New migration (next version after
  `backfill_projects`): `ALTER TABLE activity_log DROP COLUMN team_id`
  (superseded by `project_id` since B1/B2 backfilled and cut over every
  read/write) and `ALTER TABLE team_members DROP COLUMN points` only — **not**
  `role` too, despite this doc's own earlier text (both the top-of-file
  "Target shape" section and the original end-of-B7 sketch) having assumed
  both would go together. C1's implementation notes found that
  `team_members.role` still gates `service::teams.rs`'s own team-management
  operations (invite, rename, `set_team_member_role`, via `require_team_admin`)
  — a genuinely different authority than item/points, which C1 deliberately
  left alone (see Stage A's still-unresolved "does Team drop role entirely"
  open call). Dropping `role` now would break team management outright; doing
  so safely would mean actually resolving that open call first — either
  migrating team-management gating onto `project_members.role` too (awkward:
  team management, e.g. inviting the team's very first member, can happen
  before any project is attached) or replacing it with the plan's original
  "lightweight un-roled `owner_user_id`" recommendation — and that's a real
  design decision needing the same explicit user confirmation this doc's
  process has required for every other genuine fork, not something to fold
  into a column-drop migration's scope. Left as an explicit **open call for a
  future stage** (call it C4.5 or fold into a re-scoped C2 next time this file
  is picked up) rather than silently deferred. `TeamRepo::add_team_points`/
  `SqliteTeamRepo::add_team_points` (dead code since C1, per its own `cargo
  check` note) are removed in this sub-stage alongside the column, once
  nothing references `team_members.points` at all. **`items.user_id`/`team_id`
  are explicitly out of scope for this sub-stage** — see the scope-correction
  above; do not drop them.
  **Verify:** same `task db copy`-snapshot-first process as B1 — run the
  migration against a copy, confirm the app still starts and every
  points/activity-log flow still works (proving nothing silently still reads
  the dropped columns), *then* let it auto-run against production on next
  restart per the established rollout convention. `cargo test
  storage::migrations` idempotency test, matching every prior migration's
  precedent.

- **C5 — Doc/cleanup pass.** `CLAUDE.md`: remove the `@httpBearerAuth`-adjacent
  and Auth-section prose that still describes `team_members.role`/`points` as
  live (repoint to `project_members`); remove the Recurrence/Events/Points
  sections' now-stale cross-references to the deleted `Item`/`TeamItem`
  operations where they're used as the canonical example; update the CLI/MCP
  sections' `--project`/`projectId` prose from "optional, falls through to
  legacy" to "required." `docs/prl-user-guide.md`: same. Decide the fate of
  the dead `share_active_team` repo method flagged back at the top of this
  Stage-C sketch (found during original research — implemented, zero callers)
  — likely just delete it now, since Stage B never ended up needing it and no
  new UI feature in this plan calls for a "do these two users already share a
  project" check; confirm no caller exists before deleting.
  **Verify:** `cargo check`/`cargo build` clean; grep confirms no remaining
  reference to the removed operations/columns anywhere in `src/`/`docs/`/
  `CLAUDE.md`.

This is the last stage this plan currently anticipates — once C1-C5 land, the
Project abstraction work this whole document tracks is complete. No Stage D is
implied by anything above (the `items.user_id`/`team_id`-to-`project_id`
rearchitecture flagged in the scope correction is explicitly **not** proposed
as future work here — it would need its own from-scratch planning pass and its
own explicit ask, not an assumed continuation of this one).
