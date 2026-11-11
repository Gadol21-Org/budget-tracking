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

// A push carries no data: ask the server what is new (Sumit receipts, charges found in email) and show it as one notification.
const SYMBOL = { ILS: "₪", USD: "$", EUR: "€", GBP: "£", JPY: "¥" };
const priceOf = (l) => {
  const places = l.currency === "JPY" ? 0 : 2;
  return `${SYMBOL[l.currency] ?? l.currency + " "}${(Math.abs(l.amount_minor) / 10 ** places).toFixed(places)}`;
};
const mailLine = (l) => `${priceOf(l)} · ${l.note} · ${l.date.split("-").reverse().join("/")}`;
const getJson = (url, pick) =>
  fetch(url, { cache: "no-store" })
    .then((r) => r.json())
    .then(pick)
    .catch(() => []);

// What each source has to say: a heading for the title and a line for each item.
const receiptParts = (unread) => {
  const held = unread.filter((n) => n.title.includes("ממתי")).length;
  const refunds = unread.filter((n) => !n.title.includes("ממתי") && n.title.includes("זיכוי")).length;
  const receipts = unread.length - held - refunds;
  const parts = [];
  if (receipts) parts.push(receipts === 1 ? "קבלה חדשה" : `${receipts} קבלות חדשות`);
  if (refunds) parts.push(refunds === 1 ? "זיכוי" : `${refunds} זיכויים`);
  if (held) parts.push(`${held} ממתינים לסיווג`);
  return parts;
};
const mailParts = (added, review) => {
  const parts = [];
  if (added.length) parts.push(added.length === 1 ? "תנועה נוספה מדוא״ל" : `${added.length} תנועות נוספו מדוא״ל`);
  if (review.length) parts.push(review.length === 1 ? "חיוב מדוא״ל ממתין לבדיקה" : `${review.length} חיובים מדוא״ל ממתינים לבדיקה`);
  return parts;
};

self.addEventListener("push", (e) => {
  e.waitUntil(
    Promise.all([getJson("/api/sumit", (o) => o.notices.filter((n) => !n.read)), getJson("/api/email/status", (o) => o.latest || [])]).then(([unread, mail]) => {
      const opts = { tag: "budget-new", dir: "rtl", lang: "he", icon: "/icon-192-v2.png" };
      const added = mail.filter((l) => l.auto);
      const review = mail.filter((l) => !l.auto);
      // Only email: the click opens the email list; with receipts as well it opens the app.
      opts.data = { email: mail.length > 0 && unread.length === 0 };
      if (!unread.length && !mail.length) return self.registration.showNotification("התקבלה קבלה חדשה", opts);
      if (unread.length === 1 && !mail.length) return self.registration.showNotification(unread[0].title, { ...opts, body: unread[0].body });
      // One notification that says what came in, with a line for each (at most 5 of each kind).
      const lines = unread.slice(0, 5).map((n) => n.title);
      if (unread.length > 5) lines.push(`ועוד ${unread.length - 5}`);
      if (added.length) lines.push(...(unread.length || review.length ? ["נוספו מדוא״ל:"] : []), ...added.map(mailLine));
      if (review.length) lines.push(...(unread.length || added.length ? ["ממתינות לבדיקה:"] : []), ...review.map(mailLine));
      return self.registration.showNotification([...receiptParts(unread), ...mailParts(added, review)].join(", "), { ...opts, body: lines.join("\n") });
    }),
  );
});

self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  // Only email in it: open the email list (tell a running app to go there, or start the app on it).
  const email = e.notification.data?.email;
  e.waitUntil(
    self.clients.matchAll({ type: "window" }).then((cs) => {
      if (!cs.length) return self.clients.openWindow(email ? "/?action=email-inbox" : "/");
      if (email) cs[0].postMessage({ type: "open-email-inbox" });
      return cs[0].focus();
    }),
  );
});
