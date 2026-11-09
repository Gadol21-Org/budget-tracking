import { formatShekels } from "../format";
import { fmtDate } from "../ui";
import { api, type LogEntry } from "../api";
import { go, goBack } from "../nav";
import { state } from "../state";
import { render } from "../render";
import { type ReactElement, useState } from "react";
import { BackButton } from "../components/BackButton";
import { MonthBar } from "../components/MonthBar";
import { openReactDialog } from "../components/openReactDialog";
import { PeriodForm } from "../dialogs/PeriodForm";
import { contains, bounds, monthOf, shift, type Period } from "../period";

export const LOG_PAGE = 100;

/** Icon and Hebrew name of each kind of change. */
export const LOG_KINDS: Record<string, [string, string]> = {
  transaction_create: ["➕", "תנועה חדשה"],
  transaction_update: ["✎", "תנועה נערכה"],
  transaction_delete: ["🗑", "תנועה נמחקה"],
  category_create: ["➕", "קטגוריה חדשה"],
  category_delete: ["🗑", "קטגוריה נמחקה"],
  category_link: ["🔗", "קטגוריה קושרה לפעמונים"],
  category_move: ["⇄", "תת-קטגוריה הועברה"],
  recurring_create: ["🔁", "הוצאה חוזרת חדשה"],
  recurring_update: ["🔁", "הוצאה חוזרת עודכנה"],
  recurring_cancel: ["⏹", "הוצאה חוזרת בוטלה"],
  recurring_override: ["✎", "חודש אחד בהוצאה חוזרת נערך"],
  recurring_skip: ["⏭", "חודש אחד בהוצאה חוזרת דולג"],
  recurring_settle: ["✅", "חיוב חוזר בוצע"],
  import: ["⬇", "ייבוא מפעמונים"],
};

export const LOG_FIELDS: Record<string, string> = {
  date: "תאריך",
  amount_agorot: "סכום",
  category: "קטגוריה",
  payment_method: "אמצעי תשלום",
  note: "הערה",
  start_month: "מחודש",
  end_month: "עד חודש",
  day: "יום בחודש",
  name: "שם",
  parent: "תחת",
  linked_to: "בפעמונים",
  is_income: "סוג",
  subcategories: "תתי-קטגוריות",
  moved_to: "התנועות הועברו אל",
  transactions_moved: "תנועות שהועברו",
  from_month: "בוטלה מחודש",
  categories_added: "קטגוריות חדשות",
  payment_methods_added: "אמצעי תשלום חדשים",
  transactions_added: "תנועות חדשות",
  transactions_skipped: "תנועות שכבר היו קיימות",
  transactions_adopted: "תנועות שקושרו",
};

export const monthLabel = (m: string) => `${m.slice(5)}/${m.slice(0, 4)}`;

export function logValue(key: string, v: unknown): string {
  if (v == null || v === "" || (Array.isArray(v) && !v.length)) return key === "end_month" ? "ללא סיום" : "—";
  if (key === "amount_agorot") return formatShekels(v as number);
  if (key === "date") return fmtDate(String(v));
  if (key.endsWith("_month")) return monthLabel(v as string);
  if (key === "is_income") return v ? "הכנסה" : "הוצאה";
  if (Array.isArray(v)) return v.join(", ");
  return String(v);
}

/** The lines under a log entry: what changed (old ← new) for an edit, otherwise the values recorded. */
function LogLines({ e }: { e: LogEntry }) {
  const line = (key: string, text: string, id: string | number = key) => (
    <div className="logline" key={id}>
      <span className="muted">{`${LOG_FIELDS[key] ?? key}: `}</span>
      <span>{text}</span>
    </div>
  );
  const values = (o: Record<string, unknown> | null) =>
    Object.entries(o ?? {})
      .filter(([, v]) => v != null && v !== "" && !(Array.isArray(v) && !v.length))
      .map(([k, v], i) => line(k, logValue(k, v), `${k}${i}`));
  if (e.old && e.new && ["transaction_update", "recurring_update", "recurring_override", "category_link", "category_move"].includes(e.kind)) {
    const keys = Object.keys({ ...e.old, ...e.new }).filter((k) => JSON.stringify(e.old![k] ?? null) !== JSON.stringify(e.new![k] ?? null));
    return <>{keys.map((k) => line(k, `${logValue(k, e.old![k])} ← ${logValue(k, e.new![k])}`))}</>;
  }
  return (
    <>
      {values(e.old)}
      {values(e.new)}
    </>
  );
}

