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
  CreateItemCommand,
  ListItemsCommand,
  UpdateItemCommand,
} from "@todo/client";

const client = new ListeriaClient({ endpoint: "http://localhost:3000/api" });

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
    input.style.cssText = "padding:0.2rem;border:1px solid #999;border-radius:3px;";
span.replaceWith(input);
    input.focus();

    const finish = async () => {
      const newVal = input.value;
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

      li.appendChild(nameSpan);
      li.appendChild(goBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("create-user-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      await client.send(new CreateUserCommand({
        firstName: (document.getElementById("first-name") as HTMLInputElement).value,
        lastName: (document.getElementById("last-name") as HTMLInputElement).value,
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
        }));
      });
      nameSpan.style.flex = "1";

      const goBtn = document.createElement("button");
      goBtn.textContent = "Items →";
      goBtn.addEventListener("click", () => navigate(`/users/${userId}/lists/${l.listId}`));

      li.appendChild(nameSpan);
      li.appendChild(goBtn);
      ul.appendChild(li);
    }
  }

  document.getElementById("create-list-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      await client.send(new CreateListCommand({
        userId,
        name: (document.getElementById("list-name") as HTMLInputElement).value,
      }));
      (document.getElementById("list-name") as HTMLInputElement).value = "";
      await load();
    } catch (err) {
      showError(String(err));
    }
  });

  await load();
}

function parseDateInput(value: string): Date | undefined {
  if (!value) return undefined;
  const [year, month, day] = value.split("-").map(Number);
  return new Date(year, month - 1, day); // local midnight, avoids UTC off-by-one
}

async function renderItems(userId: string, listId: string) {
  try {
    await client.send(new GetListCommand({ userId, listId }));
  } catch {
    renderNotFound(`List "${listId}" does not exist.`);
    return;
  }

  app.innerHTML = `
    <p><a href="/users/${userId}" id="back-lists">← Lists</a></p>
    <h1>Items</h1>
    <form id="create-item-form">
      <input id="item-name" placeholder="Item name" required />
      <input id="item-due" type="date" title="Due date (optional)" />
      <button type="submit">Add Item</button>
    </form>
    <ul id="items"></ul>`;

  document.getElementById("back-lists")!.addEventListener("click", (e) => {
    e.preventDefault();
    navigate(`/users/${userId}`);
  });

  const ul = document.getElementById("items")!;

  async function load() {
    const res = await client.send(new ListItemsCommand({ userId, listId }));
    ul.innerHTML = "";
    for (const item of res.items ?? []) {
      const li = document.createElement("li");
      li.className = "row";

      const nameSpan = makeEditableText(item.name ?? "", async (newVal) => {
        await client.send(new UpdateItemCommand({
          userId, listId, itemId: item.itemId!, name: newVal,
          dueDate: item.dueDate,
        }));
      });

      const isoDate = item.dueDate ? item.dueDate.toISOString().slice(0, 10) : "";
      const dateSpan = makeEditableText(
        item.dueDate ? item.dueDate.toLocaleDateString() : "no due date",
        async (newVal) => {
          const d = parseDateInput(newVal);
          await client.send(new UpdateItemCommand({
            userId, listId, itemId: item.itemId!, name: item.name!, dueDate: d,
          }));
          item.dueDate = d;
        },
        { inputType: "date", inputValue: isoDate }
      );
      dateSpan.style.color = "#666";
      dateSpan.style.fontSize = "0.85rem";

      li.appendChild(nameSpan);
      li.appendChild(dateSpan);
      ul.appendChild(li);
    }
  }

  document.getElementById("create-item-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      await client.send(new CreateItemCommand({
        userId, listId,
        name: (document.getElementById("item-name") as HTMLInputElement).value,
        dueDate: parseDateInput(
          (document.getElementById("item-due") as HTMLInputElement).value
        ),
      }));
      (document.getElementById("item-name") as HTMLInputElement).value = "";
      (document.getElementById("item-due") as HTMLInputElement).value = "";
      await load();
    } catch (err) {
      showError(String(err));
    }
  });

  await load();
}

// ── Router ───────────────────────────────────────────────────────────────────

async function route() {
  const path = window.location.pathname;

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
