import { useState } from "react";
import { DateField } from "../components/DateField";
import { MONTHS, type Period } from "../period";
import { state } from "../state";

const KIND_LABELS = { month: "חודש", year: "שנה", range: "טווח" } as const;

/** Pick what the screens show: a month, a year or a range of dates. */
export function PeriodForm({ current, onPick, close }: { current: Period; onPick: (p: Period) => void; close: () => void }) {
  const thisYear = new Date().getFullYear();
  const dataYears = state.txs.map((t) => Number(t.date.slice(0, 4)));
  // Older years are not loaded until visited, so the picker always offers a decade back.
  const minYear = Math.min(thisYear - 10, ...dataYears);
  const maxYear = Math.max(thisYear, ...dataYears);
  const years = Array.from({ length: maxYear - minYear + 1 }, (_, i) => maxYear - i);
  const today = new Date().toISOString().slice(0, 10);
  const curMonth = current.kind === "month" ? current.month : today.slice(0, 7);

  const [kind, setKind] = useState<Period["kind"]>(current.kind);
  const [month, setMonth] = useState(String(Number(curMonth.slice(5))));
  const [monthYear, setMonthYear] = useState(String(Number(curMonth.slice(0, 4))));
  const [year, setYear] = useState(String(current.kind === "year" ? current.year : thisYear));
  const [from, setFrom] = useState(current.kind === "range" ? current.from : `${today.slice(0, 7)}-01`);
  const [to, setTo] = useState(current.kind === "range" ? current.to : today);
  const [err, setErr] = useState("");

  const yearSelect = (value: string, set: (v: string) => void) => (
    <select value={value} onChange={(e) => set(e.target.value)}>
      {years.map((y) => (
        <option key={y} value={String(y)}>
          {y}
        </option>
      ))}
    </select>
  );

  return (
    <form
      className="add"
      method="dialog"
      onSubmit={(e) => {
        e.preventDefault();
        if (kind === "month") onPick({ kind, month: `${monthYear}-${month.padStart(2, "0")}` });
        else if (kind === "year") onPick({ kind, year: Number(year) });
        else {
          if (!from || !to || from > to) return setErr("טווח התאריכים אינו תקין");
          onPick({ kind, from, to });
        }
        close();
      }}
    >
      <div className="seg3">
        {(["month", "year", "range"] as const).map((k) => (
          <button key={k} type="button" aria-pressed={k === kind} onClick={() => setKind(k)}>
            {KIND_LABELS[k]}
          </button>
        ))}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        {kind === "month" ? (
          <>
            <label>
              חודש
              <select value={month} onChange={(e) => setMonth(e.target.value)}>
                {MONTHS.map((n, i) => (
                  <option key={n} value={String(i + 1)}>
                    {n}
                  </option>
                ))}
              </select>
            </label>
            <label>
              שנה
              {yearSelect(monthYear, setMonthYear)}
            </label>
          </>
        ) : kind === "year" ? (
          <label>
            שנה
            {yearSelect(year, setYear)}
          </label>
        ) : (
          <>
            <label>
              מתאריך
              <DateField value={from} onChange={setFrom} />
            </label>
            <label>
              עד תאריך
              <DateField value={to} onChange={setTo} />
            </label>
          </>
        )}
      </div>
      <div className="error">{err}</div>
      <div className="actions">
        <button type="button" className="btn" onClick={close}>
          ביטול
        </button>
        <button type="submit" className="btn primary">
          הצגה
        </button>
      </div>
    </form>
  );
}
