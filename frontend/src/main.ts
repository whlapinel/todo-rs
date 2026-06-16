import {
  PeoplesRepublicOfListsClient,
  GetUserCommand,
  ListUsersCommand,
  UpdateUserCommand,
  CreateItemCommand,
  ListItemsCommand,
  ListItemsDueCommand,
  GetItemCommand,
  UpdateItemCommand,
  DeleteItemCommand,
  CreateTemplateCommand,
  ListTemplatesCommand,
  type ItemSummary,
  type DueItemSummary,
  type GetItemCommandOutput,
} from "@todo/client";

const client = new PeoplesRepublicOfListsClient({ endpoint: `${window.location.origin}/api` });

const app = document.getElementById("app")!;

interface AuthMe {
  userId: string;
  firstName: string;
  lastName: string;
}

async function checkAuth(): Promise<AuthMe | null> {
  try {
    const res = await fetch("/auth/me");
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

function renderLogin() {
  app.innerHTML = `
    <div style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:60vh;gap:1.5rem;">
      <h1>Todo</h1>
      <p style="color:#666;">Sign in to continue</p>
      <a href="/auth/google" style="
        display:inline-block;
        padding:0.75rem 1.5rem;
        background:#fff;
        color:#333;
        border:1px solid #ccc;
        border-radius:4px;
        text-decoration:none;
        font-weight:500;
        font-size:1rem;
      ">Sign in with Google</a>
    </div>`;
}

function addLogoutButton(userName: string) {
  const header = document.createElement("div");
  header.style.cssText = "display:flex;justify-content:flex-end;align-items:center;gap:1rem;padding:0.5rem 0;margin-bottom:0.5rem;border-bottom:1px solid #1a3a52;";
  header.innerHTML = `<span style="color:#666;font-size:0.85rem;">${userName}</span>`;
  const logoutBtn = document.createElement("a");
  logoutBtn.href = "/auth/logout";
  logoutBtn.textContent = "Sign out";
  logoutBtn.style.cssText = "color:#5aace0;font-size:0.85rem;text-decoration:none;";
  header.appendChild(logoutBtn);
  app.prepend(header);
}

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

async function renderUsers(currentUserId: string, currentUserName: string) {
  app.innerHTML = `
    <h1>Users</h1>
    <ul id="users"></ul>`;

  addLogoutButton(currentUserName);

  const ul = document.getElementById("users")!;

  const res = await client.send(new ListUsersCommand({}));
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
    goBtn.textContent = "Items →";
    goBtn.addEventListener("click", () => navigate(`/users/${u.userId}`));

    const dashBtn = document.createElement("button");
    dashBtn.textContent = "Dashboard →";
    dashBtn.addEventListener("click", () => navigate(`/users/${u.userId}/dashboard`));

    li.appendChild(nameSpan);
    li.appendChild(goBtn);
    li.appendChild(dashBtn);
    ul.appendChild(li);
  }

  // Auto-navigate to the authenticated user's items
  navigate(`/users/${currentUserId}`);
}

function localDateStr(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
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

// parentItemId: when set, we're viewing children of that item
// parentItemName: display name for the heading
async function renderItems(userId: string, parentItemId?: string, parentItemName?: string) {
  // If drilling into a parent item, fetch its full details (for the Edit form and to control UI)
  let hasTasks = true;
  let parentItem: GetItemCommandOutput | undefined;
  if (parentItemId) {
    try {
      parentItem = await client.send(new GetItemCommand({ userId, itemId: parentItemId }));
      hasTasks = parentItem.hasTasks ?? true;
    } catch {
      renderNotFound(`Item "${parentItemId}" does not exist.`);
      return;
    }
  }

  const heading = parentItemName ?? "Items";
  const backHref = parentItemId ? `/users/${userId}` : `/users/${userId}/dashboard`;
  const backLabel = parentItemId ? "← Items" : "← Dashboard";

  app.innerHTML = `
    <p><a href="${backHref}" id="back-link">${backLabel}</a></p>
    <div style="display:flex;align-items:center;gap:0.6rem;">
      <h1 style="margin:0;">${heading}</h1>
      ${parentItemId ? '<button type="button" id="edit-parent-btn">Edit</button>' : ""}
    </div>
    ${parentItemId ? `
    <form id="edit-parent-form" style="display:none;margin:0.5rem 0 1rem;">
      <div class="field-grid">
        <span class="field-label">Name</span>
        <input id="edit-parent-name" placeholder="Item name" required />
        <div id="edit-parent-task-fields" style="display:contents;">
          <span class="field-label">Due</span>
          <div class="field-row">
            <input id="edit-parent-due" type="date" title="Due date (optional)" />
            <input id="edit-parent-time" type="time" title="Due time (optional)" />
          </div>
          <span class="field-label">Repeat</span>
          <div class="field-row">
            <input id="edit-parent-recurrence" placeholder='e.g. "every 3 days"' />
            <select id="edit-parent-recurrence-basis" title="Basis for scheduling the next occurrence">
              <option value="DUE_DATE">Due date</option>
              <option value="COMPLETION_DATE">Completion date</option>
            </select>
          </div>
        </div>
      </div>
      <button type="submit">Save</button>
      <button type="button" id="cancel-edit-parent-btn">Cancel</button>
    </form>` : ""}
    <button type="button" id="new-item-btn">+ New Item</button>
    <button type="button" id="checklists-btn">Checklists</button>
    <form id="create-item-form" style="display:none;">
      <div class="field-grid">
        <span class="field-label">Name</span>
        <div class="field-row">
          <input id="item-name" placeholder="Item name" required />
          <button type="button" id="batch-toggle" title="Switch to batch input mode">Batch</button>
        </div>
        <div id="task-fields" style="display:contents;">
          <span class="field-label">Due</span>
          <div class="field-row">
            <input id="item-due" type="date" title="Due date (optional)" />
            <input id="item-time" type="time" title="Due time (optional)" />
          </div>
          <span class="field-label">Repeat</span>
          <div class="field-row">
            <input id="item-recurrence" placeholder='e.g. "every 3 days"' />
            <select id="item-recurrence-basis" title="Basis for scheduling the next occurrence">
              <option value="DUE_DATE">Due date</option>
              <option value="COMPLETION_DATE">Completion date</option>
            </select>
            <span id="recurrence-info" title="Click for help" style="cursor:pointer;color:#5aace0;font-size:1.1rem;user-select:none;">ⓘ</span>
          </div>
        </div>
        <span class="field-label">Type</span>
        <div class="field-row">
          <select id="item-has-tasks" title="Item type">
            <option value="tasks">Task list</option>
            <option value="simple">Simple list</option>
          </select>
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
    document.getElementById("edit-parent-task-fields")?.style.setProperty("display", "none");
  }

  document.getElementById("back-link")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate(backHref);
  });

  document.getElementById("checklists-btn")!.addEventListener("click", () => {
    navigate(`/users/${userId}/checklists`);
  });

  if (parentItemId && parentItem) {
    const editBtn = document.getElementById("edit-parent-btn")!;
    const editForm = document.getElementById("edit-parent-form") as HTMLFormElement;

    editBtn.addEventListener("click", () => {
      (document.getElementById("edit-parent-name") as HTMLInputElement).value = parentItem!.name ?? "";
      (document.getElementById("edit-parent-due") as HTMLInputElement).value =
        parentItem!.dueDate ? localDateStr(parentItem!.dueDate) : "";
      (document.getElementById("edit-parent-time") as HTMLInputElement).value =
        (parentItem!.hasDueTime && parentItem!.dueDate)
          ? parentItem!.dueDate.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false }).slice(0, 5)
          : "";
      (document.getElementById("edit-parent-recurrence") as HTMLInputElement).value = parentItem!.recurrence ?? "";
      (document.getElementById("edit-parent-recurrence-basis") as HTMLSelectElement).value =
        parentItem!.recurrenceBasis ?? "DUE_DATE";
      editForm.style.display = "";
      editBtn.style.display = "none";
    });

    document.getElementById("cancel-edit-parent-btn")!.addEventListener("click", () => {
      editForm.style.display = "none";
      editBtn.style.display = "";
    });

    editForm.addEventListener("submit", async (e) => {
      e.preventDefault();
      const name = (document.getElementById("edit-parent-name") as HTMLInputElement).value.trim();
      if (!name) return;
      const { date: dueDate, hasDueTime } = hasTasks
        ? parseDateTimeInput(
            (document.getElementById("edit-parent-due") as HTMLInputElement).value,
            (document.getElementById("edit-parent-time") as HTMLInputElement).value,
          )
        : { date: parentItem!.dueDate, hasDueTime: parentItem!.hasDueTime ?? false };
      const recurrence = hasTasks
        ? ((document.getElementById("edit-parent-recurrence") as HTMLInputElement).value.trim() || undefined)
        : parentItem!.recurrence;
      const recurrenceBasis = hasTasks
        ? ((document.getElementById("edit-parent-recurrence-basis") as HTMLSelectElement).value as "DUE_DATE" | "COMPLETION_DATE")
        : parentItem!.recurrenceBasis;
      try {
        await client.send(new UpdateItemCommand({
          userId, itemId: parentItemId,
          name, dueDate, complete: parentItem!.complete ?? false,
          hasDueTime,
          recurrence,
          recurrenceBasis: recurrence ? recurrenceBasis : undefined,
          hasTasks: parentItem!.hasTasks ?? true,
          parentItemId: parentItem!.parentItemId ?? undefined,
          timezoneOffsetMinutes: new Date().getTimezoneOffset(),
        }));
        await renderItems(userId, parentItemId, name);
      } catch (err) {
        showError(String(err));
      }
    });
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

  const ul = document.getElementById("items")!;

  async function load() {
    const res = await client.send(new ListItemsCommand({ userId, parentItemId }));
    const showComplete = (document.getElementById("show-complete") as HTMLInputElement).checked;
    ul.innerHTML = "";
    for (const item of (res.items ?? []).filter((i: ItemSummary) => showComplete || !i.complete)) {
      const li = document.createElement("li");
      li.className = "row";

      // Clicking the name navigates to the item's own screen (its sub-items + an Edit button)
      const nameEl = document.createElement("a");
      nameEl.href = `/users/${userId}/items/${item.itemId}`;
      nameEl.textContent = (item.hasChildren ? "▸ " : "") + (item.name ?? "");
      nameEl.style.flex = "1";
      nameEl.addEventListener("click", (e) => {
        e.preventDefault();
        navigate(`/users/${userId}/items/${item.itemId}`);
      });

      const saveAsChecklistBtn = document.createElement("button");
      saveAsChecklistBtn.textContent = "Save as checklist";
      saveAsChecklistBtn.title = "Create a reusable checklist from this item";
      saveAsChecklistBtn.style.cssText = "font-size:0.78rem;opacity:0.7;";
      saveAsChecklistBtn.addEventListener("click", async () => {
        try {
          await client.send(new CreateTemplateCommand({
            userId,
            name: item.name!,
            sourceItemId: item.itemId!,
          }));
          showSuccess(`Checklist "${item.name}" created.`);
        } catch (err) {
          showError(String(err));
        }
      });

      const deleteBtn = document.createElement("button");
      deleteBtn.textContent = "✕";
      deleteBtn.title = "Delete item";
      deleteBtn.style.color = "#c00";
      deleteBtn.addEventListener("click", async () => {
        const hasKids = item.hasChildren;
        if (hasKids && !confirm(`Delete "${item.name}" and all its sub-items?`)) return;
        try {
          await client.send(new DeleteItemCommand({ userId, itemId: item.itemId! }));
          li.remove();
        } catch (err) {
          showError(String(err));
        }
      });

      const thisHasTasks = item.hasTasks ?? true;
      const completeBtn = document.createElement("button");
      completeBtn.textContent = item.complete ? "☑" : "☐";
      completeBtn.title = item.complete ? "Mark incomplete" : "Mark complete";
      completeBtn.style.color = item.complete ? "#2a9d2a" : "#a8d8f0";
      completeBtn.addEventListener("click", async () => {
        const markingComplete = !item.complete;
        await client.send(new UpdateItemCommand({
          userId, itemId: item.itemId!,
          name: item.name!, dueDate: item.dueDate, complete: !item.complete,
          hasDueTime: item.hasDueTime ?? false,
          hasTasks: item.hasTasks ?? true,
          recurrence: item.recurrence ?? undefined,
          recurrenceBasis: item.recurrenceBasis ?? undefined,
          parentItemId: item.parentItemId ?? undefined,
          timezoneOffsetMinutes: new Date().getTimezoneOffset(),
        }));
        if (markingComplete) {
          showSuccess(item.recurrence ? "✓ Completed — next occurrence scheduled." : "✓ Done!");
        }
        await load();
      });
      if (item.complete) nameEl.style.textDecoration = "line-through";

      li.appendChild(completeBtn);
      li.appendChild(nameEl);

      if (thisHasTasks) {
        const dateSpan = document.createElement("span");
        dateSpan.textContent = item.dueDate ? item.dueDate.toLocaleDateString() : "no due date";
        dateSpan.style.color = "#666";
        dateSpan.style.fontSize = "0.85rem";
        li.appendChild(dateSpan);

        if (item.hasDueTime && item.dueDate) {
          const timeSpan = document.createElement("span");
          timeSpan.textContent = item.dueDate.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
          timeSpan.style.color = "#666";
          timeSpan.style.fontSize = "0.85rem";
          li.appendChild(timeSpan);
        }

        if (item.recurrence) {
          const basisLabel = item.recurrenceBasis === "COMPLETION_DATE" ? "completion" : "due date";
          const recurrenceSpan = document.createElement("span");
          recurrenceSpan.textContent = `↻ ${item.recurrence} (${basisLabel})`;
          recurrenceSpan.className = "recurrence-tag";
          li.appendChild(recurrenceSpan);
        }
      }

      li.appendChild(saveAsChecklistBtn);
      li.appendChild(deleteBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("create-item-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      const raw = (document.getElementById("item-name") as HTMLInputElement).value;
      const names = raw.split("\n").map(s => s.trim()).filter(Boolean);
      const itemHasTasks = (document.getElementById("item-has-tasks") as HTMLSelectElement).value === "tasks";
      const { date: dueDate, hasDueTime } = hasTasks
        ? parseDateTimeInput(
            (document.getElementById("item-due") as HTMLInputElement).value,
            (document.getElementById("item-time") as HTMLInputElement).value,
          )
        : { date: undefined, hasDueTime: false };
      const recurrence = hasTasks ? ((document.getElementById("item-recurrence") as HTMLInputElement).value.trim() || undefined) : undefined;
      const recurrenceBasis = hasTasks ? ((document.getElementById("item-recurrence-basis") as HTMLSelectElement).value as "DUE_DATE" | "COMPLETION_DATE") : undefined;
      for (const name of names) {
        await client.send(new CreateItemCommand({
          userId,
          name,
          dueDate,
          hasDueTime,
          recurrence,
          recurrenceBasis: recurrence ? recurrenceBasis : undefined,
          hasTasks: itemHasTasks,
          parentItemId: parentItemId ?? undefined,
          timezoneOffsetMinutes: new Date().getTimezoneOffset(),
        }));
      }
      if (batchMode) {
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
    <p><a href="/users/${userId}" id="back-users">← Items</a></p>
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
    navigate(`/users/${userId}`);
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
      .filter((i: DueItemSummary) => showComplete || !i.complete)
      .filter((i: DueItemSummary) => preset !== "All with due date" || i.dueDate != null);
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
            userId, itemId: item.itemId!,
            name: item.name!, dueDate: item.dueDate, complete: !item.complete,
            hasDueTime: item.hasDueTime ?? false,
            recurrence: item.recurrence ?? undefined,
            recurrenceBasis: item.recurrenceBasis ?? undefined,
            timezoneOffsetMinutes: new Date().getTimezoneOffset(),
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

      const parentBadge = document.createElement("span");
      parentBadge.textContent = item.parentName ? `[${item.parentName}]` : "";
      parentBadge.style.cssText = "font-size:0.8rem;color:#5aace0;";
      parentBadge.title = "Parent item";

      const deleteBtn = document.createElement("button");
      deleteBtn.textContent = "✕";
      deleteBtn.title = "Delete item";
      deleteBtn.style.color = "#c00";
      deleteBtn.addEventListener("click", async () => {
        try {
          await client.send(new DeleteItemCommand({ userId, itemId: item.itemId! }));
          await load();
        } catch (err) {
          showError(String(err));
        }
      });

      li.appendChild(completeBtn);
      li.appendChild(nameSpan);
      if (item.dueDate) li.appendChild(dateSpan);
      if (item.parentName) li.appendChild(parentBadge);
      li.appendChild(deleteBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("dash-preset")!.addEventListener("change", load);
  document.getElementById("dash-show-complete")!.addEventListener("change", load);

  await load();
}

async function renderChecklists(userId: string) {
  app.innerHTML = `
    <p><a href="/users/${userId}" id="back-items">← Items</a></p>
    <h1>Checklists</h1>
    <button type="button" id="new-checklist-btn">+ New Checklist</button>
    <form id="create-checklist-form" style="display:none;margin-bottom:1rem;">
      <input id="checklist-name" placeholder="Checklist name" required style="margin-right:0.5rem;" />
      <button type="submit">Create</button>
      <button type="button" id="cancel-checklist-btn">Cancel</button>
    </form>
    <ul id="checklists"></ul>`;

  document.getElementById("back-items")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate(`/users/${userId}`);
  });

  const newBtn = document.getElementById("new-checklist-btn")!;
  const form = document.getElementById("create-checklist-form") as HTMLFormElement;

  newBtn.addEventListener("click", () => {
    form.style.display = "";
    newBtn.style.display = "none";
    (document.getElementById("checklist-name") as HTMLInputElement).focus();
  });

  document.getElementById("cancel-checklist-btn")!.addEventListener("click", () => {
    form.style.display = "none";
    newBtn.style.display = "";
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const name = (document.getElementById("checklist-name") as HTMLInputElement).value.trim();
    if (!name) return;
    try {
      await client.send(new CreateTemplateCommand({ userId, name }));
      (document.getElementById("checklist-name") as HTMLInputElement).value = "";
      form.style.display = "none";
      newBtn.style.display = "";
      await load();
    } catch (err) {
      showError(String(err));
    }
  });

  const ul = document.getElementById("checklists")!;

  async function load() {
    const res = await client.send(new ListTemplatesCommand({ userId }));
    ul.innerHTML = "";
    if (!res.items?.length) {
      ul.innerHTML = `<li style="color:#666;">No checklists yet. Create one from an item or use "+ New Checklist".</li>`;
      return;
    }
    for (const item of res.items) {
      const li = document.createElement("li");
      li.className = "row";
      li.style.flexWrap = "wrap";

      const nameSpan = document.createElement("a");
      nameSpan.textContent = item.name ?? "";
      nameSpan.href = `/users/${userId}/checklists/${item.itemId}`;
      nameSpan.style.flex = "1";
      nameSpan.addEventListener("click", (e) => {
        e.preventDefault();
        navigate(`/users/${userId}/checklists/${item.itemId}`);
      });

      const useBtn = document.createElement("button");
      useBtn.textContent = "Use";
      useBtn.title = "Create an item from this checklist";

      // Inline use-form, hidden until "Use" is clicked
      const useForm = document.createElement("form");
      useForm.style.cssText = "display:none;width:100%;margin-top:0.4rem;display:none;gap:0.4rem;align-items:center;flex-wrap:wrap;";
      useForm.innerHTML = `
        <input class="use-name" placeholder="Name" value="${(item.name ?? "").replace(/"/g, "&quot;")}" required style="flex:1;min-width:8rem;" />
        <input class="use-due" type="date" title="Due date (optional)" />
        <button type="submit">Add to items</button>
        <button type="button" class="use-cancel">Cancel</button>`;

      useBtn.addEventListener("click", () => {
        useForm.style.display = "flex";
        useBtn.style.display = "none";
        (useForm.querySelector(".use-name") as HTMLInputElement).focus();
      });

      useForm.querySelector(".use-cancel")!.addEventListener("click", () => {
        useForm.style.display = "none";
        useBtn.style.display = "";
      });

      useForm.addEventListener("submit", async (e) => {
        e.preventDefault();
        const name = (useForm.querySelector(".use-name") as HTMLInputElement).value.trim();
        const dateVal = (useForm.querySelector(".use-due") as HTMLInputElement).value;
        const { date: dueDate, hasDueTime } = parseDateTimeInput(dateVal);
        try {
          await client.send(new CreateItemCommand({
            userId,
            name,
            dueDate,
            hasDueTime,
            hasTasks: item.hasTasks ?? true,
            timezoneOffsetMinutes: new Date().getTimezoneOffset(),
          }));
          showSuccess(`"${name}" added to items.`);
          useForm.style.display = "none";
          useBtn.style.display = "";
        } catch (err) {
          showError(String(err));
        }
      });

      const deleteBtn = document.createElement("button");
      deleteBtn.textContent = "✕";
      deleteBtn.title = "Delete checklist";
      deleteBtn.style.color = "#c00";
      deleteBtn.addEventListener("click", async () => {
        if (!confirm(`Delete checklist "${item.name}"?`)) return;
        try {
          await client.send(new DeleteItemCommand({ userId, itemId: item.itemId! }));
          await load();
        } catch (err) {
          showError(String(err));
        }
      });

      li.appendChild(nameSpan);
      li.appendChild(useBtn);
      li.appendChild(deleteBtn);
      li.appendChild(useForm);
      ul.appendChild(li);
    }
  }

  await load();
}

async function renderChecklistDetail(userId: string, checklistId: string, checklistName: string) {
  app.innerHTML = `
    <p><a href="/users/${userId}/checklists" id="back-checklists">← Checklists</a></p>
    <h1>${checklistName}</h1>
    <button type="button" id="new-child-btn">+ Add Item</button>
    <form id="create-child-form" style="display:none;margin-bottom:1rem;">
      <div class="field-grid">
        <span class="field-label">Name</span>
        <input id="child-name" placeholder="Item name" required />
        <span class="field-label">Offset days</span>
        <input id="child-offset" type="number" placeholder="Days from due date (optional)" title="Positive = after due date, negative = before" />
      </div>
      <button type="submit">Add</button>
      <button type="button" id="cancel-child-btn">Cancel</button>
    </form>
    <ul id="child-items"></ul>`;

  document.getElementById("back-checklists")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate(`/users/${userId}/checklists`);
  });

  const newChildBtn = document.getElementById("new-child-btn")!;
  const createForm = document.getElementById("create-child-form") as HTMLFormElement;

  newChildBtn.addEventListener("click", () => {
    createForm.style.display = "";
    newChildBtn.style.display = "none";
    (document.getElementById("child-name") as HTMLInputElement).focus();
  });

  document.getElementById("cancel-child-btn")!.addEventListener("click", () => {
    createForm.style.display = "none";
    newChildBtn.style.display = "";
  });

  createForm.addEventListener("submit", async (e) => {
    e.preventDefault();
    const name = (document.getElementById("child-name") as HTMLInputElement).value.trim();
    const offsetRaw = (document.getElementById("child-offset") as HTMLInputElement).value;
    const dueOffsetDays = offsetRaw !== "" ? parseInt(offsetRaw, 10) : undefined;
    if (!name) return;
    try {
      await client.send(new CreateItemCommand({
        userId,
        name,
        parentItemId: checklistId,
        dueOffsetDays,
        hasTasks: false,
      }));
      (document.getElementById("child-name") as HTMLInputElement).value = "";
      (document.getElementById("child-offset") as HTMLInputElement).value = "";
      createForm.style.display = "none";
      newChildBtn.style.display = "";
      await load();
    } catch (err) {
      showError(String(err));
    }
  });

  const ul = document.getElementById("child-items")!;

  async function load() {
    const res = await client.send(new ListItemsCommand({ userId, parentItemId: checklistId }));
    ul.innerHTML = "";
    if (!res.items?.length) {
      ul.innerHTML = `<li style="color:#666;">No items yet. Use "+ Add Item" to build this checklist.</li>`;
      return;
    }
    for (const item of res.items ?? []) {
      const li = document.createElement("li");
      li.className = "row";

      const nameEl = makeEditableText(item.name ?? "", async (newVal) => {
        await client.send(new UpdateItemCommand({
          userId, itemId: item.itemId!,
          name: newVal, complete: false,
          hasTasks: item.hasTasks ?? false,
          parentItemId: checklistId,
          dueOffsetDays: item.dueOffsetDays ?? undefined,
        }));
        item.name = newVal;
      });
      nameEl.style.flex = "1";

      const offsetVal = item.dueOffsetDays ?? null;
      const offsetLabel = offsetVal === null ? "no offset"
        : offsetVal === 0 ? "on due date"
        : offsetVal > 0 ? `+${offsetVal}d` : `${offsetVal}d`;
      const offsetEl = makeEditableText(offsetLabel, async (newVal) => {
        const parsed = newVal === "" ? undefined : parseInt(newVal, 10);
        await client.send(new UpdateItemCommand({
          userId, itemId: item.itemId!,
          name: item.name!, complete: false,
          hasTasks: item.hasTasks ?? false,
          parentItemId: checklistId,
          dueOffsetDays: isNaN(parsed as number) ? undefined : parsed,
        }));
        item.dueOffsetDays = isNaN(parsed as number) ? undefined : parsed;
      }, { inputType: "number", inputValue: offsetVal !== null ? String(offsetVal) : "" });
      offsetEl.style.cssText = "color:#666;font-size:0.85rem;";
      offsetEl.title = "Click to edit offset days from due date";

      const deleteBtn = document.createElement("button");
      deleteBtn.textContent = "✕";
      deleteBtn.title = "Remove from checklist";
      deleteBtn.style.color = "#c00";
      deleteBtn.addEventListener("click", async () => {
        try {
          await client.send(new DeleteItemCommand({ userId, itemId: item.itemId! }));
          li.remove();
        } catch (err) {
          showError(String(err));
        }
      });

      li.appendChild(nameEl);
      li.appendChild(offsetEl);
      li.appendChild(deleteBtn);
      ul.appendChild(li);
    }
  }

  await load();
}

// ── Router ───────────────────────────────────────────────────────────────────

let currentAuth: AuthMe | null = null;

async function route() {
  const path = window.location.pathname;

  const checklistDetailMatch = path.match(/^\/users\/([^/]+)\/checklists\/([^/]+)$/);
  if (checklistDetailMatch) {
    const [, uid, cid] = checklistDetailMatch;
    try {
      const item = await client.send(new GetItemCommand({ userId: uid, itemId: cid }));
      await renderChecklistDetail(uid, cid, item.name ?? "Checklist");
    } catch {
      renderNotFound(`Checklist not found.`);
    }
    if (currentAuth) addLogoutButton(`${currentAuth.firstName} ${currentAuth.lastName}`);
    return;
  }

  const checklistsMatch = path.match(/^\/users\/([^/]+)\/checklists$/);
  if (checklistsMatch) {
    await renderChecklists(checklistsMatch[1]);
    if (currentAuth) addLogoutButton(`${currentAuth.firstName} ${currentAuth.lastName}`);
    return;
  }

  const dashMatch = path.match(/^\/users\/([^/]+)\/dashboard$/);
  if (dashMatch) {
    await renderDashboard(dashMatch[1]);
    if (currentAuth) addLogoutButton(`${currentAuth.firstName} ${currentAuth.lastName}`);
    return;
  }

  // /users/:uid/items/:iid — drill into item's children
  const itemDrillMatch = path.match(/^\/users\/([^/]+)\/items\/([^/]+)$/);
  if (itemDrillMatch) {
    const [, uid, iid] = itemDrillMatch;
    try {
      const item = await client.send(new GetItemCommand({ userId: uid, itemId: iid }));
      await renderItems(uid, iid, item.name);
    } catch {
      renderNotFound(`Item not found.`);
    }
    if (currentAuth) addLogoutButton(`${currentAuth.firstName} ${currentAuth.lastName}`);
    return;
  }

  // /users/:uid — top-level items for user
  const userMatch = path.match(/^\/users\/([^/]+)$/);
  if (userMatch) {
    await renderItems(userMatch[1]);
    if (currentAuth) addLogoutButton(`${currentAuth.firstName} ${currentAuth.lastName}`);
    return;
  }

  // Root: redirect to auth user's page
  if (currentAuth) {
    navigate(`/users/${currentAuth.userId}`);
    return;
  }

  renderLogin();
}

window.addEventListener("popstate", route);

(async () => {
  currentAuth = await checkAuth();
  if (!currentAuth) {
    renderLogin();
    return;
  }
  await route();
})();
