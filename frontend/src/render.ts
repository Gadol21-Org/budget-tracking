import { el } from "./ui";
import { insightsView, type Cmp, type Ctx } from "./insights";
import { currenciesView } from "./views/currencies";
import { businessDashboard, businessesView } from "./business";
import { type View, root, state } from "./state";
import { go, goBack, manageCtx, navigateInsights, openDialog, showCategory } from "./nav";
import { monthBar, monthView } from "./views/month";
import { listView } from "./views/list";
import { logView } from "./views/log";
import { peopleView } from "./views/people";
import { categoriesView } from "./views/categories";
import { settingsView } from "./views/settings";
import { ensureHistory, neededFrom } from "./sync";
import { openTransactionDialog } from "./dialogs/transaction";

export function nav(): HTMLElement {
  const item = (view: View, ico: string, label: string) => {
    const b = el("button", {}, el("span", { className: "ico", textContent: ico }), label);
    if (state.view === view || (view === "settings" && (state.view === "log" || state.view === "categories" || state.view === "businesses" || state.view === "currencies" || state.view === "people")) || (view === "month" && state.view === "business")) b.setAttribute("aria-current", "page");
    b.onclick = () => {
      if (view === "insights") {
        if (state.view === "insights") {
          if (state.insights.cat !== undefined) navigateInsights({}, "push");
          return;
        }
        Object.assign(state.insights, { cat: undefined, sub: undefined });
      }
      if (view === "month") {
        // Tapping the open tab goes back up from a category to the summary itself.
        if (state.view === "month") {
          if (state.insights.cat !== undefined) navigateInsights({}, "push");
          return;
        }
        Object.assign(state.insights, { cat: undefined, sub: undefined });
      }
      go(view);
    };
    return b;
  };
  return el("nav", { className: "nav" }, item("month", "📊", "סיכום"), item("list", "📋", "תנועות"), item("insights", "📈", "תובנות"), item("settings", "⚙️", "הגדרות"));
}

/** What the dashboards (and the summary's category breakdown) need from the app. */
export function insightsCtx(): Ctx {
  return {
    txs: state.txs,
    cats: state.cats,
    period: state.period,
    ins: state.insights,
    monthBar,
    navigate: navigateInsights,
    setCmp: (c: Cmp) => ((state.insights.cmp = c), render()),
    setPeriod: (p) => ((state.period = p), render()),
    openList: showCategory,
    // Categories open on the summary page: a new history entry, so back returns to where the tap came from.
    openCategory: (cat, sub) => {
      Object.assign(state.insights, { dash: "cats", side: "expense", cat, sub });
      state.kind = "expense";
      go("month");
    },
    openDialog,
  };
}

export function insightsScreen(): HTMLElement {
  return insightsView(insightsCtx());
}

export function businessScreen(): HTMLElement {
  return businessDashboard({
    txs: state.txs,
    cats: state.cats,
    biz: state.biz,
    period: state.period,
    view: state.bizView,
    monthBar,
    setView: (patch) => ((state.bizView = { ...state.bizView, ...patch }), render()),
    openDialog,
    back: () => goBack("month"),
  });
}

export let renderedView: View | undefined;

export function render() {
  ensureHistory();
  const view = { month: monthView, list: listView, insights: insightsScreen, log: logView, categories: categoriesView, settings: settingsView, business: businessScreen, businesses: () => businessesView(manageCtx()), currencies: () => currenciesView({ back: () => goBack("settings") }), people: peopleView }[state.view]();
  // A different screen opens at its top, not wherever the previous one was scrolled; re-rendering the same one keeps the position.
  const entering = renderedView !== state.view;
  if (entering) window.scrollTo(0, 0);
  renderedView = state.view;
  const fab = el("button", { className: "fab", title: "הוספת תנועה", textContent: "+" });
  fab.onclick = () => openTransactionDialog();
  const banner = state.offline ? el("div", { className: "banner", role: "status", textContent: "אין חיבור או שהשרת אינו זמין: מוצגים הנתונים האחרונים. תנועות חדשות יישמרו וישלחו כשהשרת יחזור." }) : null;
  // Offline, only the last three months are on the device: say so instead of showing an empty period.
  const missing = state.offline && state.loadedFrom !== undefined && neededFrom() < state.loadedFrom;
  const gap = missing ? el("div", { className: "banner", role: "status", textContent: "חודשים ישנים זמינים רק עם חיבור לשרת. כאן מוצגים רק 3 החודשים האחרונים." }) : null;
  // A floating label, not part of the page: nothing moves when it appears or goes.
  const older = state.loadingOlder ? el("div", { className: "loading-toast", role: "status", textContent: "טוען חודשים קודמים…" }) : null;
  const main = el("main", {}, banner, gap, view);
  main.dataset.view = state.view;
  root.replaceChildren(el("div", { className: "shell" }, nav(), main), fab, ...(older ? [older] : []));
  // Transactions open focused on the latest day that actually happened; upcoming days stay reachable above.
  if (entering && state.view === "list") document.getElementById("now-anchor")?.scrollIntoView({ block: "start" });
}
