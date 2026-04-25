import {
  ListeriaClient,
  CreateUserCommand,
  GetUserCommand,
  ListUsersCommand,
  UpdateUserCommand,
  CreateListCommand,
  GetListCommand,
  ListListsCommand,
  UpdateListCommand,
  DeleteListCommand,
  CreateItemCommand,
  ListItemsCommand,
  ListItemsDueCommand,
  UpdateItemCommand,
  DeleteItemCommand,
} from "@todo/client";

const client = new ListeriaClient({ endpoint: `${window.location.origin}/api` });

const app = document.getElementById("app")!;

function navigate(path: string) {
  history.pushState(null, "", path);
  route();
}

function showError(msg: string) {
  const p = document.createElement("p");
  p.className = "error";
  p.textContent = msg;
  app.appendChild(p);
  setTimeout(() => p.remove(), 4000);
}

function showSuccess(msg: string) {
  const p = document.createElement("p");
  p.className = "success";
  p.textContent = msg;
  app.appendChild(p);
  setTimeout(() => p.remove(), 2000);
}

function makeEditableText(
  displayValue: string,
  onSave: (rawValue: string) => Promise<void>,
  opts: { inputType?: string; inputValue?: string } = {}
): HTMLElement {
  const { inputType = "text", inputValue = displayValue } = opts;
  let currentDisplay = displayValue;
  let currentInput = inputValue;

  const span = document.createElement("span");
  span.textContent = currentDisplay;
  span.title = "Click to edit";

  span.addEventListener("click", (e) => {
    e.stopPropagation();
    const input = document.createElement("input");
    input.type = inputType;
    input.value = currentInput;
    input.style.cssText = "padding:0.2rem;border:1px solid #2a5a78;border-radius:3px;background:#0f1e2a;color:#a8d8f0;";
span.replaceWith(input);
    input.focus();

    const finish = async () => {
      const newVal = input.value.trim();
      if (newVal !== currentInput) {
        try {
          await onSave(newVal);
          currentInput = newVal;
          currentDisplay = newVal ? (inputType === "date"
            ? new Date(newVal + "T00:00:00").toLocaleDateString()
            : newVal) : "no due date";
        } catch (err) {
          showError(String(err));
        }
      }
      input.replaceWith(span);
      span.textContent = currentDisplay;
    };

    input.addEventListener("blur", finish);
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") input.blur();
      if (ev.key === "Escape") { input.value = currentInput; input.blur(); }
    });
  });

  return span;
}

function renderNotFound(msg: string) {
  app.innerHTML = `
    <h1>Not Found</h1>
    <p>${msg}</p>
    <p><a href="/" id="go-home">← Back to home</a></p>`;
  document.getElementById("go-home")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate("/");
  });
}

// ── Views ────────────────────────────────────────────────────────────────────

async function renderUsers() {
  app.innerHTML = `
    <h1>Users</h1>
    <form id="create-user-form">
      <input id="first-name" placeholder="First name" required />
      <input id="last-name" placeholder="Last name" required />
      <button type="submit">Create User</button>
    </form>
    <ul id="users"></ul>`;

  const ul = document.getElementById("users")!;

  async function load() {
    const res = await client.send(new ListUsersCommand({}));
    ul.innerHTML = "";
    for (const u of res.users ?? []) {
      const li = document.createElement("li");
      li.className = "row";

      const nameSpan = makeEditableText(
        `${u.firstName} ${u.lastName}`,
        async (newVal) => {
          const [first, ...rest] = newVal.split(" ");
          await client.send(new UpdateUserCommand({
            userId: u.userId!,
            firstName: first,
            lastName: rest.join(" ") || first,
          }));
        }
      );
      nameSpan.style.flex = "1";

      const goBtn = document.createElement("button");
      goBtn.textContent = "Lists →";
      goBtn.addEventListener("click", () => navigate(`/users/${u.userId}`));

      const dashBtn = document.createElement("button");
      dashBtn.textContent = "Dashboard →";
      dashBtn.addEventListener("click", () => navigate(`/users/${u.userId}/dashboard`));

      li.appendChild(nameSpan);
      li.appendChild(goBtn);
      li.appendChild(dashBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("create-user-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      await client.send(new CreateUserCommand({
        firstName: (document.getElementById("first-name") as HTMLInputElement).value.trim(),
        lastName: (document.getElementById("last-name") as HTMLInputElement).value.trim(),
      }));
      (document.getElementById("first-name") as HTMLInputElement).value = "";
      (document.getElementById("last-name") as HTMLInputElement).value = "";
      await load();
    } catch (err) {
      showError(String(err));
    }
  });

  await load();
}

