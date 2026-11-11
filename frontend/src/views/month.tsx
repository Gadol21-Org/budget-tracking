import { type ReactElement } from "react";
import { MonthBar } from "../components/MonthBar";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { openReactDialog } from "../components/openReactDialog";
import { PeriodForm } from "../dialogs/PeriodForm";
import { shift } from "../period";
import { goBack, showSide } from "../nav";
import { state } from "../state";
import { insightsCtx, render } from "../render";
import { openEmailInbox } from "./emailInbox";
import { DrillCrumbs, SummaryCategories } from "./insights";
import { categoryName, isIncomeTx } from "../categories";
import { inPeriod } from "../selectors";
import { PushOffer, SumitBanner } from "./sumit";

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

/** On the summary: charges found in emails that wait for a check (same banner as the Sumit receipts). */
function EmailBanner() {
  const n = state.email?.pending ?? 0;
  if (!n) return null;
  return (
    <div className="sumit-banner warn" {...clickableProps(() => void openEmailInbox().catch(() => alert("טעינת ההצעות נכשלה")))}>
      {n === 1 ? "חיוב אחד מדוא״ל ממתין לבדיקה" : `${n} חיובים מדוא״ל ממתינים לבדיקה`}
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
  const props = kind ? { ...clickableProps(() => showSide(kind)), className: `card clickable${state.kind === kind ? " selected" : ""}` } : { className: "card" };
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
  // By category, not by sign: a refund is a negative income and lowers the income.
  const income = txs.filter((t) => isIncomeTx(state.cats, t)).reduce((s, t) => s + t.amount_agorot, 0);
  const expense = txs.filter((t) => !isIncomeTx(state.cats, t)).reduce((s, t) => s + t.amount_agorot, 0);
  const balance = income + expense;
  return (
    <div>
      <ConnectedMonthBar />
      <SumitBanner />
      <PushOffer />
      <div className="stats">
        <Stat label="הכנסות" agorot={income} cls="income" kind="income" />
        <Stat label="הוצאות" agorot={-expense} cls="expense" kind="expense" />
        <Stat label="יתרה" agorot={balance} cls={sign(balance)} />
      </div>
      <EmailBanner />
      <SummaryCategories ctx={insightsCtx()} />
    </div>
  );
}
