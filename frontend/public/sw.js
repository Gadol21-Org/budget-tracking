// Network first, falling back to cache, so the app shell (including hashed assets)
// still opens offline once it has been loaded. API calls are never cached. A server error from the
// proxy (502/503/504, Cloudflare 52x) counts as "no server": the cached copy is used instead of the error page.
const CACHE = "budget-shell-v7";

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
          if (res.ok) cache.put(key, res.clone());
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
