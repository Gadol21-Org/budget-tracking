import { Fragment, type ReactElement } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { type Cmp, type Ctx } from "./insights/model";
import { insightsView } from "./views/insights";
import { currenciesView } from "./views/currencies";
import { bagView, bagsView } from "./views/bags";
import { businessDashboard } from "./views/business";
import { type View, root, state } from "./state";
import { go, goBack, navigateInsights, showCategory } from "./nav";
import { monthView } from "./views/month";
import { listView } from "./views/list";
import { logView } from "./views/log";
import { peopleView } from "./views/people";
import { categoriesView } from "./views/categories";
import { settingsView } from "./views/settings";
import { ensureHistory, neededFrom } from "./sync";
import { openTransactionDialog } from "./dialogs/transaction";

const TABS: [View, string, string][] = [
  ["month", "📊", "סיכום"],
  ["list", "📋", "תנועות"],
  ["insights", "📈", "תובנות"],
  ["bags", "🎒", "טיוגים"],
  ["settings", "⚙️", "הגדרות"],
];

const SETTINGS_SCREENS: View[] = ["log", "categories", "currencies", "people"];

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
      {TABS.map(([view, ico, label]) => {
        const current = state.view === view || (view === "bags" && state.view === "bag") || (view === "settings" && SETTINGS_SCREENS.includes(state.view)) || (view === "month" && state.view === "business");
        return (
          <button key={view} aria-current={current ? "page" : undefined} onClick={() => openTab(view)}>
            <span className="ico">{ico}</span>
            {label}
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
  currencies: () => currenciesView({ back: () => goBack("settings") }),
  people: peopleView,
  bags: bagsView,
  bag: bagView,
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
      {/* While lines are being chosen, the bar at the bottom takes the place of the + button. */}
      {state.selecting === null && <button className="fab" title="הוספת תנועה" onClick={() => openTransactionDialog()}>+</button>}
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
  // Choosing lines only makes sense on the list.
  if (state.view !== "list") state.selecting = null;
  ensureHistory();
  flushSync(() => app.render(<App />));
  // A different screen opens at its top, not wherever the previous one was scrolled; re-rendering the same one keeps the position.
  const entering = renderedView !== state.view;
  if (entering) window.scrollTo(0, 0);
  renderedView = state.view;
  // Transactions open focused on the latest day that actually happened; upcoming days stay reachable above.
  if (entering && state.view === "list") document.getElementById("now-anchor")?.scrollIntoView({ block: "start" });
}