async function renderLists(userId: string) {
  try {
    await client.send(new GetUserCommand({ userId }));
  } catch {
    renderNotFound(`User "${userId}" does not exist.`);
    return;
  }

  app.innerHTML = `
    <p><a href="/" id="back-users">← Users</a></p>
    <h1>Lists</h1>
    <form id="create-list-form">
      <input id="list-name" placeholder="List name" required />
      <select id="list-type">
        <option value="tasks">Task list</option>
        <option value="simple">Simple list</option>
      </select>
      <button type="submit">Create List</button>
    </form>
    <ul id="lists"></ul>`;

  document.getElementById("back-users")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate("/");
  });

  const ul = document.getElementById("lists")!;

  async function load() {
    const res = await client.send(new ListListsCommand({ userId }));
    ul.innerHTML = "";
    for (const l of res.lists ?? []) {
      const li = document.createElement("li");
      li.className = "row";

      const nameSpan = makeEditableText(l.name ?? "", async (newVal) => {
        await client.send(new UpdateListCommand({
          userId,
          listId: l.listId,
          name: newVal,
          hasTasks: l.hasTasks ?? true,
        }));
      });
      nameSpan.style.flex = "1";

      const typeSelect = document.createElement("select");
      typeSelect.className = "list-type-select";
      typeSelect.title = "List type";
      const optTasks = document.createElement("option");
      optTasks.value = "tasks"; optTasks.textContent = "Tasks";
      const optSimple = document.createElement("option");
      optSimple.value = "simple"; optSimple.textContent = "Simple";
      typeSelect.appendChild(optTasks);
      typeSelect.appendChild(optSimple);
      typeSelect.value = (l.hasTasks ?? true) ? "tasks" : "simple";
      typeSelect.addEventListener("change", async () => {
        const hasTasks = typeSelect.value === "tasks";
        try {
          await client.send(new UpdateListCommand({ userId, listId: l.listId, name: l.name, hasTasks }));
          l.hasTasks = hasTasks;
        } catch (err) {
          showError(String(err));
          typeSelect.value = (l.hasTasks ?? true) ? "tasks" : "simple";
        }
      });

      const goBtn = document.createElement("button");
      goBtn.textContent = "Items →";
      goBtn.addEventListener("click", () => navigate(`/users/${userId}/lists/${l.listId}`));

      const deleteBtn = document.createElement("button");
      deleteBtn.textContent = "✕";
      deleteBtn.title = "Delete list";
      deleteBtn.style.color = "#c00";
      deleteBtn.addEventListener("click", async () => {
        if (!confirm(`Delete list "${l.name}" and all its items?`)) return;
        try {
          await client.send(new DeleteListCommand({ userId, listId: l.listId }));
          li.remove();
        } catch (err) {
          showError(String(err));
        }
      });

      li.appendChild(nameSpan);
      li.appendChild(typeSelect);
      li.appendChild(goBtn);
      li.appendChild(deleteBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("create-list-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      const hasTasks = (document.getElementById("list-type") as HTMLSelectElement).value === "tasks";
      await client.send(new CreateListCommand({
        userId,
        name: (document.getElementById("list-name") as HTMLInputElement).value.trim(),
        hasTasks,
      }));
      (document.getElementById("list-name") as HTMLInputElement).value = "";
      await load();
    } catch (err) {
      showError(String(err));
    }
  });

  await load();
}

function parseDateTimeInput(dateVal: string, timeVal = ""): { date: Date | undefined; hasDueTime: boolean } {
  if (!dateVal) return { date: undefined, hasDueTime: false };
  const [year, month, day] = dateVal.split("-").map(Number);
  if (timeVal) {
    const [h, m] = timeVal.split(":").map(Number);
    return { date: new Date(year, month - 1, day, h, m), hasDueTime: true };
  }
  return { date: new Date(year, month - 1, day, 23, 59, 59), hasDueTime: false };
}

