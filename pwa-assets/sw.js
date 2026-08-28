// Service worker for PRL. Scope is /web/ (see Service-Worker-Allowed handling
// in src/main.rs, since this file is served from /web/static/sw.js). Content
// here is always live/user-specific, so navigations go network-first with an
// offline fallback page rather than serving stale cached HTML. Static assets
// (css/js/icons under /web/static/) are cached so the app shell still loads
// (and can render the offline page's own styling) with no connection.
const CACHE_VERSION = "v1";
const STATIC_CACHE = `prl-static-${CACHE_VERSION}`;
const OFFLINE_URL = "/web/static/offline.html";

const PRECACHE_URLS = [
  "/web/static/style.css",
  "/web/static/js/htmx.min.js",
  "/web/static/icons/icon-192.png",
  OFFLINE_URL,
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(STATIC_CACHE).then((cache) => cache.addAll(PRECACHE_URLS))
  );
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys.filter((key) => key !== STATIC_CACHE).map((key) => caches.delete(key))
        )
      )
  );
  self.clients.claim();
});

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  // API/auth requests always need a real network round trip — never intercept them.
  if (url.pathname.startsWith("/api/") || url.pathname.startsWith("/auth/")) return;

  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request).catch(() => caches.match(OFFLINE_URL))
    );
    return;
  }

  if (url.pathname.startsWith("/web/static/")) {
    event.respondWith(
      caches.match(request).then(
        (cached) =>
          cached ||
          fetch(request).then((response) => {
            const copy = response.clone();
            caches.open(STATIC_CACHE).then((cache) => cache.put(request, copy));
            return response;
          })
      )
    );
  }
});

// Push notifications (docs/push-notifications-plan.md) — the push service delivers an
// already-encrypted message; by the time it reaches here the browser has decrypted it and
// event.data is the plain JSON payload src/push.rs::send_push built ({title, body, url}).
self.addEventListener("push", (event) => {
  const data = event.data ? event.data.json() : {};
  event.waitUntil(
    self.registration.showNotification(data.title || "PRL", {
      body: data.body,
      icon: "/web/static/icons/icon-192.png",
      data: { url: data.url || "/web/tasks" },
    })
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(clients.openWindow(event.notification.data.url));
});
