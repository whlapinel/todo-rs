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

**Implementation notes:** _(none yet — fill in before ending this stage)_

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

**Implementation notes:** _(none yet — fill in before ending this stage)_