async function renderItems(userId: string, listId: string) {
  let hasTasks = true;
  try {
    const listInfo = await client.send(new GetListCommand({ userId, listId }));
    hasTasks = listInfo.hasTasks ?? true;
  } catch {
    renderNotFound(`List "${listId}" does not exist.`);
    return;
  }

  app.innerHTML = `
    <p><a href="/users/${userId}" id="back-lists">← Lists</a></p>
    <h1>Items</h1>
    <button type="button" id="new-item-btn">+ New Item</button>
    <form id="create-item-form" style="display:none;">
      <input id="item-name" placeholder="Item name" required style="flex:1;" />
      <button type="button" id="batch-toggle" title="Switch to batch input mode">Batch</button>
      <div id="task-fields">
        <div style="display:flex;gap:0.4rem;flex-wrap:wrap;">
          <input id="item-due" type="date" title="Due date (optional)" />
          <input id="item-time" type="time" title="Due time (optional)" />
        </div>
        <div style="display:flex;align-items:center;gap:0.4rem;flex-wrap:wrap;">
          <input id="item-recurrence" placeholder='Recurrence, e.g. "every 3 days"' style="flex:1;" />
          <select id="item-recurrence-basis" title="Basis for scheduling the next occurrence">
            <option value="DUE_DATE">Due date basis</option>
            <option value="COMPLETION_DATE">Completion date basis</option>
          </select>
          <span id="recurrence-info" title="Click for help" style="cursor:pointer;color:#5aace0;font-size:1.1rem;user-select:none;">ⓘ</span>
        </div>
      </div>
      <button type="submit">Add Item</button>
    </form>
    <label style="display:flex;align-items:center;gap:0.4rem;margin-bottom:0.5rem;cursor:pointer;">
      <input type="checkbox" id="show-complete" /> Show completed
    </label>
    <ul id="items"></ul>`;

  if (!hasTasks) {
    (document.getElementById("task-fields") as HTMLElement).style.display = "none";
  }

  const newItemBtn = document.getElementById("new-item-btn")!;
  const createForm = document.getElementById("create-item-form") as HTMLFormElement;
  newItemBtn.addEventListener("click", () => {
    const isOpen = createForm.style.display !== "none";
    createForm.style.display = isOpen ? "none" : "";
    newItemBtn.textContent = isOpen ? "+ New Item" : "✕ Cancel";
    if (!isOpen) {
      (document.getElementById("item-name") as HTMLInputElement).focus();
    }
  });

  document.getElementById("show-complete")!.addEventListener("change", load);

  let batchMode = false;
  document.getElementById("batch-toggle")!.addEventListener("click", () => {
    batchMode = !batchMode;
    const current = document.getElementById("item-name") as HTMLInputElement | HTMLTextAreaElement;
    const val = current.value;
    const next = batchMode ? document.createElement("textarea") : document.createElement("input");
    next.id = "item-name";
    next.required = true;
    next.style.flex = "1";
    if (!batchMode) (next as HTMLInputElement).type = "text";
    if (batchMode) (next as HTMLTextAreaElement).rows = 3;
    next.placeholder = batchMode ? "Item name (one per line)" : "Item name";
    next.value = val;
    current.replaceWith(next);
    const btn = document.getElementById("batch-toggle")!;
    btn.textContent = batchMode ? "Single" : "Batch";
    btn.style.background = batchMode ? "#1a3a52" : "";
    btn.style.borderColor = batchMode ? "#5aace0" : "";
    next.focus();
  });

  document.getElementById("recurrence-info")?.addEventListener("click", () => {
    alert(
      "Recurrence schedules the next task automatically when you mark this one complete.\n\n" +
      "Supported phrases:\n" +
      "  every day / every N days\n" +
      "  every week / every N weeks\n" +
      "  every month / every N months\n" +
      "  every year / every N years\n" +
      "  every month on the Nth  (e.g. \"every month on the 15th\")\n" +
      "  every [weekday]  (e.g. \"every Monday\")\n\n" +
      "Due date basis: the next task is scheduled relative to the original\n" +
      "due date, keeping a fixed calendar rhythm even if you complete early or late.\n\n" +
      "Completion date basis: the next task is scheduled relative to when\n" +
      "you actually complete this one."
    );
  });

  document.getElementById("back-lists")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate(`/users/${userId}`);
  });

  const ul = document.getElementById("items")!;

  async function load() {
    const res = await client.send(new ListItemsCommand({ userId, listId }));
    const showComplete = (document.getElementById("show-complete") as HTMLInputElement).checked;
    ul.innerHTML = "";
    for (const item of (res.items ?? []).filter(i => showComplete || !i.complete)) {
      const li = document.createElement("li");
      li.className = "row";

      // Name span (click-to-edit)
      const nameSpan = makeEditableText(item.name ?? "", async (newVal) => {
        await client.send(new UpdateItemCommand({
          userId, listId, itemId: item.itemId!,
          name: newVal, dueDate: item.dueDate, complete: item.complete ?? false,
        }));
        item.name = newVal;
      });
      nameSpan.style.flex = "1";

      // Delete button
      const deleteBtn = document.createElement("button");
      deleteBtn.textContent = "✕";
      deleteBtn.title = "Delete item";
      deleteBtn.style.color = "#c00";
      deleteBtn.addEventListener("click", async () => {
        try {
          await client.send(new DeleteItemCommand({
            userId, listId, itemId: item.itemId!,
          }));
          li.remove();
        } catch (err) {
          showError(String(err));
        }
      });

      // Complete toggle button (always present)
      const completeBtn = document.createElement("button");
      completeBtn.textContent = item.complete ? "☑" : "☐";
      completeBtn.title = item.complete ? "Mark incomplete" : "Mark complete";
      completeBtn.style.color = item.complete ? "#2a9d2a" : "#a8d8f0";
      completeBtn.addEventListener("click", async () => {
        const markingComplete = !item.complete;
        await client.send(new UpdateItemCommand({
          userId, listId, itemId: item.itemId!,
          name: item.name!, dueDate: item.dueDate, complete: !item.complete,
          hasDueTime: item.hasDueTime ?? false,
          recurrence: item.recurrence ?? undefined,
          recurrenceBasis: item.recurrenceBasis ?? undefined,
        }));
        if (markingComplete) {
          showSuccess(item.recurrence ? "✓ Completed — next occurrence scheduled." : "✓ Done!");
        }
        await load();
      });
      if (item.complete) nameSpan.style.textDecoration = "line-through";

      li.appendChild(completeBtn);
      li.appendChild(nameSpan);

      if (hasTasks) {

        // Due date span (click-to-edit)
        const isoDate = item.dueDate ? item.dueDate.toISOString().slice(0, 10) : "";
        const dateSpan = makeEditableText(
          item.dueDate ? item.dueDate.toLocaleDateString() : "no due date",
          async (newVal) => {
            const { date: d, hasDueTime: hdt } = parseDateTimeInput(newVal);
            await client.send(new UpdateItemCommand({
              userId, listId, itemId: item.itemId!,
              name: item.name!, dueDate: d, complete: item.complete ?? false,
              hasDueTime: newVal ? (item.hasDueTime ?? false) : false,
            }));
            item.dueDate = d;
            if (!newVal) item.hasDueTime = false;
          },
          { inputType: "date", inputValue: isoDate }
        );
        dateSpan.style.color = "#666";
        dateSpan.style.fontSize = "0.85rem";

        // Due time span (click-to-edit)
        const isoTime = (item.hasDueTime && item.dueDate)
          ? item.dueDate.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false }).slice(0, 5)
          : "";
        const timeDisplayVal = (item.hasDueTime && item.dueDate)
          ? item.dueDate.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
          : "no time";
        const timeSpan = makeEditableText(
          timeDisplayVal,
          async (newVal) => {
            const dateStr = item.dueDate ? item.dueDate.toISOString().slice(0, 10) : "";
            const { date: d, hasDueTime: hdt } = parseDateTimeInput(dateStr, newVal);
            await client.send(new UpdateItemCommand({
              userId, listId, itemId: item.itemId!,
              name: item.name!, dueDate: d ?? item.dueDate, complete: item.complete ?? false,
              hasDueTime: hdt,
            }));
            item.dueDate = d ?? item.dueDate;
            item.hasDueTime = hdt;
          },
          { inputType: "time", inputValue: isoTime }
        );
        timeSpan.style.color = "#666";
        timeSpan.style.fontSize = "0.85rem";
        timeSpan.title = "Click to set due time";

        // Recurrence tag (click-to-edit)
        const basisLabel = item.recurrenceBasis === "COMPLETION_DATE" ? "completion" : "due date";
        const recurrenceSpan = makeEditableText(
          item.recurrence ? `↻ ${item.recurrence} (${basisLabel})` : "↻ add recurrence",
          async (newVal) => {
            await client.send(new UpdateItemCommand({
              userId, listId, itemId: item.itemId!,
              name: item.name!, dueDate: item.dueDate, complete: item.complete ?? false,
              recurrence: newVal || undefined,
              recurrenceBasis: item.recurrenceBasis ?? "DUE_DATE",
            }));
            item.recurrence = newVal || undefined;
          },
          { inputValue: item.recurrence ?? "" }
        );
        recurrenceSpan.className = "recurrence-tag";
        recurrenceSpan.title = "Click to edit recurrence";

        li.appendChild(dateSpan);
        li.appendChild(timeSpan);
        li.appendChild(recurrenceSpan);
      }

      li.appendChild(deleteBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("create-item-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      const raw = (document.getElementById("item-name") as HTMLInputElement).value;
      const names = raw.split("\n").map(s => s.trim()).filter(Boolean);
      const { date: dueDate, hasDueTime } = hasTasks
        ? parseDateTimeInput(
            (document.getElementById("item-due") as HTMLInputElement).value,
            (document.getElementById("item-time") as HTMLInputElement).value,
          )
        : { date: undefined, hasDueTime: false };
      const recurrence = hasTasks ? ((document.getElementById("item-recurrence") as HTMLInputElement).value.trim() || undefined) : undefined;
      const recurrenceBasis = hasTasks ? ((document.getElementById("item-recurrence-basis") as HTMLSelectElement).value as "DUE_DATE" | "COMPLETION_DATE") : undefined;
      for (const name of names) {
        await client.send(new CreateItemCommand({ userId, listId, name, dueDate, hasDueTime, recurrence, recurrenceBasis: recurrence ? recurrenceBasis : undefined }));
      }
      if (batchMode) {
        // Reset back to single input after batch submit
        batchMode = false;
        const ta = document.getElementById("item-name") as HTMLTextAreaElement;
        const input = document.createElement("input");
        input.id = "item-name"; input.type = "text";
        input.placeholder = "Item name"; input.required = true; input.style.flex = "1";
        ta.replaceWith(input);
        const btn = document.getElementById("batch-toggle")!;
        btn.textContent = "Batch"; btn.style.background = ""; btn.style.borderColor = "";
      } else {
        (document.getElementById("item-name") as HTMLInputElement).value = "";
      }
      if (hasTasks) {
        (document.getElementById("item-due") as HTMLInputElement).value = "";
        (document.getElementById("item-time") as HTMLInputElement).value = "";
        (document.getElementById("item-recurrence") as HTMLInputElement).value = "";
      }
      await load();
      createForm.style.display = "none";
      newItemBtn.textContent = "+ New Item";
    } catch (err) {
      showError(String(err));
    }
  });

  await load();
}