/** The display name given to an email, else the email itself. */
export const userLabel = (email: string) => (email === SYNC_ACTOR ? "Sumit Sync" : state.logUsers.find((u) => u.email === email)?.name || email);

/** The author the server records for changes made by the Sumit sync (not a person). */
const SYNC_ACTOR = "sumit-sync";

function LogEntryRow({ e }: { e: LogEntry }) {
  const [ico, label] = LOG_KINDS[e.kind] ?? ["•", e.kind];
  const snap = (e.new ?? e.old) as Record<string, unknown> | null;
  const what = e.kind.startsWith("category_") ? (snap?.name as string | undefined) : (snap?.category as string | null | undefined) ?? (e.kind.startsWith("transaction_") || e.kind.startsWith("recurring_") ? "ללא קטגוריה" : undefined);
  const time = new Date(e.at).toLocaleTimeString("he-IL", { hour: "2-digit", minute: "2-digit" });
  return (
    <div className="row logentry">
      <span className="logico">{ico}</span>
      <div className="grow">
        <div className="logtitle">
          {label}
          {what ? <span className="muted">{` · ${what}`}</span> : null}
        </div>
        {e.user_email ? <div className="logline muted">{`${e.user_email === SYNC_ACTOR ? "🏢" : "👤"} ${userLabel(e.user_email)}`}</div> : null}
        <LogLines e={e} />
      </div>
      <span className="muted logtime">{time}</span>
    </div>
  );
}

export function logView(): ReactElement {
  return <LogScreen />;
}

const localDay = (at: string) => new Date(at).toLocaleDateString("sv-SE");

/** Whether the loaded entries reach back to the start of the period (or there is nothing older). */
const covers = (p: Period) => {
  const last = state.log[state.log.length - 1];
  return !state.logMore || (last != null && localDay(last.at) < bounds(p).from);
};

/** Load older pages until the period's first day is covered. Returns false if a page failed to load. */
export async function loadLogFor(p: Period): Promise<boolean> {
  try {
    while (!covers(p)) {
      const older = await api.log(state.log[state.log.length - 1]?.id, LOG_PAGE);
      state.log = [...state.log, ...older];
      state.logMore = older.length === LOG_PAGE;
    }
    return true;
  } catch {
    return false;
  }
}

function LogScreen() {
  const [loadingMore, setLoadingMore] = useState(false);
  const period = state.logPeriod;
  const shown = state.log.filter((e) => contains(period, localDay(e.at)));
  const days = [...new Set(shown.map((e) => localDay(e.at)))];
  const choose = async (p: Period) => {
    state.logPeriod = p;
    render();
    setLoadingMore(true);
    await loadLogFor(p);
    setLoadingMore(false);
    render();
  };
  const more = async () => {
    setLoadingMore(true);
    await loadLogFor(period);
    setLoadingMore(false);
    render();
  };
  return (
    <div>
      <div className="card filterbar">
        <BackButton onClick={() => goBack("settings")} />
        <div className="grow">
          <div className="name">היסטוריית שינויים</div>
          <div className="muted">נשמרת שנה אחרונה</div>
        </div>
      </div>
      <MonthBar
        period={period}
        onShift={(delta) => void choose(shift(period, delta) ?? period)}
        onPick={() => openReactDialog((close) => <PeriodForm current={period} close={close} onPick={(p) => void choose(p)} />)}
      />
      {days.length ? (
        days.map((d) => (
          <div key={d}>
            <div className="daylabel">{new Date(`${d}T00:00:00`).toLocaleDateString("he-IL", { weekday: "long", day: "numeric", month: "long", year: "numeric" })}</div>
            <div className="card rows">
              {shown
                .filter((e) => localDay(e.at) === d)
                .map((e) => (
                  <LogEntryRow key={e.id} e={e} />
                ))}
            </div>
          </div>
        ))
      ) : (
        <div className="empty">{loadingMore ? "טוען…" : "לא נרשמו שינויים בתקופה זו"}</div>
      )}
      {!covers(period) ? (
        <div className="actions" style={{ justifyContent: "center", marginTop: 12 }}>
          <button className="btn" disabled={loadingMore} onClick={more}>
            הצגת שינויים ישנים יותר
          </button>
        </div>
      ) : null}
    </div>
  );
}

export async function openLog() {
  state.logUsers = await api.logUsers().catch(() => []);
  state.log = await api.log(undefined, LOG_PAGE);
  state.logMore = state.log.length === LOG_PAGE;
  state.logPeriod = monthOf(new Date().toLocaleDateString("sv-SE"));
  await loadLogFor(state.logPeriod);
  go("log");
}
