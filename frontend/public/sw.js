// Network first, falling back to cache, so the app shell (including hashed assets)
// still opens offline once it has been loaded. API calls are never cached. A server error from the
// proxy (502/503/504, Cloudflare 52x) counts as "no server": the cached copy is used instead of the error page.
const CACHE = "budget-shell-v8";

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(["/", "/manifest.webmanifest", "/icon-v2.svg", "/icon-192-v2.png"])));
  self.skipWaiting();
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

// Stale-while-revalidate: the cached copy is served at once and refreshed in the background, so launching never waits
// for the network. A new version is downloaded behind the scenes and takes effect on the next launch.
// Hashed assets never change, so they are simply served from the cache once stored.
self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin || url.pathname.startsWith("/api/")) return;
  const hashed = url.pathname.startsWith("/assets/");
  e.respondWith(
    caches.open(CACHE).then(async (cache) => {
      const key = e.request.mode === "navigate" ? "/" : e.request;
      const cached = await cache.match(key);
      if (cached && hashed) return cached;
      const network = fetch(e.request)
        .then((res) => {
          if (res.status >= 500) throw new TypeError("server unavailable");
          if (res.ok) {
            const copy = res.clone();
            // The new page's hashed files are stored with it, so the stored page and its files always match (also offline).
            const files = e.request.mode === "navigate" ? res.clone().text().then((html) => Promise.all([...new Set(html.match(/\/assets\/[^"'\s]+/g) ?? [])].map((u) => cache.match(u).then((hit) => hit || cache.add(u))))) : Promise.resolve();
            // Store the page, then tell the open page so it can see whether it is a newer version than the one running.
            const stored = Promise.all([files, cache.put(key, copy)]).then(async () => {
              if (e.request.mode === "navigate") for (const c of await self.clients.matchAll()) c.postMessage({ type: "shell-updated" });
            });
            e.waitUntil(stored.catch(() => {}));
          }
          return res;
        })
        .catch(() => undefined);
      if (cached) {
        e.waitUntil(network);
        return cached;
      }
      return (await network) ?? (await cache.match("/")) ?? Response.error();
    }),
  );
});

// A push carries its text (title, body, tag and the page to open when tapped), encrypted for this device by the server.
// A device the server has no keys for yet only gets an empty wake-up: it shows a plain notice until it is subscribed again.
self.addEventListener("push", (e) => {
  let msg = null;
  try {
    msg = e.data ? e.data.json() : null;
  } catch {
    /* an empty or unreadable push */
  }
  e.waitUntil(
    self.registration.showNotification(msg?.title || "יש חדש באפליקציה", {
      body: msg?.body || "",
      tag: msg?.tag || "budget-new",
      dir: "rtl",
      lang: "he",
      icon: "/icon-192-v2.png",
      data: { url: msg?.url || "/" },
    }),
  );
});

self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  // Open the page the push named: tell a running app to go there, or start the app on it.
  const url = e.notification.data?.url || "/";
  e.waitUntil(
    self.clients.matchAll({ type: "window" }).then((cs) => {
      if (!cs.length) return self.clients.openWindow(url);
      if (url.includes("action=email-inbox")) cs[0].postMessage({ type: "open-email-inbox" });
      return cs[0].focus();
    }),
  );
});
