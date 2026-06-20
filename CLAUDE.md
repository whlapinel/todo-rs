# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Git

Do not add `Co-Authored-By` trailers to commits.

## Commands

```sh
task codegen       # Generate Rust server SDK + TS client from .smithy files (slow first run, cached after)
task run           # codegen + frontend build + cargo run
task build         # codegen + frontend build + cargo build
task check         # codegen + cargo check (fastest Rust verify)
task frontend      # Build the TypeScript frontend only (outputs to frontend/dist/)
task client        # Build the generated TypeScript client package only
task cli-build     # Build the prl CLI binary (todo-cli/)
task cli-install   # Install prl to ~/.cargo/bin
cargo test         # Run all Rust tests
```

Live-server smoke test (requires `TODO_API_URL` + `TODO_API_TOKEN`):

```sh
set -a; source .env; set +a; ./scripts/smoke-test.sh
```

For frontend-only changes (no Smithy or backend changes): `cd frontend && npm run build`

For TS client-only changes: `cd todo-typescript-client && npm run build`

`task codegen` requires JDK 11+. It compiles the smithy-rs Kotlin codegen from source via a Gradle composite build — first run takes several minutes, subsequent runs are cached in `~/.gradle/caches/`.

## Architecture

### Full Pipeline

```text
model/src/main/smithy/*.smithy
        │  (task codegen / ./gradlew :model:assemble)
        ▼
todo-server-sdk/          ← generated Rust crate (DO NOT EDIT)
todo-client/              ← generated Rust client crate (DO NOT EDIT)
todo-typescript-client/   ← generated TS client (DO NOT EDIT)
        │
        ▼
src/main.rs               ← handler implementations, axum server wiring
frontend/src/main.ts      ← TypeScript SPA, imports from @todo/client
todo-cli/src/main.rs      ← prl CLI (standalone crate, uses reqwest directly)
```

### Smithy Model → Generated Code

The `.smithy` files in `model/src/main/smithy/` define the full API contract. Two Smithy resources own items:

- **`User → Item`** — personal items at `/users/{userId}/items/{itemId}`
- **`TeamItem`** — team-scoped resource with identifiers `{teamId, itemId}`, at `/teams/{teamId}/items/{itemId}`; assignment (`assignedToUserId`) is only valid here, checked against the specific team's active membership

`Team` itself is not a Smithy resource — team CRUD operations (`CreateTeam`, `GetTeam`, etc.) are plain service operations scoped under `/users/{userId}/teams`. Both item types share the `items` SQLite table (nullable `user_id` / `team_id`). Items support nesting via `parentItemId`.

smithy-rs (a Gradle composite build via the `smithy-rs/` git submodule) generates:

- `todo_server_sdk::input::*` / `output::*` / `error::*` — typed structs per operation
- `PeoplesRepublicOfLists` — the tower `Service` (HTTP routing + serde handled automatically)
- `PeoplesRepublicOfListsBuilder` — wires async handler functions to operations

Handler signature: `async fn op_name(input: input::OpInput, server::Extension(repo): server::Extension<Arc<dyn Repo>>) -> Result<output::OpOutput, error::OpError>`

### Server (`src/main.rs`)

Handlers receive dependencies via `server::Extension` (injected via `tower::ServiceBuilder` layers). The smithy service is nested under `/api`; static frontend assets are served from `frontend/dist/` via axum's `ServeDir`.

Auth mode is selected at startup via `TODO_AUTH_MODE` (see Auth section below).

### Auth (`src/auth.rs`)

Two modes, selected by the `TODO_AUTH_MODE` env var:

- **`internal`** (default) — full Google OAuth + JWT flow. Requires `TODO_GOOGLE_CLIENT_ID`, `TODO_GOOGLE_CLIENT_SECRET`, `TODO_JWT_SECRET`. Mounts `/auth/google`, `/auth/callback`, `/auth/logout`, `/auth/me`, `/auth/token` routes. The `/auth/token` endpoint issues a long-lived JWT for use by CLI/MCP clients.
- **`caddy`** — trusts the `x-token-user-email` header injected by caddy-security upstream. No OAuth env vars needed. The `/auth/*` routes are not mounted. Set `TODO_DEV_EMAIL=you@example.com` to bypass the header check for local dev without Caddy in front.

