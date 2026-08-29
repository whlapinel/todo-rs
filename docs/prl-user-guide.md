# prl User Guide

`prl` is the official command-line interface for the PeoplesRepublicOfLists task management system. It communicates directly with the REST API, so you can manage your tasks from any terminal.

---

## Installation

From the project root:

```sh
task cli-install
```

This compiles and installs `prl` to `~/.cargo/bin`. Make sure that directory is on your `PATH`.

---

## First-time setup

`prl` needs three pieces of information: the API URL, a bearer token, and your user ID. Save them once and they'll be used automatically for every command.

**1. Set the API URL**

```sh
prl config set-url https://todo.lapinel-fam.club
```

**2. Get a token**

Sign in to the web app, then visit `/auth/token` in the same browser session to receive a long-lived JWT. Copy the `token` value from the JSON response.

```sh
prl config set-token eyJhbGci...
```

**3. Find and save your user ID**

Your user record is created automatically the first time you sign in via the web app. Find your ID with:

```sh
prl users list
```

Copy your ID from the output, then:

```sh
prl config set-user <your-user-id>
```

**Verify setup**

```sh
prl config show
```

---

## Configuration

Config is stored at `~/.config/prl/config.toml`. Any value can be overridden per-command with a flag or environment variable:

| Setting  | Flag      | Env var       |
|----------|-----------|---------------|
| API URL  | `--url`   | `TODO_URL`    |
| Token    | `--token` | `TODO_TOKEN`  |
| User ID  | `--user`  | `TODO_USER`   |

Example — use a different user for a single command:

```sh
prl --user abc123 items list
```

---

## Items

