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
```

`--due` accepts `YYYY-MM-DD` or a Unix timestamp.

### Mark complete

```sh
prl items done <item-id>
```

If the item has a recurrence rule, completing it automatically spawns the next occurrence with an updated due date.

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
