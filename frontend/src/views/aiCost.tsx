import { type ReactElement, useState } from "react";
import { api } from "../api";
import { BackButton } from "../components/BackButton";
import { MonthBar } from "../components/MonthBar";
import { openReactDialog } from "../components/openReactDialog";
import { PeriodForm } from "../dialogs/PeriodForm";
import { aiResultText, formatUsd } from "../format";
import { go, goBack } from "../nav";
import { bounds, monthOf, shift, type Period } from "../period";
import { render } from "../render";
import { state } from "../state";

const lastDay = (month: string) => {
  const [y, m] = month.split("-").map(Number);
  return new Date(Date.UTC(y, m, 0)).toISOString().slice(0, 10);
};

async function load(p: Period) {
  const { from, to } = bounds(p);
  // The period is in local days and the log is stamped in UTC, so a day's edge can be off by the time zone: fine for a cost overview.
  state.aiCost = await api.aiCost(from, p.kind === "month" ? lastDay(p.month) : to);
}

/** What the AI calls that read emails cost: each call with its price, and the total for the chosen period. */
export function aiCostView(): ReactElement {
  return <AiCostScreen />;
}

function AiCostScreen() {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const period = state.aiCostPeriod;
  const data = state.aiCost;
  const choose = async (p: Period) => {
    state.aiCostPeriod = p;
    setBusy(true);
    setErr("");
    try {
      await load(p);
    } catch {
      setErr("טעינת העלות נכשלה");
    }
    setBusy(false);
    render();
  };
  const days = [...new Set((data?.items ?? []).map((i) => new Date(i.at).toLocaleDateString("sv-SE")))];
  return (
    <div>
      <div className="card filterbar">
        <BackButton onClick={() => goBack("settings")} />
        <div className="grow">
          <div className="name">עלות AI</div>
          <div className="muted">קריאת מיילים ב-AI, לפי מחירי הטוקנים שהוגדרו בשרת</div>
        </div>
      </div>
      <MonthBar
        period={period}
        onShift={(delta) => void choose(shift(period, delta) ?? period)}
        onPick={() => openReactDialog((close) => <PeriodForm current={period} close={close} onPick={(p) => void choose(p)} />)}
      />
      <div className="card aicost-total">
        <div className="muted">סך הכול לתקופה</div>
        <div className="aicost-sum" dir="ltr">{formatUsd(data?.total_micro_usd ?? 0)}</div>
        <div className="muted small">{`${data?.items.length ?? 0} קריאות`}</div>
      </div>
      {err && <div className="note error">{err}</div>}
      {data && data.items.length === 0 && <div className="empty">{busy ? "טוען…" : "לא נקראו מיילים בתקופה זו"}</div>}
      {days.map((d) => (
        <div key={d}>
          <div className="daylabel">{new Date(`${d}T00:00:00`).toLocaleDateString("he-IL", { weekday: "long", day: "numeric", month: "long", year: "numeric" })}</div>
          <div className="card rows">
            {(data?.items ?? [])
              .filter((i) => new Date(i.at).toLocaleDateString("sv-SE") === d)
              .map((i, k) => (
                <div className="row aicost-row" key={`${i.at}${k}`}>
                  <div className="grow" dir="auto">
                    <div className="logtitle">{i.subject || i.sender}</div>
                    <div className="muted small">{aiResultText(i.result)}</div>
                  </div>
                  <b dir="ltr">{formatUsd(i.cost_micro_usd)}</b>
                </div>
              ))}
          </div>
        </div>
      ))}
    </div>
  );
}

export async function openAiCost() {
  state.aiCostPeriod = monthOf(new Date().toLocaleDateString("sv-SE"));
  await load(state.aiCostPeriod);
  go("aiCost");
}