Every `items` subcommand requires a `--project <project-id>` flag (see
[Projects](#projects)) — `list`, `add`, `done`, `delete`, and `get` all
route through that project's items, personal or team-backed, the same
command either way; omitting `--project` on any of them is a client-side
error. `due` and `assigned` are the two exceptions — they're deliberately
cross-project queries (see their own sections below) and have no `--project`
flag at all. Every user has an auto-created personal project ("Personal" —
see [Projects](#projects)) to pass here for everyday personal items;
`--project` doesn't default to it automatically, so you do need to look up
its ID once with `prl projects list` and pass it explicitly (or save it in
a shell variable/alias).

### List items

```sh
prl items list --project <project-id>
```

Output columns: `ID`, `DONE`, `DUE`, `ASSIGNED`, `NAME`. Items with
sub-tasks show a `▸` suffix.

### List sub-tasks

```sh
prl items list --parent <item-id> --project <project-id>
```

### Add an item

```sh
prl items add "Buy groceries" --project <project-id>
prl items add "Submit report" --due 2026-06-20 --project <project-id>
prl items add "Chapter notes" --parent <parent-item-id> --project <project-id>
prl items add "Pack bag" --parent <parent-item-id> --days-before-due 1 --project <project-id>
prl items add "Team offsite" --item-type event --due 2026-09-01 --project <project-id>
prl items add "Rain today" --item-type event --event-type rain --project <project-id>
prl items add "Write report" --scheduled 2026-06-18 --scheduled-end 2026-06-20 --project <project-id>
prl items add "Milk" --item-type simple --project <project-id>
prl items add "Trip planning" --description "Book flights, hotel, and rental car" --project <project-id>
prl items add "Buy cake" --source-event-id <event-item-id> --days-before-due 2 --project <project-id>
```

`--description` is free-form notes text (up to 5000 characters), separate from the
required `name`, and valid on every item type/kind.

`--item-type` is `task` (default), `event`, or `simple` — events are calendar-style
items, distinguished from tasks mainly for display purposes; simple items are a bare
checkable name with no due date, scheduled window, or days-before-due offset
(the server rejects any of those on a `simple` item). `--event-type` is a free-text
category (e.g. `rain`), only valid on `--item-type event` (the server rejects it on
tasks and simple items — `prl` itself checks this too and errors before sending the
request); if it matches a checklist template's own event type, that template's
checklist items are automatically added the moment the item is created — as
`--source-event-id`-linked top-level tasks (see below), not children, since an
event can never have children. Checklist templates themselves aren't yet
manageable from `prl` (web UI or MCP server only), but an event created via
`prl items add --item-type event --event-type ...` can still trigger one that
already exists.

`--source-event-id` links a top-level task to an event it tracks — the same
mechanism the auto-trigger above uses, available for manually linking a task
to an event too. It's mutually exclusive with `--parent` (an item either
nests under a parent or references an event, never both) and, like a child's
`--days-before-due`, drives the task's due date instead of a manually-typed
one — the server computes it from the referenced event's own due/scheduled
date minus the offset, ignoring any `--due` you pass alongside it.

`--due`, `--scheduled`, and `--scheduled-end` all accept `YYYY-MM-DD` or a Unix
timestamp. `--due` is the deadline (drives offset-based child due
dates); `--scheduled`/`--scheduled-end` describe an optional start→end window —
when you actually plan to do it — and apply to tasks just as much as events. Note
that a child item (`--parent`) or event-linked task (`--source-event-id`) can't
use `--scheduled`/`--scheduled-end` at all — the server rejects it, since their
only supported date is the offset-derived due date.

`--project <project-id>` is required on every `add` (see the note at the
top of this section); `--assign <user-id>`/`--points <n>` only take effect
on a team-backed project (silently dropped by the server on a personal
one). `--priority <1-4>` (1 highest, 4 lowest) has no such restriction —
it's a plain Task-only sort key any project member can set on either a
personal or team-backed project:

```sh
prl items add "Mow the lawn" --project <project-id> --assign <user-id> --points 25 --priority 2
```

> **Note:** individual items don't support recurrence anymore — create an
> [item series](#item-series) instead if you want something to repeat. A
> child item (`--parent`) or event-linked task (`--source-event-id`) uses
> `--days-before-due` (days *before* the top-level item's or linked event's
> due date — must be zero or positive, since the server rejects a due date
> set after the anchor's) — the offset is what sets the item's due date
> whenever the top-level item is edited or the linked event is rescheduled.

### Download an import template

```sh
prl items template                    # prints CSV to stdout
prl items template --output items.csv # writes to a file instead
```

Header row plus 3 example rows (one Task, one Event, one Simple item)
covering every supported column.

### Import items from CSV

```sh
prl items template --output items.csv
# ... edit items.csv ...
prl items import items.csv --project <project-id>
```

Every row is attempted independently — a bad row is reported with an error
and skipped, valid rows are still created (nothing rolls back). Column order
doesn't matter; unrecognized or missing optional columns are ignored/left
blank. `parentItemId`, if given, must reference an item that already exists
(not another row in the same file — there's no intra-file parent/child
resolution yet). Dates use the same `YYYY-MM-DD`-or-Unix-timestamp
convention as `--due`/`--scheduled` above. `--format` defaults to `PRL`
(the only format currently supported); it's an extension point for future
formats (e.g. importing a Todoist export).

There is no `recurrence`/`recurrenceBasis` column — a row with `recurrence`
set is rejected as an error for that row. Bulk-import is for loading a batch
of items, not authoring a recurring series; create a series with `prl series
create` (or the Item Series screen) instead — see
[Item series](#item-series) below.

### Mark complete

```sh
prl items done <item-id> --project <project-id>
```

Completing an [item series](#item-series)'s currently-materialized occurrence advances the series to its next occurrence (materialized lazily, not eagerly created) rather than spawning a new plain item — individual items themselves don't recur anymore.

On a team-backed project, completing an assigned, points-bearing item awards
those points to the assignee — see `prl teams activity`/`undo-activity`
below.

### Get item details

```sh
prl items get <item-id> --project <project-id>
```

The output includes `assigned`/`points` (blank/`-` on a personal project,
where they're never set) and `priority` (`-` if unset, on any project).

### Delete an item

```sh
prl items delete <item-id> --project <project-id>
```

Deletes the item and all its descendants.

### List items due in a window

```sh
prl items due
prl items due --after 2026-06-01 --before 2026-06-30
```

Output includes the parent item name so you can see context at a glance.
This is a cross-project query — there's no `--project` flag, since it's
meant to answer "what's due" regardless of which project it lives in.

### List items assigned to you

```sh
prl items assigned
```

Shows team items other members have assigned to you, across all teams,
regardless of due date. Also cross-project, for the same reason as `due`
above.

> **Note:** there's still no dedicated `prl items assign`/`unassign` — set
> assignment at creation with `prl items add --project ... --assign
> <user-id>`, or re-assign by re-running `prl items add` and deleting the
> old item, or use the web UI/MCP server for an in-place change.

---

## Users

### List all users

```sh
prl users list
```

### Get user details

```sh
prl users get              # uses configured default user
prl users get <user-id>    # explicit user
```

### Send an app invite

Sends an email to the given address with a link to the app. The invite appears to come from your name (as shown in the app) via the shared SMTP account. The recipient must sign up via the web UI to get an account before they can be added to a team.

```sh
prl users invite someone@example.com
```

Requires SMTP to be configured on the server (`TODO_SMTP_USER` and `TODO_SMTP_PASSWORD`).

### Set your timezone

Sets your IANA timezone (e.g. `America/New_York`), used by Google Calendar import to resolve all-day event dates into the correct day for you. Without this set, imported all-day events default to UTC and may display a day off.

```sh
prl users set-timezone America/New_York
```

---

## Teams

Teams group users together so they can assign tasks to one another. Joining
requires mutual agreement: one member invites an existing user, and that
user must accept before they become an active member. Anyone can invite
others; you can only remove yourself (decline an invite or leave a team
you're in).

Each team member has a **role** — `admin` or `member` — and a **points**
balance, both scoped to that specific team (a user can be an admin of one
team and a plain member of another). The team's creator becomes its first
admin automatically. Only an admin can set a `points` value on a top-level
team item or change another member's role, and a team can never be left
with zero admins — demoting the last one is rejected.

Team items themselves — create/list/get/done/delete, including setting
`points`/`assignedToUserId` — are managed through `prl items ... --project
<project-id>` once a team has a project (every team gets one automatically;
see [Projects](#projects)), not through `prl teams`. The commands below
cover team membership, roles, and the points activity log.

### List your teams

Shows teams you're an active member of, plus any pending invites.

```sh
prl teams list
```

### Create a team

You become its first (active) member and its first admin.

```sh
prl teams create "Family"
```

### List a team's members

You must already be a member (pending or active) of the team. Shows each
member's status, role, points balance, and name.

```sh
prl teams members <team-id>
```

### Rename a team

You must already be an admin of the team.

```sh
prl teams rename <team-id> "New name"
```

### Invite an existing user

You must be an active member of the team. The invitee shows up with status
`PENDING` until they accept.

```sh
prl teams invite <team-id> <invitee-user-id>
```

### Accept a pending invite

```sh
prl teams accept <team-id>
```

### Leave a team (or decline an invite)

If this removes the last member, the team itself is deleted.

```sh
prl teams leave <team-id>
```

### Promote or demote a member

You must already be an admin of the team.

```sh
prl teams set-role <team-id> <target-user-id> admin
prl teams set-role <team-id> <target-user-id> member
```

### View a team's activity log

Every completed, points-bearing team item leaves an entry here (most recent
first). Completing a team item awards its `points` value to whoever it's
assigned to; un-completing a non-recurring item automatically reverses that
award.

```sh
prl teams activity <team-id>
```

### Undo a specific activity log entry

Reverses one entry's points directly by ID, independent of the item's
current state — this is the only way to undo a **recurring** item's
completion, since completing one deletes the old item row entirely and
replaces it with the next occurrence. Only the entry's own user (whoever
earned or lost the points) can undo it, and an already-reversed entry can't
be undone again.

```sh
prl teams undo-activity <team-id> <entry-id>
```

---

## Projects

A **project** is the one namespace every item belongs to. Every user gets
an auto-created personal project ("Personal") the first time they log in,
and every team gets its own project automatically the first time it's
created — you don't need to create or attach anything by hand to start
using `prl items ... --project <project-id>` on a team's items. A project
is either personal (no attached team, only its owner can access it) or
team-backed (attached to exactly one team; every active member of that team
can access it) — attach/detach change which one it is. Role (`admin`/
`member`) and points on a project are tracked separately from the
underlying team's own role/points (see [Teams](#teams)) — in practice
they're kept in sync automatically whenever the attached team's own
membership changes, so you rarely need `prl projects set-role` directly
except to fine-tune a project without touching the team.

### List your projects

```sh
prl projects list
```

### Create a personal project

Creates a new project with no attached team — just another namespace of
your own, alongside the auto-created "Personal" one.

```sh
prl projects create "Side hustle"
```

### Delete a project

Permanently deletes a project and everything in it — every task, event,
series, calendar subscription, and its points activity history. You must be
a project admin. Your own personal project (the one auto-created for you at
signup) can't be deleted this way. This cannot be undone.

```sh
prl projects delete <project-id>
```

### List a project's members

Shows each member's role, points balance (team-backed projects only — see
[Teams](#teams) for how points/completion work), and name.

```sh
prl projects members <project-id>
```

### Attach or detach a team

Attaching seeds project membership from the team's current active members;
detaching removes it (keeping the project's owner). You must already be a
project admin. A project can have at most one attached team at a time, but
the same team can back multiple projects.

```sh
prl projects attach-team <project-id> <team-id>
prl projects detach-team <project-id>
```

### Promote or demote a project member

You must already be a project admin.

```sh
prl projects set-role <project-id> <target-user-id> admin
prl projects set-role <project-id> <target-user-id> member
```

### Google Calendar subscriptions

Subscribe a project to a private Google Calendar (its "Secret address in
iCal format" from Google Calendar's settings) to import its events, read-only,
into that project's Events screen. A background task re-syncs every ~15
minutes (and once immediately on subscribe); imported events can't be edited
or deleted directly — unsubscribe to remove them, or edit the source calendar
in Google and wait for the next sync. You must be a project admin to
subscribe or unsubscribe; any project member can list subscriptions.

```sh
prl projects calendar-add <project-id> <ical-url>
prl projects calendar-list <project-id>
prl projects calendar-remove <project-id> <subscription-id>
```

`calendar-remove` also deletes every event that subscription imported.

---

## Recurrence patterns

Individual items don't recur anymore — recurring things are modeled as an
[item series](#item-series) instead (`prl series create --recurrence ...`).
The series' `--recurrence` understands the same natural English phrases the
old per-item flag used to:

| Pattern | Example |
|---|---|
| Every N days | `every 3 days` |
| Every N weeks | `every 2 weeks` |
| Every N months | `every month` |
| Every N years | `every year` |
| Day of month | `every month on the 15th` |
| Day of week | `every monday` |

A child item (created with `--parent`) or an event-linked task (created with
`--source-event-id`) still can't have any recurrence of its own — instead it
has an offset (days before its top-level item's or linked event's due date,
set with `--days-before-due`), which is used to recompute its due date
whenever the top-level item is edited or the linked event is rescheduled.

---

## Item Series

An **item series** is how a recurring Task or Event is modeled: a
recurrence rule plus an anchor date and a set of static fields (name,
description, item type). Individual occurrence dates aren't items at all
until something actually materializes one, via the web UI's calendar/
dashboard/Tasks-list views (`prl series` itself only covers the series:
create, read, update, list — there's no CLI browse/materialize command).
A series's `recurrence` field uses the exact same English-phrase syntax as
the table above. Every series has an `--item-type` of either
`task` or `event`, controlling whether it materializes Task or Event
occurrences — there's no default, so it must always be given explicitly on
`create`/`update`. `event_type` is not currently supported on any series.

### List a project's item series

```sh
prl series list <project-id>
```

### Create an item series

`<anchor>` accepts the same `YYYY-MM-DD`-or-Unix-timestamp format as
`--due`/`--scheduled` elsewhere.

```sh
prl series create <project-id> "Standup" "every weekday" 2026-08-17 --item-type event
prl series create <project-id> "Standup" "every weekday" 2026-08-17 \
  --item-type event --description "Daily sync"
```

`--basis schedule` (the default), `--basis completion`, or `--basis
due-date` controls what the next occurrence is measured from and which date
field each materialized occurrence gets: `schedule` uses the fixed
recurrence rule and materializes onto the scheduled date; `completion`
measures the next occurrence from when the current one was actually
completed or skipped instead (only valid with an "every N
days/weeks/months/years" recurrence — not a fixed weekday or day-of-month);
`due-date` still follows the fixed schedule, but materializes each
occurrence onto its due date instead of its scheduled date. Both
`completion` and `due-date` are only valid on a task series (`--item-type
task`).

```sh
prl series create <project-id> "Water plants" "every 3 days" 2026-08-17 \
  --item-type task --basis completion
prl series create <project-id> "Pay rent" "every month on the 1st" 2026-09-01 \
  --item-type task --basis due-date
```

`--template <item-id>` links the series to an existing Template item — create
one (and add its children) via the web UI's Templates screen for the
project, then pass its id here. Every occurrence this series materializes
copies that template's children onto it — the children definition stays
stable and independently editable, not tied to any one materialized
occurrence. Only valid on a task series. There's no CLI command for creating
templates themselves.

```sh
prl series create <project-id> "Water plants" "every 3 days" 2026-08-17 \
  --item-type task --template <template-item-id>
```

`--assign <user-id>` fixes every materialized occurrence's assignee, and
`--points <n>` awards that many points to the assignee on completion — both
only valid on a task series (`--item-type task`) on a team-backed project,
and `--points` further requires that project's admin (silently dropped
otherwise). `--priority <1-4>` sets the priority every materialized
occurrence inherits — only valid on a task series, but unlike `--assign`/
`--points` it's not restricted to a team-backed project.

As an alternative to a fixed `--assign`, `--rotate <user-id>` (repeatable)
rotates the assignee through a set of users, one per occurrence, cycling in
order of user id — e.g. occurrence 1 goes to the lowest user id given,
occurrence 2 to the next, wrapping back around after the last. `--rotate`
and `--assign` are mutually exclusive; pass `--rotate` two or more times to
build the rotation:

```sh
prl series create <project-id> "Take out trash" "every week" 2026-08-24 \
  --item-type task --rotate <user-id-1> --rotate <user-id-2> --rotate <user-id-3>
```

Removing someone from the rotation, or a rotation member leaving the
project, doesn't need special handling — see `docs/assignment-rotation-plan.md`
if you're curious why. A single occurrence's assignee can always be
overridden afterward like any other item field; it won't be recomputed once
materialized.

### Show one item series

```sh
prl series get <project-id> <series-id>
```

`prl series get` prints the resolved rotation (if any) as a comma-separated
list of user ids under `rotation:`.

### Update an item series

Update is a full replace of `name`/`recurrence`/`anchor`/`description`/
`item-type`/`basis`/`template`/`assign`/`points`/`priority`/`rotate` — pass
`--description`/`--basis`/`--template`/`--assign`/`--points`/`--priority`/
`--rotate` again to keep them, or omit to clear them (omitting `--basis`
resets to `schedule`; omitting both `--assign` and `--rotate` clears
whichever assignment mode was set). `--item-type` is required on every
update, the same as on create.

```sh
prl series update <project-id> <series-id> "Standup" "every weekday" 2026-08-17 \
  --item-type event --description "Daily sync"
```

### Delete an item series

Orphan, not cascade: this deletes the series itself, but any occurrences
already materialized from it are kept as plain standalone items, untouched.

```sh
prl series delete <project-id> <series-id>
```

---

## Tips

**Quick daily review**

```sh
prl items due --before $(date -d "+7 days" +%Y-%m-%d)
```

**Pipe IDs into other commands**

The ID column is always first, fixed-width, and space-separated — easy to cut:

```sh
prl items list --project <project-id> | grep "shopping" | awk '{print $1}' | xargs prl items done --project <project-id>
```

**Use environment variables in scripts**

```sh
export TODO_URL=https://todo.lapinel-fam.club
export TODO_TOKEN=eyJhbGci...
export TODO_USER=<your-user-id>

prl items add "Automated task" --project <project-id>
```
