import { Fragment, type ReactElement } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { type Cmp, type Ctx } from "./insights/model";
import { insightsView } from "./views/insights";
import { currenciesView } from "./views/currencies";
import { businessDashboard, productsView } from "./views/business";
import { actionsPage } from "./views/actionsPage";
import { type View, root, state } from "./state";
import { go, goBack, manageCtx, navigateBusiness, navigateInsights, openBusinessDashboard, showCategory } from "./nav";
import { monthView } from "./views/month";
import { listView } from "./views/list";
import { logView } from "./views/log";
import { trackScreen } from "./analytics";
import { peopleView } from "./views/people";
import { categoriesView } from "./views/categories";
import { settingsView } from "./views/settings";
import { ensureHistory, neededFrom } from "./sync";
import { openTransactionDialog } from "./dialogs/transaction";

/** Tab icons: Lucide (ISC licence), drawn with currentColor so a theme can colour and size them. */
const ICONS: Record<string, string[]> = {
  month: ["M21.21 15.89A10 10 0 1 1 8 2.83", "M22 12A10 10 0 0 0 12 2v10z"],
  list: ["M3 12h.01", "M3 18h.01", "M3 6h.01", "M8 12h13", "M8 18h13", "M8 6h13"],
  business: ["M16 20V4a2 2 0 0 0-2-2h-4a2 2 0 0 0-2 2v16", "M20 6H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2z"],
  insights: ["M22 7 13.5 15.5 8.5 10.5 2 17", "M16 7h6v6"],
  businessOverview: ["M22 7 13.5 15.5 8.5 10.5 2 17", "M16 7h6v6"],
  products: ["M6 22V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v18Z", "M6 12H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h2", "M18 9h2a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-2", "M10 6h4", "M10 10h4", "M10 14h4", "M10 18h4"],
  actions: ["M3 3h7v7H3z", "M14 3h7v7h-7z", "M14 14h7v7h-7z", "M3 14h7v7H3z"],
  exit: ["M5 12h14", "m12 5 7 7-7 7"],
  settings: ["M10 5H3", "M12 19H3", "M14 3v4", "M16 17v4", "M21 12h-9", "M21 19h-5", "M21 5h-7", "M8 10v4", "M8 12H3"],
};

function TabIcon({ view }: { view: string }) {
  return (
    <svg className="ico" viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {ICONS[view].map((d) => (
        <path key={d} d={d} />
      ))}
    </svg>
  );
}

const TABS: [View | "business", string][] = [
  ["month", "סיכום"],
  ["list", "תנועות"],
  ["insights", "תובנות"],
  ["business", "העסק"],
  ["settings", "הגדרות"],
];

const BUSINESS_SCREENS: View[] = ["actions", "businessOverview", "products"];
const SETTINGS_SCREENS: View[] = ["log", "categories", "currencies", "people"];

function openTab(view: View | "business") {
  if (view === "business") return openBusinessDashboard();
  // Tapping the open tab goes back up from a category; opening another one starts at its top level.
  if ((view === "insights" || view === "month") && state.view === view) {
    if (state.insights.cat !== undefined) navigateInsights({}, "push");
    return;
  }
  if (view === "insights" || view === "month") Object.assign(state.insights, { cat: undefined, sub: undefined });
  go(view);
}

/** Inside the business section the tray holds the business's own items; the first leaves it. Add an item here for each new action. */
const BUSINESS_TABS: [View, string, () => void, string][] = [
  ["month", "יציאה מהעסק", () => openTab("month"), "exit"],
  ["businessOverview", "סקירה", openBusinessDashboard, "business"],
  ["actions", "פעולות", () => go("actions"), "actions"],
  ["products", "מוצרים", () => go("products"), "products"],
];

