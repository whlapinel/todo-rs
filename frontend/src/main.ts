import {
  ListeriaClient,
  CreateUserCommand,
  ListUsersCommand,
  CreateListCommand,
  ListListsCommand,
  CreateItemCommand,
  ListItemsCommand,
} from "@todo/client";

const client = new ListeriaClient({ endpoint: "http://localhost:3000/api" });

const app = document.getElementById("app")!;

function navigate(path: string) {
  history.pushState(null, "", path);
  route();
}

function showError(container: HTMLElement, msg: string) {
  const p = document.createElement("p");
  p.className = "error";
  p.textContent = msg;
  container.appendChild(p);
  setTimeout(() => p.remove(), 4000);
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
      li.textContent = `${u.firstName} ${u.lastName}`;
      li.addEventListener("click", () => navigate(`/users/${u.userId}`));
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
      showError(app, String(err));
    }
  });

  await load();
}

async function renderLists(userId: string) {
  app.innerHTML = `
    <p><a href="/" id="back-users">← Users</a></p>
    <h1>Lists</h1>
    <p class="subtitle">User: ${userId}</p>
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
      li.textContent = l.name ?? "";
      li.addEventListener("click", () => navigate(`/users/${userId}/lists/${l.listId}`));
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
      showError(app, String(err));
    }
  });

  await load();
}

async function renderItems(userId: string, listId: string) {
  app.innerHTML = `
    <p><a href="/users/${userId}" id="back-lists">← Lists</a></p>
    <h1>Items</h1>
    <p class="subtitle">List: ${listId}</p>
    <form id="create-item-form">
      <input id="item-name" placeholder="Item name" required />
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
      li.textContent = item.name ?? "";
      ul.appendChild(li);
    }
  }

  document.getElementById("create-item-form")!.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      await client.send(new CreateItemCommand({
        userId,
        listId,
        name: (document.getElementById("item-name") as HTMLInputElement).value,
      }));
      (document.getElementById("item-name") as HTMLInputElement).value = "";
      await load();
    } catch (err) {
      showError(app, String(err));
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
