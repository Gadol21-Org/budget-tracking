import "./style.css";
import "./gestures";
import "./widgets";
import { el } from "./ui";
import { root } from "./state";
import { load } from "./sync";
import { screenState } from "./nav";
import { openTransactionDialog } from "./dialogs/transaction";

root.replaceChildren(el("main", {}, el("div", { className: "empty", textContent: "טוען…" })));

load()
  .then(openShortcutAction)
  .catch((e) => {
    root.replaceChildren(el("main", {}, el("div", { className: "empty error", textContent: `לא ניתן לטעון נתונים: ${e}` })));
  });

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});

/** App-icon shortcuts (see manifest.webmanifest) start the app at /?action=add-expense or /?action=add-income. */
function openShortcutAction() {
  const action = new URLSearchParams(location.search).get("action");
  if (action !== "add-expense" && action !== "add-income") return;
  history.replaceState(screenState({ n: 0 }), "", location.pathname);
  openTransactionDialog(undefined, action === "add-income");
}