function presetRange(preset: string): { after?: Date; before?: Date } {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  switch (preset) {
    case "Today":        return { after: todayStart, before: new Date(now.getFullYear(), now.getMonth(), now.getDate(), 23, 59, 59) };
    case "This Week":    return { after: todayStart, before: new Date(+todayStart + 7 * 86400e3) };
    case "Next 30 Days": return { after: todayStart, before: new Date(+todayStart + 30 * 86400e3) };
    case "Overdue":      return { before: now };
    default:             return {};
  }
}

async function renderDashboard(userId: string) {
  let userName = userId;
  try {
    const user = await client.send(new GetUserCommand({ userId }));
    userName = `${user.firstName} ${user.lastName}`;
  } catch {
    renderNotFound(`User "${userId}" does not exist.`);
    return;
  }

  app.innerHTML = `
    <p><a href="/" id="back-users">← Users</a></p>
    <h1>${userName}'s Dashboard</h1>
    <div style="display:flex;align-items:center;gap:0.8rem;margin-bottom:0.5rem;flex-wrap:wrap;">
      <label>View:
        <select id="dash-preset">
          <option value="All">All</option>
          <option value="All with due date">All with due date</option>
          <option value="Today" selected>Today</option>
          <option value="This Week">This Week</option>
          <option value="Next 30 Days">Next 30 Days</option>
          <option value="Overdue">Overdue</option>
        </select>
      </label>
      <label style="display:flex;align-items:center;gap:0.3rem;cursor:pointer;">
        <input type="checkbox" id="dash-show-complete" /> Show completed
      </label>
    </div>
    <ul id="due-items"></ul>`;

  document.getElementById("back-users")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate("/");
  });

  const ul = document.getElementById("due-items")!;

  async function load() {
    const preset = (document.getElementById("dash-preset") as HTMLSelectElement).value;
    const showComplete = (document.getElementById("dash-show-complete") as HTMLInputElement).checked;
    const { after, before } = presetRange(preset);

    const res = await client.send(new ListItemsDueCommand({
      userId,
      deadlineAfter: after,
      deadlineBefore: before,
    }));

    ul.innerHTML = "";
    const items = (res.items ?? [])
      .filter(i => showComplete || !i.complete)
      .filter(i => preset !== "All with due date" || i.dueDate != null);
    if (items.length === 0) {
      ul.innerHTML = `<li style="color:#666;">No items.</li>`;
      return;
    }

    for (const item of items) {
      const li = document.createElement("li");
      li.className = "row";

      const completeBtn = document.createElement("button");
      completeBtn.textContent = item.complete ? "☑" : "☐";
      completeBtn.title = item.complete ? "Mark incomplete" : "Mark complete";
      completeBtn.style.color = item.complete ? "#2a9d2a" : "#a8d8f0";
      completeBtn.addEventListener("click", async () => {
        const markingComplete = !item.complete;
        try {
          await client.send(new UpdateItemCommand({
            userId, listId: item.listId!, itemId: item.itemId!,
            name: item.name!, dueDate: item.dueDate, complete: !item.complete,
            hasDueTime: item.hasDueTime ?? false,
            recurrence: item.recurrence ?? undefined,
            recurrenceBasis: item.recurrenceBasis ?? undefined,
          }));
          if (markingComplete) {
            showSuccess(item.recurrence ? "✓ Completed — next occurrence scheduled." : "✓ Done!");
          }
          await load();
        } catch (err) {
          showError(String(err));
        }
      });

      const nameSpan = document.createElement("span");
      nameSpan.textContent = item.name ?? "";
      nameSpan.style.flex = "1";
      if (item.complete) nameSpan.style.textDecoration = "line-through";

      const dateSpan = document.createElement("span");
      dateSpan.style.color = "#666";
      dateSpan.style.fontSize = "0.85rem";
      if (item.dueDate) {
        dateSpan.textContent = item.hasDueTime
          ? item.dueDate.toLocaleString([], { dateStyle: "short", timeStyle: "short" })
          : item.dueDate.toLocaleDateString();
      }

      const listBadge = document.createElement("span");
      listBadge.textContent = `[${item.listName}]`;
      listBadge.style.cssText = "font-size:0.8rem;color:#5aace0;";
      listBadge.title = "List";

      const deleteBtn = document.createElement("button");
      deleteBtn.textContent = "✕";
      deleteBtn.title = "Delete item";
      deleteBtn.style.color = "#c00";
      deleteBtn.addEventListener("click", async () => {
        try {
          await client.send(new DeleteItemCommand({ userId, listId: item.listId!, itemId: item.itemId! }));
          await load();
        } catch (err) {
          showError(String(err));
        }
      });

      li.appendChild(completeBtn);
      li.appendChild(nameSpan);
      if (item.dueDate) li.appendChild(dateSpan);
      li.appendChild(listBadge);
      li.appendChild(deleteBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("dash-preset")!.addEventListener("change", load);
  document.getElementById("dash-show-complete")!.addEventListener("change", load);

  await load();
}

// ── Router ───────────────────────────────────────────────────────────────────

async function route() {
  const path = window.location.pathname;

  const dashMatch = path.match(/^\/users\/([^/]+)\/dashboard$/);
  if (dashMatch) {
    await renderDashboard(dashMatch[1]);
    return;
  }

  const itemsMatch = path.match(/^\/users\/([^/]+)\/lists\/([^/]+)$/);
  if (itemsMatch) {
    await renderItems(itemsMatch[1], itemsMatch[2]);
    return;
  }

  const listsMatch = path.match(/^\/users\/([^/]+)$/);
  if (listsMatch) {
    await renderLists(listsMatch[1]);
    return;
  }

  await renderUsers();
}

window.addEventListener("popstate", route);
route();
