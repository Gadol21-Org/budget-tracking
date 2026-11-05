import "./style.css";
import "./themes/material.css";
import "./gestures";
import "./widgets";
import { el } from "./ui";
import { showStatus } from "./render";
import { load, showCached } from "./sync";
import { screenState } from "./nav";
import { openTransactionDialog } from "./dialogs/transaction";

showStatus("טוען…");

// Show the last-seen data at once; the fresh data is fetched behind it and replaces it when it arrives.
showCached()
  .catch(() => false)
  .then(() => load())
  .then(openShortcutAction)
  .catch((e) => {
    showStatus(`לא ניתן לטעון נתונים: ${e}`, true);
  });

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
  // The service worker stores a newer app page in the background (used from the next launch): offer a relaunch, never reload on its own.
  navigator.serviceWorker.addEventListener("message", (e) => e.data?.type === "shell-updated" && void checkForUpdate());
  setTimeout(() => void checkForUpdate(), 3000);
}

const assetsOf = (html: string) => [...html.matchAll(/\/assets\/[^"'\s]+/g)].map((m) => m[0]).sort().join();

/** Is the stored app page a different version (other hashed files) from the one running? Then show the banner. */
async function checkForUpdate() {
  if (document.querySelector(".update-banner")) return;
  try {
    const stored = await (await caches.match("/"))?.text();
    const running = [...document.querySelectorAll<HTMLElement>("script[src], link[href]")].map((n) => (n as HTMLScriptElement).src || (n as HTMLLinkElement).href);
    const mine = assetsOf(running.map((u) => `"${new URL(u).pathname}"`).join(" "));
    if (!stored || !mine || assetsOf(stored) === mine) return;
    document.body.append(
      el("div", { className: "update-banner", role: "status" }, el("span", { textContent: "גרסה חדשה זמינה" }), el("button", { textContent: "הפעלה מחדש", onclick: () => location.reload() })),
    );
  } catch {
    /* no cache support: no banner */
  }
}

/** App-icon shortcuts (see manifest.webmanifest) start the app at /?action=add-expense or /?action=add-income. */
function openShortcutAction() {
  const action = new URLSearchParams(location.search).get("action");
  if (action !== "add-expense" && action !== "add-income") return;
  history.replaceState(screenState({ n: 0 }), "", location.pathname);
  openTransactionDialog(undefined, action === "add-income");
}