function Nav() {
  if (BUSINESS_SCREENS.includes(state.view)) {
    return (
      <nav className="nav">
        {BUSINESS_TABS.map(([view, label, open, icon]) => (
          <button key={view} aria-current={state.view === view ? "page" : undefined} onClick={open}>
            <TabIcon view={icon} />
            <span>{label}</span>
          </button>
        ))}
      </nav>
    );
  }
  return (
    <nav className="nav">
      {TABS.map(([view, label]) => {
        const current = (view !== "business" && state.view === view) || (view === "settings" && SETTINGS_SCREENS.includes(state.view)) || (view === "business" && BUSINESS_SCREENS.includes(state.view));
        return (
          <button key={view} aria-current={current ? "page" : undefined} onClick={() => openTab(view)}>
            <TabIcon view={view} />
            <span>{label}</span>
          </button>
        );
      })}
    </nav>
  );
}

/** What the dashboards (and the summary's category breakdown) need from the app. */
export function insightsCtx(): Ctx {
  return {
    txs: state.txs,
    cats: state.cats,
    period: state.period,
    ins: state.insights,
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
  };
}

function businessScreen() {
  return businessDashboard({
    txs: state.txs,
    cats: state.cats,
    products: state.products,
    period: state.period,
    view: state.businessView,
    setView: (patch) => navigateBusiness({ ...(patch.scope !== undefined ? { all: false } : {}), ...patch }, patch.scope !== undefined && patch.scope !== state.businessView.scope ? "push" : "replace"),
  });
}

const screens: Record<View, () => ReactElement> = {
  month: monthView,
  list: listView,
  insights: () => insightsView(insightsCtx()),
  log: logView,
  categories: categoriesView,
  settings: settingsView,
  actions: actionsPage,
  businessOverview: businessScreen,
  products: () => productsView(manageCtx()),
  currencies: () => currenciesView({ back: () => goBack("settings") }),
  people: peopleView,
};

function App() {
  const banner = state.offline ? "אין חיבור או שהשרת אינו זמין: מוצגים הנתונים האחרונים. תנועות חדשות יישמרו וישלחו כשהשרת יחזור." : null;
  // Offline, only the last three months are on the device: say so instead of showing an empty period.
  const gap = state.offline && state.loadedFrom !== undefined && neededFrom() < state.loadedFrom ? "חודשים ישנים זמינים רק עם חיבור לשרת. כאן מוצגים רק 3 החודשים האחרונים." : null;
  return (
    <>
      <div className="shell">
        <Nav />
        <main data-view={state.view}>
          {banner && <div className="banner" role="status">{banner}</div>}
          {gap && <div className="banner" role="status">{gap}</div>}
          <Fragment key={screenKey()}>{screens[state.view]()}</Fragment>
        </main>
      </div>
      <button className="fab" title="הוספת תנועה" onClick={() => openTransactionDialog()}>+</button>
      {/* A floating label, not part of the page: nothing moves when it appears or goes. */}
      {state.loadingOlder && <div className="loading-toast" role="status">טוען חודשים קודמים…</div>}
    </>
  );
}

/**
 * What the screen shows, apart from its data. A screen starts afresh when this changes (another screen, period,
 * category or tab); a data refresh keeps what was typed or picked on it (a search, a half-typed name, a message).
 */
const screenKey = () =>
  JSON.stringify([state.view, state.filter?.id, state.period, state.kind, state.catKind, state.productFilter, state.businessView, state.insights.dash, state.insights.cat, state.insights.sub, state.insights.side, state.insights.cmp]);

const app = createRoot(root);
let renderedView: View | undefined;

/** Show a plain message instead of the app (while loading, or when loading failed). */
export function showStatus(text: string, error = false) {
  flushSync(() => app.render(<main><div className={error ? "empty error" : "empty"}>{text}</div></main>));
}

export function render() {
  ensureHistory();
  flushSync(() => app.render(<App />));
  // A different screen opens at its top, not wherever the previous one was scrolled; re-rendering the same one keeps the position.
  const entering = renderedView !== state.view;
  if (entering) window.scrollTo(0, 0);
  renderedView = state.view;
  trackScreen(state.view);
  // Transactions open focused on the latest day that actually happened; upcoming days stay reachable above.
  if (entering && state.view === "list") document.getElementById("now-anchor")?.scrollIntoView({ block: "start" });
}
