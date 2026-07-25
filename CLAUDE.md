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
todo-cli/src/client.rs    ← prl CLI (standalone crate, uses generated todo-client)
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

The service is also annotated `@httpBearerAuth`. This only affects **client** codegen (generic smithy-rs `codegen-client`, not AWS-specific) — it generates a `Config::builder().bearer_token(Token::new(...))` method and a `BearerAuthScheme` that auto-attaches the `Authorization: Bearer` header on `todo-client`. Server-side codegen (`codegen-server`, → `todo-server-sdk`) has no handling for HTTP auth traits at all — adding/removing this trait never changes generated server code. Actual token verification is, and must remain, hand-written in `src/auth.rs`; the trait only standardizes how the client carries the credential, not how the server validates it.

### Server (`src/main.rs`)

Handlers receive dependencies via `server::Extension` (injected via `tower::ServiceBuilder` layers). The smithy service is nested under `/api`; static frontend assets are served from `frontend/dist/` via axum's `ServeDir`.

Auth mode is selected at startup via `TODO_AUTH_MODE` (see Auth section below).

### Auth (`src/auth.rs`)

Two modes, selected by the `TODO_AUTH_MODE` env var:

- **`internal`** (default) — full Google OAuth + JWT flow. Requires `TODO_GOOGLE_CLIENT_ID`, `TODO_GOOGLE_CLIENT_SECRET`, `TODO_JWT_SECRET`. Mounts `/auth/google`, `/auth/callback`, `/auth/logout`, `/auth/me`, `/auth/token` routes. The `/auth/token` endpoint issues a long-lived JWT for use by CLI/MCP clients.
- **`caddy`** — also requires `TODO_JWT_SECRET` now (used to verify Bearer tokens, see below). Mounts `/auth/me` and `/auth/token`. `caddy_header_middleware` accepts identity two ways: (1) the `x-token-user-email` header injected by caddy-security for browser sessions — trusted as-is, no signature check, because only caddy-security can set it; or (2) an `Authorization: Bearer <jwt>` header for CLI/MCP requests, verified via `jsonwebtoken::decode` against `TODO_JWT_SECRET` — same check `jwt_auth_middleware` does for `internal` mode. `caddy_auth_token` mints tokens the same way `auth_token` does in `internal` mode, but resolves identity from `x-token-user-email` instead of a session cookie. Set `TODO_DEV_EMAIL=you@example.com` to bypass the header check for local dev without Caddy in front.

  **Cross-repo dependency:** for a Bearer-token request to ever reach path (2) above, the edge reverse proxy must not intercept it first. In production this is caddy-security sitting in front of everything — its `authorize with mypolicy` directive would otherwise 302-redirect *any* unauthenticated request (including ones carrying our own JWT, which caddy-security doesn't recognize) to its login portal before it ever reaches this app. The home-server repo's `Caddyfile.local` gives `todo.lapinel-fam.club` its own dedicated site block with a matcher that skips `authorize` specifically for requests carrying `Authorization: Bearer`, letting them fall through to this middleware instead. See that repo's `CLAUDE.md` for the Caddyfile side of this. If that Caddyfile change is ever reverted, path (2) becomes unreachable in production even though the app code still supports it.

In both modes, `AuthUser { user_id }` is injected into request extensions by the respective middleware and is available to handlers.

**Browser SPA client gotcha:** the `@httpBearerAuth` trait on the service (see Smithy section above) makes the *generated TS client*, not just the Rust one, refuse to send any request without a configured bearer identity — it throws `HttpAuthScheme 'smithy.api#httpBearerAuth' did not have an IdentityProvider configured` client-side, before the request ever leaves the browser, breaking the whole app (empty `#app` div, uncaught promise rejection in the console). The browser SPA doesn't use a bearer token at all — it's authenticated via the `todo_auth` cookie (internal mode) or the edge-injected `x-token-user-email` header (caddy mode), both handled server-side as described above. `frontend/src/main.ts`'s `PeoplesRepublicOfListsClient` construction works around this by passing a `httpAuthSchemes` override with a no-op identity provider and signer for `smithy.api#httpBearerAuth`, so the client proceeds without ever attaching an `Authorization` header, leaving the browser's normal same-origin cookie/header flow untouched. If `task codegen` regenerates `todo-typescript-client` and this override is ever lost or the scheme id changes, the app will fail exactly this way again.

### Storage Layer (`src/storage/`)

`mod.rs` defines `UserRepo` and `ItemRepo` traits (annotated with `mockall::automock` for testing). Three implementations: `sqlite.rs` (active), `memory.rs`, `dynamo.rs`.

`UserRepo` methods include `get_or_create_by_google_id` (used by internal OAuth flow) and `get_or_create_by_email` (used by caddy middleware — looks up by email, creates a record if none exists).

SQLite schema is created/migrated inline in `create_pool()`. Additive schema migrations use `ALTER TABLE ... ADD COLUMN` with the error ignored (handles existing DBs where the column already exists).

### Domain Models (`src/domain/`)

Plain Rust structs (`User`, `Item`) — no framework coupling. `Item` stores `deadline` as `Option<DateTime<Utc>>`, `recurrence` as a raw English string, `recurrence_basis` as `Option<String>` (`"DUE_DATE"` or `"COMPLETION_DATE"`), and `due_offset_days` as `Option<i32>` (see Recurrence below).

`Item::next_recurrence(now, tz_offset_minutes)` (`src/domain/item.rs`) is the domain-level decision of whether a completed item should recur and what its successor looks like: returns `None` unless `complete` and `recurrence` are both set and the pattern parses, otherwise a fresh (`id` cleared, `complete: false`) clone with `deadline` advanced via `recurrence::next_date`. `Item::deadline_from_offset(root_deadline, tz_offset_minutes)` is the analogous decision for a child: `due_offset_days.map(|n| end_of_day(root_deadline + n days))`, `None` if no offset is set. Both are pure and unit-tested in isolation from the handlers that call them.

### Recurrence (`src/domain/recurrence.rs`)

Custom English-phrase parser supporting: `every N days/weeks/months/years`, `every month on the Nth`, `every [weekday]`. `parse()` returns a `RecurrenceRule`; `next_date()` computes the next UTC datetime (advancing past the present if cycles were missed).

**Only top-level items (no `parentItemId`) can have a `recurrence`.** `create_item`/`update_item`/`create_team_item`/`update_team_item` reject any request that sets both `recurrence` and `parentItemId`. Child items instead carry a `dueOffsetDays: Option<i32>` — days from the top-level item's due date (negative = before, positive = after).

When a recurring item is marked complete in `update_item`/`update_team_item`, the handler calls `Item::next_recurrence` to build the successor, persists it, then calls `clone_children` (`src/handlers/mod.rs`) to recursively re-parent the entire old subtree onto the new item's id — every descendant gets a fresh id, `complete: false`, and a deadline recomputed as `deadline_from_offset(new_root_deadline, tz_offset)` (or `None` if it has no offset). The old subtree (parent and all descendants) is then deleted. Two things worth remembering:

- The offset reference is always the item that actually recurred (the root of this clone operation), not each descendant's immediate parent — a grandchild's offset is measured from the same root a direct child's is, not chained through an intermediate parent's own (offset-derived) deadline.
- A child's *prior* deadline (however it got there — manual edit, a previous offset computation) is never read during this recompute; it's offset-or-`None`, full stop. The frontend surfaces this as a warning near due-date fields on child items: manual edits don't survive the next recurrence of an ancestor.

### Frontend (`frontend/src/main.ts`)

Single-file TypeScript SPA using the History API for routing (`/`, `/users/:id`, `/users/:id/items/:id`). Imports the generated `@todo/client` package (symlinked from `todo-typescript-client/`). Built with Vite (content-hashed filenames — hard refresh needed after rebuild during dev).

`renderItems` handles both the top-level item list and, when passed a `parentItemId`, that item's children — the same create/edit forms are reused for both, toggling which fields show based on nesting: a "Repeat" field for top-level items, an "Offset days" field instead for children (mirroring the constraint in the Recurrence section above). Per-child rows show an editable offset label (`+2d`/`-3d`/`on due date`/`no offset`) next to the due date, and the due-date field carries a warning that it's recalculated from the offset whenever an ancestor recurs — manual edits to a child's due date don't survive that. The Checklists screen (`renderChecklistDetail`, reached via `/users/:id/checklists`) is a separate, parallel feature for building `isTemplate` checklists and setting offsets on their items; those items never carry real dates (templates have no dates — see `create_template`), so the deadline-recalculation concern doesn't apply there. The "Use" button (in `renderChecklists`, the list screen one level up) creates the top-level item, carries over the template's own `recurrence`/`recurrenceBasis`, then calls `copyTemplateChildren` to recursively recreate the template's child structure under the new item — each child's initial due date is computed from its `dueOffsetDays` against the new item's actual due date (re-fetched via `GetItemCommand` after creation, since `create_item` may have auto-computed a deadline from a carried-over recurrence pattern even when the "Use" form's date field was left blank). If the new item has no due date at all, children are created with no due date either, regardless of any offset — same "no root, no derived date" rule as recurrence-driven offset computation.

