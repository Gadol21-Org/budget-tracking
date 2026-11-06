import { Fragment, type ReactElement } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { type Cmp, type Ctx } from "./insights/model";
import { insightsView } from "./views/insights";
import { currenciesView } from "./views/currencies";
import { businessDashboard, businessesView } from "./views/business";
import { type View, root, state } from "./state";
import { go, goBack, manageCtx, navigateInsights, showCategory } from "./nav";
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
  insights: ["M22 7 13.5 15.5 8.5 10.5 2 17", "M16 7h6v6"],
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

const TABS: [View, string][] = [
  ["month", "סיכום"],
  ["list", "תנועות"],
  ["insights", "תובנות"],
  ["settings", "הגדרות"],
];

const SETTINGS_SCREENS: View[] = ["log", "categories", "businesses", "currencies", "people"];

function openTab(view: View) {
  // Tapping the open tab goes back up from a category; opening another one starts at its top level.
  if ((view === "insights" || view === "month") && state.view === view) {
    if (state.insights.cat !== undefined) navigateInsights({}, "push");
    return;
  }
  if (view === "insights" || view === "month") Object.assign(state.insights, { cat: undefined, sub: undefined });
  go(view);
}

function Nav() {
  return (
    <nav className="nav">
      {TABS.map(([view, label]) => {
        const current = state.view === view || (view === "settings" && SETTINGS_SCREENS.includes(state.view)) || (view === "month" && state.view === "business");
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
    biz: state.biz,
    period: state.period,
    view: state.bizView,
    setView: (patch) => ((state.bizView = { ...state.bizView, ...patch }), render()),
    back: () => goBack("month"),
  });
}

const screens: Record<View, () => ReactElement> = {
  month: monthView,
  list: listView,
  insights: () => insightsView(insightsCtx()),
  log: logView,
  categories: categoriesView,
  settings: settingsView,
  business: businessScreen,
  businesses: () => businessesView(manageCtx()),
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
  JSON.stringify([state.view, state.filter?.id, state.period, state.kind, state.catKind, state.bizFilter, state.bizView, state.insights.dash, state.insights.cat, state.insights.sub, state.insights.side, state.insights.cmp]);

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