In both modes, `AuthUser { user_id }` is injected into request extensions by the respective middleware and is available to handlers.

### Storage Layer (`src/storage/`)

`mod.rs` defines `UserRepo` and `ItemRepo` traits (annotated with `mockall::automock` for testing). Three implementations: `sqlite.rs` (active), `memory.rs`, `dynamo.rs`.

`UserRepo` methods include `get_or_create_by_google_id` (used by internal OAuth flow) and `get_or_create_by_email` (used by caddy middleware — looks up by email, creates a record if none exists).

SQLite schema is created/migrated inline in `create_pool()`. Additive schema migrations use `ALTER TABLE ... ADD COLUMN` with the error ignored (handles existing DBs where the column already exists).

### Domain Models (`src/domain/`)

Plain Rust structs (`User`, `Item`) — no framework coupling. `Item` stores `deadline` as `Option<DateTime<Utc>>`, `recurrence` as a raw English string, `recurrence_basis` as `Option<String>` (`"DUE_DATE"` or `"COMPLETION_DATE"`).

### Recurrence (`src/recurrence.rs`)

Custom English-phrase parser supporting: `every N days/weeks/months/years`, `every month on the Nth`, `every [weekday]`. `parse()` returns a `RecurrenceRule`; `next_date()` computes the next UTC datetime (advancing past the present if cycles were missed). When a recurring item is marked complete in `update_item`, the handler spawns a new item with the next deadline and deletes the completed one.

### Frontend (`frontend/src/main.ts`)

Single-file TypeScript SPA using the History API for routing (`/`, `/users/:id`, `/users/:id/items/:id`). Imports the generated `@todo/client` package (symlinked from `todo-typescript-client/`). Built with Vite (content-hashed filenames — hard refresh needed after rebuild during dev).

### CLI (`todo-cli/`)

A standalone Rust crate that builds the `prl` binary. Uses `reqwest::blocking` to call the REST API directly (not the generated Smithy client). Config stored at `~/.config/prl/config.toml`. See `docs/prl-user-guide.md` for usage.

Build/install separately from the main server (`cargo` commands run from `todo-cli/` or via `task cli-build` / `task cli-install`).

## MCP Server (`mcp-server/`)

A Claude Code MCP server wrapping the todo API. Registered at `~/rust-projects/todo/.mcp.json` — picked up automatically when Claude Code opens this directory.

**Build:**
```sh
cd mcp-server && npm install && npm run build
```
Output lands in `mcp-server/dist/index.js`. Must be built before the MCP server works.

**Auth — how to get `TODO_API_TOKEN`:**

The MCP server authenticates with a long-lived JWT. This requires the server to be running in `internal` auth mode (the default before caddy-security was added). If running in `caddy` mode, obtain the token while the server is temporarily set to `internal` mode, or configure caddy-security API key support.

1. Visit `https://todo.lapinel-fam.club/auth/login` in a browser → complete Google OAuth
2. With the session cookie set, visit `https://todo.lapinel-fam.club/auth/token` in the same browser
3. Copy the `token` value from the JSON response
4. Paste it into `.mcp.json` as `TODO_API_TOKEN` — the token is valid for 365 days

**`.mcp.json` config** (token is loaded from `.env`, which is gitignored):
```json
{
  "mcpServers": {
    "todo": {
      "command": "bash",
      "args": ["-c", "set -a; source /home/whlapinel/rust-projects/todo/.env; set +a; exec node /home/whlapinel/rust-projects/todo/mcp-server/dist/index.js"]
    }
  }
}
```

**`.env` file** (gitignored — create at project root):
```
TODO_API_URL=https://todo.lapinel-fam.club
TODO_API_TOKEN=<paste JWT here>
```

---

## Key Workflows

**Adding/changing a Smithy operation:**

1. Edit `.smithy` file
2. `task codegen`
3. Fix Rust compile errors (the generated types changed)
4. Add/update handler in `src/main.rs` and wire into `PeoplesRepublicOfLists::builder(...)`