Team items are handled by `renderTeamItems`, which mirrors `renderItems`'s recursive drill-down pattern exactly (`renderTeamItemDetail`, a separate one-level-deep duplicate with no date/recurrence/offset/assignee fields, was deleted in favor of this — the router's `/teams/:id/items/:id` match now calls `renderTeamItems(teamId, itemId, item.name)` the same way the personal-item route calls `renderItems`). Same Repeat-vs-Offset toggle, same per-child offset label and due-date recalculation warning, plus an "Assign to" dropdown (create and edit forms both) scoped to that team's active members via `loadTeamMembers(userId, teamId)` — assignment is the one property unique to team items, and previously had no UI at all (just a raw-user-id badge in the list, now resolved to the member's name).

### CLI (`todo-cli/`)

A standalone Rust crate that builds the `prl` binary. Async (`#[tokio::main]`), uses the generated `todo-client` crate directly rather than hand-rolled `reqwest`/JSON structs. `client.rs` builds a `todo_client::Client` via `Config::builder().endpoint_url(...).bearer_token(Token::new(token, None)).build()` — the `@httpBearerAuth` trait (see Smithy section above) generates that `bearer_token` method, so there's no hand-written request interceptor. Config stored at `~/.config/prl/config.toml`. See `docs/prl-user-guide.md` for usage.

