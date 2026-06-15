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
        "Create a new todo item. Supports due dates, recurrence rules (e.g. 'every Monday', 'every 2 weeks'), and nesting under a parent item.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          name: { type: "string", description: "Title of the todo item" },
          dueDate: {
            type: "string",
            description: "ISO 8601 date/time string for the due date",
          },
          complete: { type: "boolean" },
          recurrence: {
            type: "string",
            description:
              "English recurrence rule, e.g. 'every day', 'every Monday', 'every 2 weeks', 'every month on the 1st'",
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
          hasTasks: {
            type: "boolean",
            description: "Whether this item can have sub-tasks",
          },
          parentItemId: {
            type: "string",
            description: "ID of the parent item (for sub-tasks)",
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
        "Update a todo item. Marking a recurring item as complete will auto-create the next occurrence.",
      inputSchema: {
        type: "object",
        properties: {
          userId: { type: "string" },
          itemId: { type: "string" },
          name: { type: "string" },
          complete: { type: "boolean" },
          dueDate: { type: "string", description: "ISO 8601 date/time string" },
          recurrence: { type: "string" },
          recurrenceBasis: {
            type: "string",
            enum: ["DUE_DATE", "COMPLETION_DATE"],
          },
          hasDueTime: { type: "boolean" },
          hasTasks: { type: "boolean" },
          parentItemId: { type: "string" },
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
        if (args.dueDate) body.dueDate = toEpochSecs(args.dueDate as string);
        if (args.complete !== undefined) body.complete = args.complete;
        if (args.recurrence) body.recurrence = args.recurrence;
        if (args.recurrenceBasis) body.recurrenceBasis = args.recurrenceBasis;
        if (args.hasDueTime !== undefined) body.hasDueTime = args.hasDueTime;
        if (args.hasTasks !== undefined) body.hasTasks = args.hasTasks;
        if (args.parentItemId) body.parentItemId = args.parentItemId;
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
        if (args.dueDate) body.dueDate = toEpochSecs(args.dueDate as string);
        if (args.recurrence) body.recurrence = args.recurrence;
        if (args.recurrenceBasis) body.recurrenceBasis = args.recurrenceBasis;
        if (args.hasDueTime !== undefined) body.hasDueTime = args.hasDueTime;
        if (args.hasTasks !== undefined) body.hasTasks = args.hasTasks;
        if (args.parentItemId) body.parentItemId = args.parentItemId;
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
