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
prl items add "Water plants" --recurrence "every week" --project <project-id>
prl items add "Chapter notes" --parent <parent-item-id> --project <project-id>
prl items add "Pack bag" --parent <parent-item-id> --due-offset-days -1 --project <project-id>
prl items add "Team offsite" --item-type event --due 2026-09-01 --project <project-id>
prl items add "Rain today" --item-type event --event-type rain --project <project-id>
prl items add "Write report" --scheduled 2026-06-18 --scheduled-end 2026-06-20 --project <project-id>
prl items add "Milk" --item-type simple --project <project-id>
prl items add "Trip planning" --description "Book flights, hotel, and rental car" --project <project-id>
prl items add "Buy cake" --source-event-id <event-item-id> --due-offset-days -2 --project <project-id>
```

`--description` is free-form notes text (up to 5000 characters), separate from the
required `name`, and valid on every item type/kind.

`--item-type` is `task` (default), `event`, or `simple` — events are calendar-style
items, distinguished from tasks mainly for display purposes; simple items are a bare
checkable name with no due date, scheduled window, recurrence, or due offset (the
server rejects any of those on a `simple` item). `--event-type` is a free-text
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
`--due-offset-days`, drives the task's due date instead of a manually-typed
one — the server computes it from the referenced event's own due/scheduled
date plus the offset, ignoring any `--due` you pass alongside it.

`--due`, `--scheduled`, and `--scheduled-end` all accept `YYYY-MM-DD` or a Unix
timestamp. `--due` is the deadline (drives recurrence and offset-based child due
dates); `--scheduled`/`--scheduled-end` describe an optional start→end window —
when you actually plan to do it — and apply to tasks just as much as events. Note
that a child item (`--parent`) or event-linked task (`--source-event-id`) can't
use `--scheduled`/`--scheduled-end` at all — the server rejects it, since their
only supported date is the offset-derived due date.

`--project <project-id>` is required on every `add` (see the note at the
top of this section); `--assign <user-id>`/`--points <n>` only take effect
on a team-backed project (silently dropped by the server on a personal
one):

```sh
prl items add "Mow the lawn" --project <project-id> --assign <user-id> --points 25
```

> **Note:** `--recurrence` only works on items with no `--parent`/
> `--source-event-id` — child and event-linked items can't have their own
> recurrence, and `prl` rejects the combination before it ever reaches the
> server. Use `--due-offset-days` instead (days from the top-level item's or
> linked event's due date, negative = before, positive = after) — the offset
> is what actually sets the item's due date whenever the top-level item
> recurs or the linked event is rescheduled.

### Mark complete

```sh
prl items done <item-id> --project <project-id>
```

If the item has a recurrence rule, completing it automatically spawns the next occurrence with an updated due date, and any child items are carried over to the new occurrence — with their own due dates recalculated from their offset (or cleared, if they have none). A child's due date is always recalculated this way when its top-level ancestor recurs; manually setting one in the meantime won't survive the next recurrence.

On a team-backed project, completing an assigned, points-bearing item awards
those points to the assignee — see `prl teams activity`/`undo-activity`
below.

### Get item details

```sh
prl items get <item-id> --project <project-id>
```

The output includes `assigned`/`points` (blank/`-` on a personal project,
where they're never set).

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

---

## Recurrence patterns

When adding an item with `--recurrence`, the system understands natural English phrases:

| Pattern | Example |
|---|---|
| Every N days | `every 3 days` |
| Every N weeks | `every 2 weeks` |
| Every N months | `every month` |
| Every N years | `every year` |
| Day of month | `every month on the 15th` |
| Day of week | `every monday` |

When a recurring item is marked done, it is replaced by a new item with the next computed date. The recurrence basis — due date, completion date, or scheduled date — controls both which date the next occurrence is measured from and which field (`due` or `scheduled`) it's written into; due-date basis writes the new due date, the other two write the new scheduled date instead. Basis must be set via the web app — `prl` has no flag for it yet.

Recurrence only applies to top-level items with no `--source-event-id` — a child item (created with `--parent`) or an event-linked task (created with `--source-event-id`) can't have its own recurrence. Instead, either can have an offset (days from its top-level item's or linked event's due date, set with `--due-offset-days`), which is used to recompute its due date whenever the top-level item recurs or the linked event is rescheduled/recurs.

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