See the touch-point checklist below for all files that may need updating.

**Adding a DB column:**

1. Add the column to `CREATE TABLE IF NOT EXISTS` in `create_pool()` (sqlite.rs)
2. Add a `let _ = sqlx::query("ALTER TABLE ... ADD COLUMN ...").execute(&pool).await;` line after the CREATE (error ignored for existing DBs)
3. Update relevant SELECT/INSERT/UPDATE queries and row mapping

**Frontend-only change:**

- Edit `frontend/src/main.ts` → `cd frontend && npm run build` — no codegen or cargo needed

**Switching auth modes:**

- Set `TODO_AUTH_MODE=caddy` to trust caddy-security headers (no Google/JWT env vars needed)
- Set `TODO_AUTH_MODE=internal` (or unset) to use the built-in OAuth flow
- For local dev in caddy mode without Caddy: set `TODO_DEV_EMAIL=you@example.com`

---

## Touch-Point Checklist

Every place that must be updated when the Smithy model changes. The generated crates (`todo-server-sdk/`, `todo-client/`, `todo-typescript-client/`) update themselves via `task codegen` — everything else is hand-written and must be updated manually.

### Adding or removing an operation

| File | What to do |
|------|-----------|
| `model/src/main/smithy/*.smithy` | Add/remove the operation definition |
| *(run `task codegen`)* | Regenerates Rust SDKs + TS client source |
| *(run `task client`)* | Compiles the TS client source → `dist/`; required before the frontend build picks up new commands. Skippable if you use `task frontend` or `task build` (both depend on `client` automatically) |
| `src/main.rs` | Add/remove the handler function; wire/unwire from `PeoplesRepublicOfLists::builder(...)` |
| `src/handlers/mod.rs` | If creating a new handler file, add `pub mod <module>;` here |
| `frontend/src/main.ts` | Import/remove the generated `*Command`; add/remove UI for it |
| `mcp-server/src/index.ts` | Add/remove tool definition in the tools list; add/remove `case` in the switch |
| *(run `cd mcp-server && npm run build`)* | Recompile the MCP server source → `dist/index.js`; changes are not live until this runs |
| `todo-cli/src/main.rs` | Add/remove subcommand variant and match arm in the relevant `cmd_*` function |
| `docs/prl-user-guide.md` | Document the new command or remove it |

### Renaming the service

| File | What to change |
|------|---------------|
| `model/src/main/smithy/service.smithy` | `service OldName {` → `service NewName {` |
| `model/smithy-build.json` | All `"service": "common#OldName"` → `"service": "common#NewName"` |
| *(run `task codegen`)* | |
| `src/main.rs` | `OldName`, `OldNameConfig` imports and usage |
| `frontend/src/main.ts` | `OldNameClient` import and `new OldNameClient(...)` instantiation |

### Renaming an error type

| File | What to change |
|------|---------------|
| `model/src/main/smithy/errors.smithy` | Rename the `structure` |
| All other `.smithy` files | Update references in `errors: [...]` lists |
| `model/smithy-build.json` | No change needed (errors aren't referenced here) |
| *(run `task codegen`)* | |
| `src/main.rs` | `error::OldErrorName` usage in handler return types and `internal()`/`not_found()` helpers |

### Docker builds

`todo-server-sdk/` and `todo-typescript-client/` are gitignored (generated). The Dockerfile copies them from the local filesystem, so **`task codegen` must be run before `task docker-build`**. `task docker-build` and `task docker-release` do this automatically.

---

## Known Issues

- **`prl users list` panics** — `todo-cli/src/main.rs:323` calls `.unwrap()` on `.json()` for a response that isn't valid JSON (likely an auth error page or empty body). Fix: check HTTP status code before parsing the body.

- **CLI has no team item support** — `todo-cli/` was not updated when team items were added. Team items are accessible via the web UI and MCP server only. The `prl items assign` / `prl items unassign` subcommands have also been removed from the Smithy model (assignment is now a team-item-only concept). The CLI still sends `assignedToUserId` in `CreateItem` / `UpdateItem` requests, but the server ignores it.
