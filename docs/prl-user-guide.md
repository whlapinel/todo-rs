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

### List items

```sh
prl items list
```

Output columns: `ID`, `DONE`, `DUE`, `NAME`. Items with sub-tasks show a `▸` suffix.

### List sub-tasks

```sh
prl items list --parent <item-id>
```

### Add an item

```sh
prl items add "Buy groceries"
prl items add "Submit report" --due 2026-06-20
prl items add "Water plants" --recurrence "every week"
prl items add "Chapter notes" --parent <parent-item-id>
prl items add "Pack bag" --parent <parent-item-id> --due-offset-days -1
```

`--due` accepts `YYYY-MM-DD` or a Unix timestamp.

> **Note:** Assignment is no longer supported on personal items. To create an
> assignable task, use the web UI to create a team item under a team you
> belong to (see [Teams](#teams)).

> **Note:** `--recurrence` only works on items with no `--parent` — child
> items can't have their own recurrence, and `prl` rejects the combination
> before it ever reaches the server. Use `--due-offset-days` on a child
> instead (days from the top-level item's due date, negative = before,
> positive = after) — the offset is what actually sets the child's due date
> whenever the top-level item recurs.

### Mark complete

```sh
prl items done <item-id>
```

If the item has a recurrence rule, completing it automatically spawns the next occurrence with an updated due date, and any child items are carried over to the new occurrence — with their own due dates recalculated from their offset (or cleared, if they have none). A child's due date is always recalculated this way when its top-level ancestor recurs; manually setting one in the meantime won't survive the next recurrence.

### Get item details

```sh
prl items get <item-id>
```

### Delete an item

```sh
prl items delete <item-id>
```

Deletes the item and all its descendants.

### List items due in a window

```sh
prl items due
prl items due --after 2026-06-01 --before 2026-06-30
```

Output includes the parent item name so you can see context at a glance.

### List items assigned to you

```sh
prl items assigned
```

Shows team items other members have assigned to you, across all teams,
regardless of due date.

> **Note:** `prl items assign` / `prl items unassign` have been removed.
> Assignment is now a team-item-only concept and is managed via the web UI or
> MCP server. CLI support for team items is planned.

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

Teams group users together so they can eventually assign tasks to one
another. Joining requires mutual agreement: one member invites an existing
user, and that user must accept before they become an active member. Anyone
can invite others; you can only remove yourself (decline an invite or leave
a team you're in) — there's no admin role.

### List your teams

Shows teams you're an active member of, plus any pending invites.

```sh
prl teams list
```

### Create a team

You become its first (active) member.

```sh
prl teams create "Family"
```

### List a team's members

You must already be a member (pending or active) of the team.

```sh
prl teams members <team-id>
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

When a recurring item is marked done, it is replaced by a new item with the next computed due date. The recurrence basis (due date vs. completion date) controls how that date is calculated and must be set via the web app.

Recurrence only applies to top-level items — a child item (created with `--parent`) can't have its own recurrence. Instead, a child can have an offset (days from its top-level item's due date, set with `--due-offset-days`), which is used to recompute the child's due date whenever the top-level item recurs.

---

## Tips

**Quick daily review**

```sh
prl items due --before $(date -d "+7 days" +%Y-%m-%d)
```

**Pipe IDs into other commands**

The ID column is always first, fixed-width, and space-separated — easy to cut:

```sh
prl items list | grep "shopping" | awk '{print $1}' | xargs prl items done
```

**Use environment variables in scripts**

```sh
export TODO_URL=https://todo.lapinel-fam.club
export TODO_TOKEN=eyJhbGci...
export TODO_USER=<your-user-id>

prl items add "Automated task"
```
