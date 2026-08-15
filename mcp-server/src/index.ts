#!/usr/bin/env node
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";

const API_URL = (process.env.TODO_API_URL ?? "http://localhost:3000").replace(/\/$/, "");
const API_TOKEN = process.env.TODO_API_TOKEN ?? "";

if (!API_TOKEN) {
  process.stderr.write("TODO_API_TOKEN is required\n");
  process.exit(1);
}

async function api(method: string, path: string, body?: unknown): Promise<unknown> {
  const res = await fetch(`${API_URL}/api${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${API_TOKEN}`,
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${res.status} ${res.statusText}: ${text}`);
  }
  const text = await res.text();
  return text ? JSON.parse(text) : {};
}

function toEpochSecs(iso: string): number {
  return Math.floor(new Date(iso).getTime() / 1000);
}

const server = new Server(
  { name: "todo-mcp-server", version: "0.1.0" },
  { capabilities: { tools: {} } }
);

server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [
    {
      name: "list_users",
      description: "List all users in the todo app.",
      inputSchema: { type: "object", properties: {}, required: [] },
    },
    {
      name: "get_user",
      description: "Get a user by their ID.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The user's ID" },
        },
        required: ["userId"],
      },
    },
    {
      name: "update_user",
      description: "Update a user's name.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          firstName: { type: "string" },
          lastName: { type: "string" },
        },
        required: ["userId", "firstName", "lastName"],
      },
    },
    {
      name: "list_items",
      description:
        "List a project's items. Optionally filter by parent item to get sub-tasks.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: {
            type: "string",
            description: "The project whose items to list.",
          },
          parentItemId: {
            type: "string",
            description: "If provided, returns only children of this item",
          },
        },
        required: ["projectId"],
      },
    },
    {
      name: "get_item",
      description: "Get a single todo item by ID from a project.",
      inputSchema: {
        type: "object",
        properties: {
          itemId: { type: "string" },
          projectId: {
            type: "string",
            description: "The project the item belongs to.",
          },
        },
        required: ["projectId", "itemId"],
      },
    },
    {
      name: "create_item",
      description:
        "Create a new todo item in a project. Supports due dates, recurrence rules (e.g. 'every Monday', 'every 2 weeks'), and nesting under a parent item. " +
        "Recurrence is only valid on top-level items with no parentItemId/sourceEventId — a child item or an event-linked item (sourceEventId) uses dueOffsetDays instead, and setting recurrence alongside either is rejected. " +
        "assignedToUserId/points are only meaningful on a team-backed project.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: {
            type: "string",
            description: "The project to create the item in.",
          },
          name: { type: "string", description: "Title of the todo item" },
          description: { type: "string", description: "Free-form notes, longer than name" },
          dueDate: {
            type: "string",
            description: "ISO 8601 date/time string for the due date",
          },
          scheduledDate: {
            type: "string",
            description: "ISO 8601 date/time string for when this is scheduled to start",
          },
          scheduledEndDate: {
            type: "string",
            description: "ISO 8601 date/time string for when this is scheduled to end",
          },
          complete: { type: "boolean" },
          recurrence: {
            type: "string",
            description:
              "English recurrence rule, e.g. 'every day', 'every Monday', 'every 2 weeks', 'every month on the 1st'. Only valid when parentItemId is not set.",
          },
          recurrenceBasis: {
            type: "string",
            enum: ["DUE_DATE", "COMPLETION_DATE"],
            description: "Whether recurrence advances from due date or completion date",
          },
          hasDueTime: {
            type: "boolean",
            description: "Whether the due date includes a specific time",
          },
          hasScheduledTime: {
            type: "boolean",
            description: "Whether scheduledDate includes a specific time",
          },
          hasEndTime: {
            type: "boolean",
            description: "Whether scheduledEndDate includes a specific time",
          },
          parentItemId: {
            type: "string",
            description: "ID of the parent item (for sub-tasks)",
          },
          itemType: {
            type: "string",
            enum: ["TASK", "EVENT", "SIMPLE"],
            description:
              "TASK (default) is due-date-driven; EVENT is scheduled-date-primary, for calendar-style items; " +
              "SIMPLE is a bare checkable name with no due date, scheduled window, recurrence, or due offset " +
              "(the server rejects any of those on a SIMPLE item).",
          },
          eventType: {
            type: "string",
            description:
              "Free-text category (e.g. 'rain'). Only valid on itemType EVENT — the server rejects it on any other item type. If it matches a checklist template's own eventType, that template's children are automatically added as sourceEventId-linked top-level tasks referencing this item when it's created (not nested children — an Event can never have children).",
          },
          dueOffsetDays: {
            type: "number",
            description:
              "For a child item (parentItemId) or event-linked item (sourceEventId) only: days from the top-level item's or linked event's due date (negative = before, positive = after). " +
              "The due date is always computed from this offset (a manually-set dueDate is ignored/overwritten) and is recalculated whenever the top-level item recurs or the linked event is rescheduled/recurs.",
          },
          sourceEventId: {
            type: "string",
            description:
              "ID of an EVENT-typed item this (top-level) task references and tracks — mutually exclusive with parentItemId (an item either nests under a parent or references an event, never both). Like a child item, its due date is offset-driven via dueOffsetDays rather than freely settable, and it can't have scheduledDate/scheduledEndDate.",
          },
          timezoneOffsetMinutes: {
            type: "number",
            description: "Client timezone offset in minutes (e.g. -300 for EST)",
          },
          assignedToUserId: {
            type: "string",
            description: "Active member to assign this item to. Only meaningful on a team-backed project.",
          },
          points: {
            type: "number",
            description: "Points awarded on completion. Only meaningful on a team-backed project. Project admin only (silently dropped if the caller isn't an admin).",
          },
        },
        required: ["projectId", "name"],
      },
    },
    {
      name: "import_project_items",
      description:
        "Bulk-import todo items into a project from CSV text. Parsing, validation, and creation all happen server-side, one row at a time, best-effort — invalid rows are reported individually with an error, valid rows are still created (nothing rolls back). " +
        "Each row becomes a top-level item; a parentItemId column may reference an item that already exists, but not another row in the same CSV (no intra-file parent/child resolution in this version). " +
        "Call get_item_import_template first for the expected column headers and example rows.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string", description: "The project to import items into." },
          csv: {
            type: "string",
            description: "Raw CSV text, including a header row matching get_item_import_template's output.",
          },
          format: {
            type: "string",
            description: "CSV column-mapping format. Only \"PRL\" (the default) is currently supported.",
          },
          timezoneOffsetMinutes: {
            type: "number",
            description: "Client timezone offset in minutes (JS getTimezoneOffset() convention — positive for zones behind UTC, e.g. 300 for EST), used to interpret bare dueDate/scheduledDate/scheduledEndDate dates and for offset/recurrence computation on rows that need it. Defaults to this machine's own local offset if omitted.",
          },
        },
        required: ["projectId", "csv"],
      },
    },
    {
      name: "get_item_import_template",
      description:
        "Download a starter CSV template for import_project_items — a header row with every supported column plus 3 example rows (one Task, one Event, one Simple item).",
      inputSchema: {
        type: "object",
        properties: {},
        required: [],
      },
    },
    {
      name: "update_item",
      description:
        "Update a todo item in a project. Marking a recurring item as complete will auto-create the next occurrence, carrying its child items over with deadlines recomputed from their dueOffsetDays.",
      inputSchema: {
        type: "object",
        properties: {
          itemId: { type: "string" },
          projectId: {
            type: "string",
            description: "The project the item belongs to.",
          },
          name: { type: "string" },
          description: {
            type: "string",
            description: "Free-form notes, longer than name. Omit to leave unchanged; send an empty string to clear it.",
          },
          complete: { type: "boolean" },
          dueDate: { type: "string", description: "ISO 8601 date/time string" },
          scheduledDate: { type: "string", description: "ISO 8601 date/time string" },
          scheduledEndDate: { type: "string", description: "ISO 8601 date/time string" },
          recurrence: { type: "string", description: "Only valid when parentItemId/sourceEventId are not set." },
          recurrenceBasis: {
            type: "string",
            enum: ["DUE_DATE", "COMPLETION_DATE"],
          },
          hasDueTime: { type: "boolean" },
          hasScheduledTime: { type: "boolean" },
          hasEndTime: { type: "boolean" },
          parentItemId: { type: "string" },
          itemType: {
            type: "string",
            enum: ["TASK", "EVENT", "SIMPLE"],
            description: "Omit to leave the item's current kind unchanged.",
          },
          eventType: { type: "string" },
          dueOffsetDays: {
            type: "number",
            description: "For a child item (parentItemId) or event-linked item (sourceEventId) only: days from the top-level item's or linked event's due date (negative = before, positive = after).",
          },
          sourceEventId: {
            type: "string",
            description:
              "ID of an EVENT-typed item this (top-level) task references — see create_item's sourceEventId for the full rationale. Omit to leave unchanged; the current value is not preserved automatically if omitted on a caller-built update, so round-trip it explicitly when editing an item that already has one.",
          },
          timezoneOffsetMinutes: { type: "number" },
          assignedToUserId: {
            type: "string",
            description: "Active member to assign this item to. Only meaningful on a team-backed project.",
          },
          points: {
            type: "number",
            description: "Points awarded on completion. Only meaningful on a team-backed project. Project admin only (the server preserves the existing value if the caller isn't an admin).",
          },
        },
        required: ["projectId", "itemId", "name", "complete"],
      },
    },
    {
      name: "delete_item",
      description: "Delete a todo item and all its sub-tasks from a project.",
      inputSchema: {
        type: "object",
        properties: {
          itemId: { type: "string" },
          projectId: {
            type: "string",
            description: "The project the item belongs to.",
          },
        },
        required: ["projectId", "itemId"],
      },
    },
    {
      name: "list_items_due",
      description:
        "List items due within a date range. Useful for showing today's tasks or upcoming deadlines.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          deadlineAfter: {
            type: "string",
            description: "ISO 8601 date/time — only return items due after this",
          },
          deadlineBefore: {
            type: "string",
            description: "ISO 8601 date/time — only return items due before this",
          },
        },
        required: ["userId"],
      },
    },
    {
      name: "list_assigned_items",
      description:
        "List items assigned to this user by other team members (across all owners), regardless of due date.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
        },
        required: ["userId"],
      },
    },
    {
      name: "list_projects",
      description:
        "List the projects a user is a member of. Every user has an auto-created personal 'Personal' project; a project may also be backed by a team (teamId set), granting every active team member access.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
        },
        required: ["userId"],
      },
    },
    {
      name: "create_project",
      description:
        "Create a new personal project (no attached team — use attach_team_to_project afterwards to share it via a team).",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The creating user's ID" },
          name: { type: "string" },
        },
        required: ["userId", "name"],
      },
    },
    {
      name: "get_project",
      description: "Get a project by ID. The caller must be a member (owner, or an active member of its attached team).",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          projectId: { type: "string" },
        },
        required: ["userId", "projectId"],
      },
    },
    {
      name: "update_project",
      description: "Rename a project. The caller must be a project admin.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          projectId: { type: "string" },
          name: { type: "string" },
        },
        required: ["userId", "projectId", "name"],
      },
    },
    {
      name: "delete_project",
      description: "Delete a project. The caller must be a project admin.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          projectId: { type: "string" },
        },
        required: ["userId", "projectId"],
      },
    },
    {
      name: "list_project_members",
      description: "List a project's members, their role ('admin'/'member'), and points balance. The caller must be a project member.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
        },
        required: ["projectId"],
      },
    },
    {
      name: "set_project_member_role",
      description: "Promote or demote a project member's role ('admin' or 'member'). The caller must be a project admin.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
          targetUserId: { type: "string" },
          role: { type: "string", enum: ["admin", "member"] },
        },
        required: ["projectId", "targetUserId", "role"],
      },
    },
    {
      name: "attach_team_to_project",
      description:
        "Attach a team to a project, granting its active members access. Seeds a project_members row (role 'member') for every currently-active team member. The caller must be a project admin.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
          teamId: { type: "string" },
        },
        required: ["projectId", "teamId"],
      },
    },
    {
      name: "detach_team_from_project",
      description:
        "Detach a project's team, revoking access for every member except the project owner. The caller must be a project admin.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
        },
        required: ["projectId"],
      },
    },
    {
      name: "list_item_series",
      description:
        "List a project's recurring item series (a series is a recurrence rule + anchor date + static fields, materializing either Task or Event occurrences — distinct from a plain recurring item). The caller must be a project member.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
        },
        required: ["projectId"],
      },
    },
    {
      name: "create_item_series",
      description:
        "Create a new recurring item series on a project. eventType is currently unsupported on any series (not accepted here) — the legacy template-trigger mechanism it would drive can conflict with a series' own materialization behavior. The caller must be a project member.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
          name: { type: "string" },
          description: { type: "string" },
          recurrence: { type: "string", description: "English recurrence pattern, e.g. 'every monday' — same syntax as an item's own recurrence field" },
          anchorDate: { type: "string", description: "ISO 8601 date/time string" },
          itemType: {
            type: "string",
            enum: ["TASK", "EVENT"],
            description: "The kind of item this series materializes occurrences as. No default — must be explicit.",
          },
          basis: {
            type: "string",
            enum: ["SCHEDULE", "COMPLETION"],
            description: "Defaults to SCHEDULE. COMPLETION measures the next occurrence from actual completion/skip time instead of the fixed schedule — only valid on a TASK series with an 'every N days/weeks/months/years' recurrence.",
          },
        },
        required: ["projectId", "name", "recurrence", "anchorDate", "itemType"],
      },
    },
    {
      name: "get_item_series",
      description: "Get one item series by ID. The caller must be a project member.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
          seriesId: { type: "string" },
        },
        required: ["projectId", "seriesId"],
      },
    },
    {
      name: "update_item_series",
      description:
        "Update an item series (full replace of name/recurrence/anchorDate/description/itemType/basis — round-trip description/basis to keep them, omitting clears them). eventType is currently unsupported on any series and is not accepted here. The caller must be a project member.",
      inputSchema: {
        type: "object",
        properties: {
          projectId: { type: "string" },
          seriesId: { type: "string" },
          name: { type: "string" },
          description: { type: "string" },
          recurrence: { type: "string" },
          anchorDate: { type: "string", description: "ISO 8601 date/time string" },
          itemType: {
            type: "string",
            enum: ["TASK", "EVENT"],
            description: "The kind of item this series materializes occurrences as. No default — must be explicit.",
          },
          basis: {
            type: "string",
            enum: ["SCHEDULE", "COMPLETION"],
            description: "Defaults to SCHEDULE if omitted. COMPLETION measures the next occurrence from actual completion/skip time instead of the fixed schedule — only valid on a TASK series with an 'every N days/weeks/months/years' recurrence.",
          },
        },
        required: ["projectId", "seriesId", "name", "recurrence", "anchorDate", "itemType"],
      },
    },
    {
      name: "list_teams",
      description:
        "List the teams a user belongs to, including pending invites awaiting their acceptance (status PENDING or ACTIVE).",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
        },
        required: ["userId"],
      },
    },
    {
      name: "get_team",
      description: "Get a team by ID (name only — use list_team_members for membership).",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          teamId: { type: "string" },
        },
        required: ["userId", "teamId"],
      },
    },
    {
      name: "create_team",
      description: "Create a new team. The creating user becomes its first active member.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The creating user's ID" },
          name: { type: "string", description: "Team name" },
        },
        required: ["userId", "name"],
      },
    },
    {
      name: "update_team",
      description: "Rename a team. The caller must be an active admin of the team.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The caller's user ID" },
          teamId: { type: "string" },
          name: { type: "string", description: "New team name" },
        },
        required: ["userId", "teamId", "name"],
      },
    },
    {
      name: "list_team_members",
      description:
        "List a team's members and their status (PENDING or ACTIVE). The caller must already be a member.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The caller's user ID" },
          teamId: { type: "string" },
        },
        required: ["userId", "teamId"],
      },
    },
    {
      name: "invite_team_member",
      description:
        "Invite an existing user to a team. The inviter must be an active member of the team; the invite is PENDING until accepted.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The inviting user's ID" },
          teamId: { type: "string" },
          inviteeUserId: { type: "string", description: "ID of the user being invited" },
        },
        required: ["userId", "teamId", "inviteeUserId"],
      },
    },
    {
      name: "accept_team_invite",
      description: "Accept a pending team invite, becoming an active member.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          teamId: { type: "string" },
        },
        required: ["userId", "teamId"],
      },
    },
    {
      name: "leave_team",
      description:
        "Leave a team (or decline a pending invite). If this removes the last member, the team itself is deleted.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          teamId: { type: "string" },
        },
        required: ["userId", "teamId"],
      },
    },
    {
      name: "set_team_member_role",
      description:
        "Promote or demote a team member's role ('admin' or 'member'). The caller must already be an admin of the team. Rejected if this would demote the team's last remaining admin — every team must always have at least one.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The caller's user ID (must be a team admin)" },
          teamId: { type: "string" },
          targetUserId: { type: "string", description: "The member whose role is changing" },
          role: { type: "string", enum: ["admin", "member"] },
        },
        required: ["userId", "teamId", "targetUserId", "role"],
      },
    },
    {
      name: "list_team_activity_log",
      description:
        "List a team's completion/points activity log (most recent first, capped server-side). The caller must be an active team member.",
      inputSchema: {
        type: "object",
        properties: {
          teamId: { type: "string" },
        },
        required: ["teamId"],
      },
    },
    {
      name: "undo_activity_log_entry",
      description:
        "Reverse a specific, not-yet-reversed activity log entry's points directly by id — the only way to undo a recurring item's completion, since completing one deletes the old item row entirely. Only the entry's own user (the person who earned/lost the points) may undo it.",
      inputSchema: {
        type: "object",
        properties: {
          teamId: { type: "string" },
          entryId: { type: "string" },
        },
        required: ["teamId", "entryId"],
      },
    },
    {
      name: "send_app_invite",
      description: "Send an email invite to join the todo app. The invite is sent from the authenticated user's name (as display name) via the configured SMTP account.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The inviting user's ID" },
          email: { type: "string", description: "Recipient email address" },
        },
        required: ["userId", "email"],
      },
    },
  ],
}));

server.setRequestHandler(CallToolRequestSchema, async (req) => {
  const { name, arguments: args = {} } = req.params;

  try {
    let result: unknown;

    switch (name) {
      case "list_users":
        result = await api("GET", "/users");
        break;

      case "get_user":
        result = await api("GET", `/users/${args.userId}`);
        break;

      case "update_user":
        result = await api("PUT", `/users/${args.userId}`, {
          firstName: args.firstName,
          lastName: args.lastName,
        });
        break;

      case "list_items": {
        if (!args.projectId) {
          throw new Error(
            "projectId is required — the legacy personal Item API has been retired"
          );
        }
        const qs = args.parentItemId
          ? `?parentItemId=${encodeURIComponent(args.parentItemId as string)}`
          : "";
        result = await api("GET", `/projects/${args.projectId}/items${qs}`);
        break;
      }

      case "get_item":
        if (!args.projectId) {
          throw new Error(
            "projectId is required — the legacy personal Item API has been retired"
          );
        }
        result = await api("GET", `/projects/${args.projectId}/items/${args.itemId}`);
        break;

      case "create_item": {
        if (!args.projectId) {
          throw new Error(
            "projectId is required — the legacy personal Item API has been retired"
          );
        }
        const body: Record<string, unknown> = { name: args.name };
        if (args.description) body.description = args.description;
        if (args.dueDate) body.dueDate = toEpochSecs(args.dueDate as string);
        if (args.scheduledDate) body.scheduledDate = toEpochSecs(args.scheduledDate as string);
        if (args.scheduledEndDate) body.scheduledEndDate = toEpochSecs(args.scheduledEndDate as string);
        if (args.complete !== undefined) body.complete = args.complete;
        if (args.recurrence) body.recurrence = args.recurrence;
        if (args.recurrenceBasis) body.recurrenceBasis = args.recurrenceBasis;
        if (args.hasDueTime !== undefined) body.hasDueTime = args.hasDueTime;
        if (args.hasScheduledTime !== undefined) body.hasScheduledTime = args.hasScheduledTime;
        if (args.hasEndTime !== undefined) body.hasEndTime = args.hasEndTime;
        if (args.parentItemId) body.parentItemId = args.parentItemId;
        if (args.itemType) body.itemType = args.itemType;
        if (args.eventType) body.eventType = args.eventType;
        if (args.dueOffsetDays !== undefined) body.dueOffsetDays = args.dueOffsetDays;
        if (args.sourceEventId) body.sourceEventId = args.sourceEventId;
        if (args.timezoneOffsetMinutes !== undefined)
          body.timezoneOffsetMinutes = args.timezoneOffsetMinutes;
        if (args.assignedToUserId) body.assignedToUserId = args.assignedToUserId;
        if (args.points !== undefined) body.points = args.points;
        result = await api("POST", `/projects/${args.projectId}/items`, body);
        break;
      }

      case "import_project_items": {
        if (!args.projectId) throw new Error("projectId is required");
        if (!args.csv) throw new Error("csv is required");
        const body: Record<string, unknown> = { csv: args.csv };
        if (args.format) body.format = args.format;
        // Bare dates in the CSV (e.g. dueDate=2026-09-30) have no time component, so the server
        // needs a timezone to interpret them correctly — otherwise it defaults to literal UTC,
        // which can land a date on the wrong calendar day once viewed back in local time.
        // Default to this machine's own offset (same minutes-to-add-to-local-to-reach-UTC
        // convention the web UI's X-Tz-Offset-Minutes header already uses) if the caller didn't
        // supply one.
        body.timezoneOffsetMinutes =
          args.timezoneOffsetMinutes !== undefined
            ? args.timezoneOffsetMinutes
            : new Date().getTimezoneOffset();
        result = await api("POST", `/projects/${args.projectId}/items/import`, body);
        break;
      }

      case "get_item_import_template":
        result = await api("GET", "/items/import-template");
        break;

      case "update_item": {
        if (!args.projectId) {
          throw new Error(
            "projectId is required — the legacy personal Item API has been retired"
          );
        }
        const body: Record<string, unknown> = {
          name: args.name,
          complete: args.complete,
        };
        if (args.description !== undefined) body.description = args.description;
        if (args.dueDate) body.dueDate = toEpochSecs(args.dueDate as string);
        if (args.scheduledDate) body.scheduledDate = toEpochSecs(args.scheduledDate as string);
        if (args.scheduledEndDate) body.scheduledEndDate = toEpochSecs(args.scheduledEndDate as string);
        if (args.recurrence) body.recurrence = args.recurrence;
        if (args.recurrenceBasis) body.recurrenceBasis = args.recurrenceBasis;
        if (args.hasDueTime !== undefined) body.hasDueTime = args.hasDueTime;
        if (args.hasScheduledTime !== undefined) body.hasScheduledTime = args.hasScheduledTime;
        if (args.hasEndTime !== undefined) body.hasEndTime = args.hasEndTime;
        if (args.parentItemId) body.parentItemId = args.parentItemId;
        if (args.itemType) body.itemType = args.itemType;
        if (args.eventType !== undefined) body.eventType = args.eventType;
        if (args.dueOffsetDays !== undefined) body.dueOffsetDays = args.dueOffsetDays;
        if (args.sourceEventId !== undefined) body.sourceEventId = args.sourceEventId;
        if (args.timezoneOffsetMinutes !== undefined)
          body.timezoneOffsetMinutes = args.timezoneOffsetMinutes;
        if (args.assignedToUserId) body.assignedToUserId = args.assignedToUserId;
        if (args.points !== undefined) body.points = args.points;
        result = await api("PUT", `/projects/${args.projectId}/items/${args.itemId}`, body);
        break;
      }

      case "delete_item":
        if (!args.projectId) {
          throw new Error(
            "projectId is required — the legacy personal Item API has been retired"
          );
        }
        result = await api("DELETE", `/projects/${args.projectId}/items/${args.itemId}`);
        break;

      case "list_items_due": {
        const params = new URLSearchParams();
        if (args.deadlineAfter)
          params.set("deadlineAfter", String(toEpochSecs(args.deadlineAfter as string)));
        if (args.deadlineBefore)
          params.set("deadlineBefore", String(toEpochSecs(args.deadlineBefore as string)));
        const qs = params.size ? `?${params}` : "";
        result = await api("GET", `/users/${args.userId}/due-items${qs}`);
        break;
      }

      case "list_assigned_items":
        result = await api("GET", `/users/${args.userId}/assigned-items`);
        break;

      case "list_projects":
        result = await api("GET", `/users/${args.userId}/projects`);
        break;

      case "create_project":
        result = await api("POST", `/users/${args.userId}/projects`, { name: args.name });
        break;

      case "get_project":
        result = await api("GET", `/users/${args.userId}/projects/${args.projectId}`);
        break;

      case "update_project":
        result = await api("PUT", `/users/${args.userId}/projects/${args.projectId}`, {
          name: args.name,
        });
        break;

      case "delete_project":
        result = await api("DELETE", `/users/${args.userId}/projects/${args.projectId}`);
        break;

      case "list_project_members":
        result = await api("GET", `/projects/${args.projectId}/members`);
        break;

      case "set_project_member_role":
        result = await api(
          "PUT",
          `/projects/${args.projectId}/members/${args.targetUserId}/role`,
          { role: args.role }
        );
        break;

      case "attach_team_to_project":
        result = await api(
          "PUT",
          `/projects/${args.projectId}/team/${args.teamId}`,
          {}
        );
        break;

      case "detach_team_from_project":
        result = await api("DELETE", `/projects/${args.projectId}/team`, {});
        break;

      case "list_item_series":
        result = await api("GET", `/projects/${args.projectId}/series`);
        break;

      case "create_item_series": {
        const body: Record<string, unknown> = {
          name: args.name,
          recurrence: args.recurrence,
          anchorDate: toEpochSecs(args.anchorDate as string),
          itemType: args.itemType,
        };
        if (args.description !== undefined) body.description = args.description;
        if (args.basis !== undefined) body.basis = args.basis;
        result = await api("POST", `/projects/${args.projectId}/series`, body);
        break;
      }

      case "get_item_series":
        result = await api(
          "GET",
          `/projects/${args.projectId}/series/${args.seriesId}`
        );
        break;

      case "update_item_series": {
        const body: Record<string, unknown> = {
          name: args.name,
          recurrence: args.recurrence,
          anchorDate: toEpochSecs(args.anchorDate as string),
          itemType: args.itemType,
        };
        if (args.description !== undefined) body.description = args.description;
        if (args.basis !== undefined) body.basis = args.basis;
        result = await api(
          "PUT",
          `/projects/${args.projectId}/series/${args.seriesId}`,
          body
        );
        break;
      }

      case "list_teams":
        result = await api("GET", `/users/${args.userId}/teams`);
        break;

      case "get_team":
        result = await api("GET", `/users/${args.userId}/teams/${args.teamId}`);
        break;

      case "create_team":
        result = await api("POST", `/users/${args.userId}/teams`, { name: args.name });
        break;

      case "update_team":
        result = await api("PUT", `/users/${args.userId}/teams/${args.teamId}`, {
          name: args.name,
        });
        break;

      case "list_team_members":
        result = await api("GET", `/users/${args.userId}/teams/${args.teamId}/members`);
        break;

      case "invite_team_member":
        result = await api("POST", `/users/${args.userId}/teams/${args.teamId}/invites`, {
          inviteeUserId: args.inviteeUserId,
        });
        break;

      case "accept_team_invite":
        result = await api("PUT", `/users/${args.userId}/teams/${args.teamId}/accept`, {});
        break;

      case "leave_team":
        result = await api(
          "DELETE",
          `/users/${args.userId}/teams/${args.teamId}/membership`
        );
        break;

      case "set_team_member_role":
        result = await api(
          "PUT",
          `/users/${args.userId}/teams/${args.teamId}/members/${args.targetUserId}/role`,
          { role: args.role }
        );
        break;

      case "list_team_activity_log":
        result = await api("GET", `/teams/${args.teamId}/activity-log`);
        break;

      case "undo_activity_log_entry":
        result = await api(
          "PUT",
          `/teams/${args.teamId}/activity-log/${args.entryId}/undo`,
          {}
        );
        break;

      case "send_app_invite":
        result = await api("POST", `/users/${args.userId}/app-invites`, { email: args.email });
        break;

      default:
        return {
          content: [{ type: "text", text: `Unknown tool: ${name}` }],
          isError: true,
        };
    }

    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  } catch (err) {
    return {
      content: [
        {
          type: "text",
          text: err instanceof Error ? err.message : String(err),
        },
      ],
      isError: true,
    };
  }
});

const transport = new StdioServerTransport();
await server.connect(transport);
