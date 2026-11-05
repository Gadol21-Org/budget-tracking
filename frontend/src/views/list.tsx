import { type ReactElement } from "react";
import { api, formatMinor, formatRate, type Transaction } from "../api";
import { bizName, businessOf } from "../business";
import { splitSumitNote } from "../sumitNote";
import { Money } from "../components/Money";
import { openTransactionDialog } from "../dialogs/transaction";
import { goBack } from "../nav";
import { dequeue } from "../offline";
import { render } from "../render";
import { catName, catPath, dayTitle, fxOf, inFilter, inPeriod } from "../selectors";
import { state } from "../state";
import { IN_FLIGHT, load } from "../sync";
import { fmtDate } from "../ui";
import { ConnectedMonthBar } from "./month";

/** Saving / saved / failed marker of a line in Paamonim; nothing when mirroring is off. */
function SyncMark({ t }: { t: Transaction }) {
  if (t.sync_state === "queued") return <span className="sync" title="ממתין לחיבור כדי להישלח">📴</span>;
  if (!state.mirror || t.upcoming) return null;
  if (IN_FLIGHT.has(t.sync_state)) return <span className="sync-ring" title="שומר בפעמונים…" role="status" aria-label="שומר בפעמונים" />;
  if (t.sync_state === "error") return <span className="sync" title="לא נשמר בפעמונים">⚠️</span>;
  // Can't go to Paamonim as it is (a top-level category made here, a zero amount, a refund on an expense item).
  if (t.sync_state === "skipped") return <span className="sync" title="לא נשלח לפעמונים: אין לו מקום שם">⚠️</span>;
  // The time decides whether the ✅ moment is over; trackSync renders again when it ends.
  // eslint-disable-next-line react-hooks/purity
  if ((state.justSynced.get(t.id) ?? 0) > Date.now()) return <span className="sync" title="נשמר בפעמונים">✅</span>;
  return null;
}

/** Under the category: what the line was entered as, and the rate that gave its shekel amount. */
function FxLine({ t }: { t: Transaction }) {
  const f = fxOf(t);
  if (!f) return null;
  const orig = formatMinor(Math.abs(f.orig_minor), f.currency);
  if (f.rate === 0) return <div className="fxline" title="השער ייקבע כשהתנועה תישלח">{`${orig} · ממתין לשער`}</div>;
  const stub = f.estimated;
  return (
    <div className={stub ? "fxline estimated" : "fxline"} title={stub ? "שער זמני: יתעדכן לשער של היום שבו התנועה בוצעה" : `שער ליום ${fmtDate(f.rate_date)}`}>
      {`${orig} · שער ${formatRate(f.rate)}${stub ? " (זמני)" : ""}`}
    </div>
  );
}

function TransactionRow({ t }: { t: Transaction }) {
  const biz = businessOf(state.biz, state.cats, t);
  const own = t.id > 0 && state.biz.tx.has(t.id);
  const fx = fxOf(t);
  const note = splitSumitNote(t.note);
  return (
    <div className={t.upcoming ? "row upcoming" : "row"}>
      <div className="grow">
        <div className="name">
          {t.upcoming ? <span className="upcoming-mark" title="טרם בוצע: ניתן לשנות מכאן והלאה">{"⏳ "}</span> : t.recurring_series_id != null ? <span title="הוצאה חוזרת">{"🔁 "}</span> : null}
          {catPath(t.category_id)}
        </div>
        {biz != null && (
          <span className={own ? "bizchip" : "bizchip auto"} title={own ? "סומנה בעסק" : "בעסק לפי הקטגוריה"}>
            {bizName(state.biz, biz)}
          </span>
        )}
        {note.products.map((p) => (
          <span className="bizchip prod" key={p} title="מוצר בקבלה מ-Sumit">
            {p}
          </span>
        ))}
        <FxLine t={t} />
        {note.text ? <div className="muted">{note.text}</div> : null}
      </div>
      <SyncMark t={t} />
      {fx?.rate === 0 ? (
        <span className={`num ${fx.orig_minor < 0 ? "expense" : "income"}`}>{formatMinor(Math.abs(fx.orig_minor), fx.currency)}</span>
      ) : (
        <Money agorot={t.amount_agorot} cls={t.amount_agorot < 0 ? "expense" : "income"} />
      )}
      {t.sync_state === "queued" ? null : (
        <button className="del" title="עריכה" onClick={() => openTransactionDialog(t)}>
          ✎
        </button>
      )}
      {/* An upcoming line has no row of its own to delete; stopping it is in its edit dialog. */}
      {t.upcoming ? null : (
        <button
          className="del"
          title="מחיקה"
          onClick={async () => {
            if (!confirm("למחוק את התנועה?")) return;
            if (t.sync_state === "queued") await dequeue(-t.id);
            else await api.deleteTransaction(t.id);
            await load();
          }}
        >
          ✕
        </button>
      )}
    </div>
  );
}

const BIZ_FILTERS = [
  ["all", "הכול"],
  ["biz", "רק עסק"],
  ["none", "ללא עסק"],
] as const;

export function listView(): ReactElement {
  return <ListScreen />;
}

function ListScreen() {
  const txs = inPeriod()
    .filter(inFilter)
    .filter((t) => state.bizFilter === "all" || (businessOf(state.biz, state.cats, t) != null) === (state.bizFilter === "biz"));
  const days = [...new Set(txs.map((t) => t.date))].sort().reverse();
  // The first day (newest first) with something that really happened: the list opens here, with later, upcoming days above it.
  const nowDay = days.find((d) => txs.some((t) => t.date === d && !t.upcoming));
  const total = txs.reduce((s, t) => s + t.amount_agorot, 0);
  return (
    <div>
      {state.filter && (
        <div className="card filterbar">
          <button className="btn" title="חזרה לסיכום" onClick={() => goBack("month")}>
            → סיכום
          </button>
          <div className="grow">
            <div className="name">{catName(state.filter.id)}</div>
            <div className="muted">{`${txs.length} תנועות`}</div>
          </div>
          <Money agorot={total} cls={total < 0 ? "expense" : "income"} />
        </div>
      )}
      <ConnectedMonthBar />
      {state.biz.businesses.length > 0 && (
        <div className="seg3b">
          {BIZ_FILTERS.map(([k, label]) => (
            <button key={k} type="button" aria-pressed={state.bizFilter === k} onClick={() => ((state.bizFilter = k), render())}>
              {label}
            </button>
          ))}
        </div>
      )}
      {days.length ? (
        days.map((d) => (
          <div key={d} id={d === nowDay ? "now-anchor" : undefined} className={d === nowDay ? "now-anchor" : undefined}>
            {d === nowDay && days[0] !== d ? <div className="upcoming-divider">▲ למעלה: תנועות עתידיות</div> : null}
            <div className="daylabel">{dayTitle(d)}</div>
            <div className="card rows">
              {txs
                .filter((t) => t.date === d)
                .map((t, i) => (
                  <TransactionRow key={i} t={t} />
                ))}
            </div>
          </div>
        ))
      ) : (
        <div className="empty">אין תנועות בתקופה זו</div>
      )}
    </div>
  );
}