No `prl items assign`/`unassign` — the generated `CreateItemInput`/`UpdateItemInput` builders have no `assigned_to_user_id` setter (assignment is a team-item-only concept, not exposed via these operations), so the CLI can't send it at all.

`prl items add` rejects `--parent`/`--recurrence` together client-side with an explanatory error, mirroring the server-side rule (recurrence is top-level-only — see Recurrence above), and has a `--due-offset-days` flag for setting a child's offset. `prl items done` forwards the fetched item's `dueOffsetDays` back on the completion `update_item` call — without this, marking a child item done would have silently wiped its offset, the same class of round-trip bug the frontend had before its own fix (see Recurrence above).

Build/install separately from the main server (`cargo` commands run from `todo-cli/` or via `task cli-build` / `task cli-install`).

## MCP Server (`mcp-server/`)

A Claude Code MCP server wrapping the todo API. Registered at `~/rust-projects/todo/.mcp.json` — picked up automatically when Claude Code opens this directory.

**Build:**
```sh
cd mcp-server && npm install && npm run build
```
Output lands in `mcp-server/dist/index.js`. Must be built before the MCP server works.

**Auth — how to get `TODO_API_TOKEN`:**

The MCP server authenticates with a long-lived JWT, minted by `/auth/token`. This now works directly in either auth mode — no more temporarily flipping `TODO_AUTH_MODE` to `internal` to mint one.

1. Log in through the browser: `https://todo.lapinel-fam.club/auth/google` in `internal` mode, or just load the site in `caddy` mode (caddy-security's own Google OAuth portal handles it)
2. With the session established, visit `https://todo.lapinel-fam.club/auth/token` in the same browser
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

- **CLI has no team item support** — `todo-cli/` was not updated when team items were added. Team items (create/list/update/delete under `/teams/{teamId}/items`) are accessible via the web UI and MCP server only; `todo-cli/src/teams.rs` only covers team CRUD/membership, not team items.
