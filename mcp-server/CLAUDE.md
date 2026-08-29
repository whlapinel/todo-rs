# CLAUDE.md (mcp-server/)

Guidance for working on the MCP server. See the root `CLAUDE.md` for the overall architecture this wraps.

## MCP Server (`mcp-server/`)

A Claude Code MCP server wrapping the todo API. Registered at `~/todo/.mcp.json` — picked up automatically when Claude Code opens this directory.

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
      "args": ["-c", "set -a; source /home/whlapinel/todo/.env; set +a; exec node /home/whlapinel/todo/mcp-server/dist/index.js"]
    }
  }
}
```

**`.env` file** (gitignored — create at project root):
```
TODO_API_URL=https://todo.lapinel-fam.club
TODO_API_TOKEN=<paste JWT here>
```

**Project tools** (see `docs/project-abstraction-plan.md`, stage B7): `list_projects`/`create_project`/`get_project`/`update_project`/`delete_project`/`list_project_members`/`set_project_member_role`/`attach_team_to_project`/`detach_team_from_project`, mirroring `prl projects`' shape. `list_items`/`get_item`/`create_item`/`update_item`/`delete_item` have a `projectId` parameter, **required** as of Stage C3 (`projectId` is in each tool's JSON-Schema `required` list in `mcp-server/src/index.ts`) — those five route exclusively through the `ProjectItem` operations now, mirroring `prl items`' own `--project` (see `todo-cli/CLAUDE.md`). `list_items_due`/`list_assigned_items` have no `projectId` variant, same reasoning as `prl items due`/`assigned`. `create_item`/`update_item`'s `assignedToUserId`/`points` are plain optional parameters now (no longer conditionally required on `projectId`, since `projectId` is unconditionally required for everything else on those tools).

A separate, older set of tools — `list_team_items`/`get_team_item`/`create_team_item`/`update_team_item`/`delete_team_item` — used to exist here and call `/teams/:teamId/items...` REST paths; that route was never re-wired to anything after `TeamItem`'s Smithy surface was removed in Stage C3, so every call from those five 404ed against the live server (evidently missed when Stage B7 "repoint[ed] item tools at `ProjectItem`" — only the personal-item-shaped tools were repointed, not this team-item-shaped set). Removed rather than repointed, since the `list_items`/`get_item`/`create_item`/`update_item`/`delete_item` family above already covers team-backed projects via `projectId`.

`create_item`/`update_item`'s `daysBeforeDue` parameter (renamed 2026-08-28 from `dueOffsetDays`, see root CLAUDE.md's Recurrence section) is non-negative-only — `toDueOffsetDays` throws if the caller passes a negative number, and otherwise negates it into the `dueOffsetDays` field the API itself still expects (that field's own negative-before/positive-after sign convention is unchanged; the server now just rejects a positive value outright). This is a client-side presentation convenience only, matching the web UI's and `prl`'s own sign flip — a tool response (`get_item`/`list_items`) still returns the raw, signed `dueOffsetDays` from the server, not negated back to `daysBeforeDue`.
