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

// A push carries no data and does not say who sent it: ask the server what is new. Charges found in email arrive the moment
// the mail does, so a fresh one means this push is theirs; otherwise it is the Sumit poll, with the receipts not yet looked at.
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

const emailNote = (mail, opts) => {
  const added = mail.filter((l) => l.auto);
  const review = mail.filter((l) => !l.auto);
  let title = "חיוב חדש מדוא״ל";
  let body;
  if (added.length && review.length) {
    title = "חיובים חדשים מדוא״ל";
    body = `נוספו:\n${added.map(mailLine).join("\n")}\nממתינות לבדיקה:\n${review.map(mailLine).join("\n")}`;
  } else if (added.length) {
    title = "תנועה נוספה מדוא״ל";
    body = added.map(mailLine).join("\n");
  } else {
    body = review.map(mailLine).join("\n");
  }
  // The click opens the email list.
  return self.registration.showNotification(title, { ...opts, body, data: { email: true } });
};

const receiptNote = (unread, opts) => {
  if (unread.length <= 1) {
    const n = unread[0];
    return self.registration.showNotification(n ? n.title : "התקבלה קבלה חדשה", { ...opts, body: n ? n.body : "" });
  }
  // Several at once: one notification that says what came in, with a line for each.
  const held = unread.filter((n) => n.title.includes("ממתי")).length;
  const refunds = unread.filter((n) => !n.title.includes("ממתי") && n.title.includes("זיכוי")).length;
  const receipts = unread.length - held - refunds;
  const parts = [];
  if (receipts) parts.push(receipts === 1 ? "קבלה חדשה" : `${receipts} קבלות חדשות`);
  if (refunds) parts.push(refunds === 1 ? "זיכוי" : `${refunds} זיכויים`);
  if (held) parts.push(`${held} ממתינים לסיווג`);
  const lines = unread.slice(0, 5).map((n) => n.title);
  if (unread.length > 5) lines.push(`ועוד ${unread.length - 5}`);
  return self.registration.showNotification(parts.join(", "), { ...opts, body: lines.join("\n") });
};

self.addEventListener("push", (e) => {
  // Each kind has its own tag: a newer email notification replaces an older email one, but never a Sumit one (and the other way round).
  const base = { dir: "rtl", lang: "he", icon: "/icon-192-v2.png" };
  e.waitUntil(
    getJson("/api/email/status", (o) => o.latest || []).then(async (mail) => {
      if (mail.length) return emailNote(mail, { ...base, tag: "email-suggestion" });
      return receiptNote(await getJson("/api/sumit", (o) => o.notices.filter((n) => !n.read)), { ...base, tag: "sumit-receipt" });
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
