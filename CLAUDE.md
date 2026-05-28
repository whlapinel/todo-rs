# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
task codegen       # Generate Rust server SDK + TS client from .smithy files (slow first run, cached after)
task run           # codegen + frontend build + cargo run
task build         # codegen + frontend build + cargo build
task check         # codegen + cargo check (fastest Rust verify)
task frontend      # Build the TypeScript frontend only (outputs to frontend/dist/)
task client        # Build the generated TypeScript client package only
cargo test         # Run all Rust tests
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
todo-typescript-client/   ← generated TS client (DO NOT EDIT)
        │
        ▼
src/main.rs               ← handler implementations, axum server wiring
frontend/src/main.ts      ← TypeScript SPA, imports from @todo/client
```

### Smithy Model → Generated Code

The `.smithy` files in `model/src/main/smithy/` define the full API contract. The resource hierarchy is `User → List → Item` — child resources inherit parent identifiers, so item operations require `userId + listId + itemId`.

smithy-rs (a Gradle composite build via the `smithy-rs/` git submodule) generates:

- `todo_server_sdk::input::*` / `output::*` / `error::*` — typed structs per operation
- `Listeria` — the tower `Service` (HTTP routing + serde handled automatically)
- `ListeriaBuilder` — wires async handler functions to operations

Handler signature: `async fn op_name(input: input::OpInput, server::Extension(repo): server::Extension<Arc<dyn Repo>>) -> Result<output::OpOutput, error::OpError>`

### Server (`src/main.rs`)

Handlers receive dependencies via `server::Extension` (injected via `tower::ServiceBuilder` layers). All three repos (`UserRepo`, `ListRepo`, `ItemRepo`) are mounted as extensions. The smithy service is nested under `/api`; static frontend assets are served from `frontend/dist/` via axum's `ServeDir`.

### Storage Layer (`src/storage/`)

`mod.rs` defines `UserRepo`, `ListRepo`, `ItemRepo` traits (annotated with `mockall::automock` for testing). Three implementations: `sqlite.rs` (active), `memory.rs`, `dynamo.rs`.

SQLite schema is created/migrated inline in `create_pool()`. Additive schema migrations use `ALTER TABLE ... ADD COLUMN` with the error ignored (handles existing DBs where the column already exists).

### Domain Models (`src/domain/`)

Plain Rust structs (`User`, `List`, `Item`) — no framework coupling. `Item` stores `deadline` as `Option<DateTime<Utc>>`, `recurrence` as a raw English string, `recurrence_basis` as `Option<String>` (`"DUE_DATE"` or `"COMPLETION_DATE"`).

### Recurrence (`src/recurrence.rs`)

Custom English-phrase parser supporting: `every N days/weeks/months/years`, `every month on the Nth`, `every [weekday]`. `parse()` returns a `RecurrenceRule`; `next_date()` computes the next UTC datetime (advancing past the present if cycles were missed). When a recurring item is marked complete in `update_item`, the handler spawns a new item with the next deadline and deletes the completed one.

### Frontend (`frontend/src/main.ts`)

Single-file TypeScript SPA using the History API for routing (`/`, `/users/:id`, `/users/:id/lists/:id`). Imports the generated `@todo/client` package (symlinked from `todo-typescript-client/`). Built with Vite (content-hashed filenames — hard refresh needed after rebuild during dev).

## MCP Server (`mcp-server/`)

A Claude Code MCP server wrapping the todo API. Registered at `~/rust-projects/todo/.mcp.json` — picked up automatically when Claude Code opens this directory.

**Build:**
```sh
cd mcp-server && npm install && npm run build
```
Output lands in `mcp-server/dist/index.js`. Must be built before the MCP server works.

**Auth — how to get `TODO_API_TOKEN`:**

The server uses a long-lived JWT issued by the `/auth/token` endpoint. To generate one:

1. Visit `https://todo.lapinel-fam.club/auth/login` in a browser → complete Google OAuth
2. With the session cookie set, visit `https://todo.lapinel-fam.club/auth/token` in the same browser
4. Copy the `token` value from the JSON response
5. Paste it into `.mcp.json` as `TODO_API_TOKEN` — the token is valid for 365 days

**Server env vars** (required to start): `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `JWT_SECRET`. These are stored in `~/home-server` on this machine.

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
4. Add/update handler in `src/main.rs` and wire into `Listeria::builder(...)`

**Adding a DB column:**

1. Add the column to `CREATE TABLE IF NOT EXISTS` in `create_pool()` (sqlite.rs)
2. Add a `let _ = sqlx::query("ALTER TABLE ... ADD COLUMN ...").execute(&pool).await;` line after the CREATE (error ignored for existing DBs)
3. Update relevant SELECT/INSERT/UPDATE queries and row mapping

**Frontend-only change:**

- Edit `frontend/src/main.ts` → `cd frontend && npm run build` — no codegen or cargo needed
