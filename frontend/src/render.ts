import { el } from "./ui";
import { insightsView, type Cmp } from "./insights";
import { currenciesView } from "./currencies";
import { businessDashboard, businessesView } from "./business";
import { type View, root, state } from "./state";
import { go, goBack, manageCtx, navigateInsights, openDialog, showCategory } from "./nav";
import { monthBar, monthView } from "./views/month";
import { listView } from "./views/list";
import { logView } from "./views/log";
import { categoriesView } from "./views/categories";
import { settingsView } from "./views/settings";
import { openTransactionDialog } from "./dialogs/transaction";

export function nav(): HTMLElement {
  const item = (view: View, ico: string, label: string) => {
    const b = el("button", {}, el("span", { className: "ico", textContent: ico }), label);
    if (state.view === view || (view === "settings" && (state.view === "log" || state.view === "categories" || state.view === "businesses" || state.view === "currencies")) || (view === "month" && state.view === "business")) b.setAttribute("aria-current", "page");
    b.onclick = () => {
      if (view === "insights") {
        if (state.view === "insights") {
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

export function insightsScreen(): HTMLElement {
  return insightsView({
    txs: state.txs,
    cats: state.cats,
    period: state.period,
    ins: state.insights,
    monthBar,
    navigate: navigateInsights,
    setCmp: (c: Cmp) => ((state.insights.cmp = c), render()),
    setPeriod: (p) => ((state.period = p), render()),
    openList: showCategory,
    openDialog,
  });
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
  const view = { month: monthView, list: listView, insights: insightsScreen, log: logView, categories: categoriesView, settings: settingsView, business: businessScreen, businesses: () => businessesView(manageCtx()), currencies: () => currenciesView({ back: () => goBack("settings") }) }[state.view]();
  // A different screen opens at its top, not wherever the previous one was scrolled; re-rendering the same one keeps the position.
  const entering = renderedView !== state.view;
  if (entering) window.scrollTo(0, 0);
  renderedView = state.view;
  const fab = el("button", { className: "fab", title: "הוספת תנועה", textContent: "+" });
  fab.onclick = () => openTransactionDialog();
  const banner = state.offline ? el("div", { className: "banner", role: "status", textContent: "אין חיבור: מוצגים הנתונים האחרונים. תנועות חדשות יישמרו וישלחו כשהחיבור יחזור." }) : null;
  const main = el("main", {}, banner, view);
  main.dataset.view = state.view;
  root.replaceChildren(el("div", { className: "shell" }, nav(), main), fab);
  // Transactions open focused on the latest day that actually happened; upcoming days stay reachable above.
  if (entering && state.view === "list") document.getElementById("now-anchor")?.scrollIntoView({ block: "start" });
}
