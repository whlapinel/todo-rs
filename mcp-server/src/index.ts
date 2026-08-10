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
        "List todo items for a user. Optionally filter by parent item to get sub-tasks.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string", description: "The user's ID" },
          parentItemId: {
            type: "string",
            description: "If provided, returns only children of this item",
          },
        },
        required: ["userId"],
      },
    },
    {
      name: "get_item",
      description: "Get a single todo item by ID.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          itemId: { type: "string" },
        },
        required: ["userId", "itemId"],
      },
    },
    {
      name: "create_item",
      description:
        "Create a new todo item. Supports due dates, recurrence rules (e.g. 'every Monday', 'every 2 weeks'), and nesting under a parent item. " +
        "Recurrence is only valid on top-level items with no parentItemId/sourceEventId — a child item or an event-linked item (sourceEventId) uses dueOffsetDays instead, and setting recurrence alongside either is rejected.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
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
        },
        required: ["userId", "name"],
      },
    },
    {
      name: "update_item",
      description:
        "Update a todo item. Marking a recurring item as complete will auto-create the next occurrence, carrying its child items over with deadlines recomputed from their dueOffsetDays.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          itemId: { type: "string" },
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
        },
        required: ["userId", "itemId", "name", "complete"],
      },
    },
    {
      name: "delete_item",
      description: "Delete a todo item and all its sub-tasks.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          itemId: { type: "string" },
        },
        required: ["userId", "itemId"],
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
      name: "list_team_items",
      description: "List items belonging to a team. The caller must be an active team member.",
      inputSchema: {
        type: "object",
        properties: {
          teamId: { type: "string" },
          parentItemId: { type: "string", description: "Filter to children of this item" },
        },
        required: ["teamId"],
      },
    },
    {
      name: "get_team_item",
      description: "Get a team item by ID. The caller must be an active team member.",
      inputSchema: {
        type: "object",
        properties: {
          teamId: { type: "string" },
          itemId: { type: "string" },
        },
        required: ["teamId", "itemId"],
      },
    },
    {
      name: "create_team_item",
      description: "Create a new item owned by a team. The caller must be an active team member. Supports assignment to any active team member. " +
        "Recurrence is only valid on top-level items with no parentItemId/sourceEventId — a child item or an event-linked item (sourceEventId) uses dueOffsetDays instead, and setting recurrence alongside either is rejected.",
      inputSchema: {
        type: "object",
        properties: {
          teamId: { type: "string" },
          name: { type: "string" },
          description: { type: "string", description: "Free-form notes, longer than name" },
          dueDate: { type: "string", description: "ISO 8601 date/time string" },
          scheduledDate: { type: "string", description: "ISO 8601 date/time string" },
          scheduledEndDate: { type: "string", description: "ISO 8601 date/time string" },
          complete: { type: "boolean" },
          recurrence: { type: "string", description: "Only valid when parentItemId is not set." },
          recurrenceBasis: { type: "string", enum: ["DUE_DATE", "COMPLETION_DATE"] },
          hasDueTime: { type: "boolean" },
          hasScheduledTime: { type: "boolean" },
          hasEndTime: { type: "boolean" },
          parentItemId: { type: "string" },
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
              "ID of an EVENT-typed team item this (top-level) task references and tracks — mutually exclusive with parentItemId. Unlike parentItemId-nested children, a sourceEventId-linked task is top-level, so it can still be assigned and carry points.",
          },
          assignedToUserId: { type: "string", description: "Active team member to assign this item to" },
          timezoneOffsetMinutes: { type: "number" },
          points: {
            type: "number",
            description:
              "Top-level items only (rejected alongside a parentItemId). Admin-only: the server silently drops this if the caller isn't an admin of the team, rather than rejecting the rest of the request.",
          },
        },
        required: ["teamId", "name"],
      },
    },
    {
      name: "update_team_item",
      description: "Update a team item. The caller must be an active team member. Marking a recurring item complete will auto-create the next occurrence, carrying its child items over with deadlines recomputed from their dueOffsetDays.",
      inputSchema: {
        type: "object",
        properties: {
          teamId: { type: "string" },
          itemId: { type: "string" },
          name: { type: "string" },
          description: {
            type: "string",
            description: "Free-form notes, longer than name. Omit to leave unchanged; send an empty string to clear it.",
          },
          complete: { type: "boolean" },
          dueDate: { type: "string", description: "ISO 8601 date/time string" },
          scheduledDate: { type: "string", description: "ISO 8601 date/time string" },
          scheduledEndDate: { type: "string", description: "ISO 8601 date/time string" },
          recurrence: { type: "string", description: "Only valid when parentItemId is not set." },
          recurrenceBasis: { type: "string", enum: ["DUE_DATE", "COMPLETION_DATE"] },
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
              "ID of an EVENT-typed team item this (top-level) task references — see create_team_item's sourceEventId for the full rationale. Omit to leave unchanged; the current value is not preserved automatically if omitted on a caller-built update, so round-trip it explicitly when editing an item that already has one.",
          },
          assignedToUserId: { type: "string", description: "Active team member to assign this item to" },
          timezoneOffsetMinutes: { type: "number" },
          points: {
            type: "number",
            description:
              "Top-level items only. Admin-only: the server preserves the item's existing value if the caller isn't an admin of the team, rather than rejecting the rest of the request. Omit to leave the current value unchanged only if you're not an admin — an admin omitting this will clear it, so admins should always round-trip the item's current points if they don't intend to change it.",
          },
        },
        required: ["teamId", "itemId", "name", "complete"],
      },
    },
    {
      name: "delete_team_item",
      description: "Delete a team item and all its sub-items. The caller must be an active team member.",
      inputSchema: {
        type: "object",
        properties: {
          teamId: { type: "string" },
          itemId: { type: "string" },
        },
        required: ["teamId", "itemId"],
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
        const qs = args.parentItemId
          ? `?parentItemId=${encodeURIComponent(args.parentItemId as string)}`
          : "";
        result = await api("GET", `/users/${args.userId}/items${qs}`);
        break;
      }

      case "get_item":
        result = await api("GET", `/users/${args.userId}/items/${args.itemId}`);
        break;

      case "create_item": {
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
        result = await api("POST", `/users/${args.userId}/items`, body);
        break;
      }

      case "update_item": {
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
        result = await api(
          "PUT",
          `/users/${args.userId}/items/${args.itemId}`,
          body
        );
        break;
      }

      case "delete_item":
        result = await api(
          "DELETE",
          `/users/${args.userId}/items/${args.itemId}`
        );
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

      case "list_team_items": {
        const params = new URLSearchParams();
        if (args.parentItemId) params.set("parentItemId", args.parentItemId as string);
        const qs = params.size ? `?${params}` : "";
        result = await api("GET", `/teams/${args.teamId}/items${qs}`);
        break;
      }

      case "get_team_item":
        result = await api("GET", `/teams/${args.teamId}/items/${args.itemId}`);
        break;

      case "create_team_item": {
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
        if (args.assignedToUserId) body.assignedToUserId = args.assignedToUserId;
        if (args.timezoneOffsetMinutes !== undefined)
          body.timezoneOffsetMinutes = args.timezoneOffsetMinutes;
        if (args.points !== undefined) body.points = args.points;
        result = await api("POST", `/teams/${args.teamId}/items`, body);
        break;
      }

      case "update_team_item": {
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
        if (args.assignedToUserId) body.assignedToUserId = args.assignedToUserId;
        if (args.timezoneOffsetMinutes !== undefined)
          body.timezoneOffsetMinutes = args.timezoneOffsetMinutes;
        if (args.points !== undefined) body.points = args.points;
        result = await api("PUT", `/teams/${args.teamId}/items/${args.itemId}`, body);
        break;
      }

      case "delete_team_item":
        result = await api("DELETE", `/teams/${args.teamId}/items/${args.itemId}`);
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
