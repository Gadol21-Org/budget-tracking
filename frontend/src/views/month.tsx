import { type ReactElement } from "react";
import { MonthBar } from "../components/MonthBar";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { openReactDialog } from "../components/openReactDialog";
import { PeriodForm } from "../dialogs/PeriodForm";
import { shift } from "../period";
import { businessOf, summaryFigures } from "../business";
import { go, goBack } from "../nav";
import { state } from "../state";
import { insightsCtx, render } from "../render";
import { DrillCrumbs, SummaryCategories } from "./insights";
import { categoryName } from "../categories";
import { inPeriod } from "../selectors";

/** The period switcher wired to the app's period. */
export function ConnectedMonthBar() {
  return (
    <MonthBar
      period={state.period}
      onShift={(delta) => {
        state.period = shift(state.period, delta) ?? state.period;
        render();
      }}
      onPick={openPeriodDialog}
    />
  );
}

export function openPeriodDialog() {
  openReactDialog((close) => (
    <PeriodForm
      current={state.period}
      close={close}
      onPick={(p) => {
        state.period = p;
        render();
      }}
    />
  ));
}

const sign = (n: number) => (n < 0 ? "expense" : "income");

/** "Of which the businesses": profit of the businesses in the period; they stay in the figures above. */
function BusinessCard() {
  if (!state.biz.businesses.length) return null;
  const { total, per } = summaryFigures({ txs: state.txs, cats: state.cats, biz: state.biz, period: state.period });
  if (!state.txs.some((t) => businessOf(state.biz, state.cats, t) != null)) return null;
  // Always opens the whole business; a single business is picked from there.
  const open = () => {
    state.bizView = { tab: "overview", scope: "all" };
    go("business");
  };
  return (
    <div className="card bizcard clickable" style={{ marginTop: 10 }} {...clickableProps(open)}>
      <div className="head">
        <div className="grow">
          <div className="bizname">מתוכם העסקים</div>
          <div className="muted small">כלול בסכומים למעלה</div>
        </div>
        <b>
          <span>רווח </span>
          <Money agorot={total.profit} cls={sign(total.profit)} />
        </b>
        <span className="muted">›</span>
      </div>
      {per.map(({ b, f }) => (
        <div className="row sub" key={b.id}>
          <div className="grow">{b.is_general ? `${b.name} (משותף)` : b.name}</div>
          <Money agorot={f.profit} cls={sign(f.profit)} />
        </div>
      ))}
    </div>
  );
}

/** The top bar of a category drill-down: back to the level above (the summary, or the category when a sub-category is open). */
function DrillBackBar() {
  const sub = state.insights.sub !== undefined;
  const label = sub ? categoryName(state.cats, state.insights.cat ?? null) : "סיכום";
  return (
    <div className="card filterbar">
      <button className="btn" title={sub ? "חזרה לקטגוריה" : "חזרה לסיכום"} onClick={() => goBack("month")}>
        {`→ ${label}`}
      </button>
      <div className="grow">
        <DrillCrumbs ctx={insightsCtx()} />
      </div>
    </div>
  );
}

function Stat({ label, agorot, cls, kind }: { label: string; agorot: number; cls: string; kind?: "expense" | "income" }) {
  const props = kind ? { ...clickableProps(() => ((state.kind = kind), render())), className: `card clickable${state.kind === kind ? " selected" : ""}` } : { className: "card" };
  return (
    <div {...props}>
      <div className="label">{label}</div>
      <div className="value">
        <Money agorot={agorot} cls={cls} />
      </div>
    </div>
  );
}

export function monthView(): ReactElement {
  // The category breakdown (and the drill-down into a category) comes from the insights code; the cards above choose its side.
  if (state.insights.cat !== undefined) state.kind = state.insights.side ?? state.kind;
  else state.insights.side = state.kind;
  return <MonthScreen />;
}

function MonthScreen() {
  if (state.insights.cat !== undefined) {
    return (
      <div>
        <DrillBackBar />
        <ConnectedMonthBar />
        <SummaryCategories ctx={insightsCtx()} />
      </div>
    );
  }
  const txs = inPeriod();
  const income = txs.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
  const expense = txs.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);
  const balance = income + expense;
  return (
    <div>
      <ConnectedMonthBar />
      <div className="stats">
        <Stat label="הכנסות" agorot={income} cls="income" kind="income" />
        <Stat label="הוצאות" agorot={-expense} cls="expense" kind="expense" />
        <Stat label="יתרה" agorot={balance} cls={sign(balance)} />
      </div>
      <BusinessCard />
      <SummaryCategories ctx={insightsCtx()} />
    </div>
  );
}
