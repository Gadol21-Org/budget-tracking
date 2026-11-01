import { formatShekels } from "../format";
import { el, fmtDate } from "../ui";
import { api, type LogEntry } from "../api";
import { go, goBack } from "../nav";
import { state } from "../state";
import { render } from "../render";

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
export function logLines(e: LogEntry): HTMLElement[] {
  const line = (key: string, text: string) =>
    el("div", { className: "logline" }, el("span", { className: "muted", textContent: `${LOG_FIELDS[key] ?? key}: ` }), el("span", { textContent: text }));
  const values = (o: Record<string, unknown> | null) =>
    Object.entries(o ?? {})
      .filter(([, v]) => v != null && v !== "" && !(Array.isArray(v) && !v.length))
      .map(([k, v]) => line(k, logValue(k, v)));
  if (e.old && e.new && ["transaction_update", "recurring_update", "recurring_override", "category_link", "category_move"].includes(e.kind)) {
    const keys = Object.keys({ ...e.old, ...e.new }).filter((k) => JSON.stringify(e.old![k] ?? null) !== JSON.stringify(e.new![k] ?? null));
    return keys.map((k) => line(k, `${logValue(k, e.old![k])} ← ${logValue(k, e.new![k])}`));
  }
  return [...values(e.old), ...values(e.new)];
}

export function logEntryRow(e: LogEntry): HTMLElement {
  const [ico, label] = LOG_KINDS[e.kind] ?? ["•", e.kind];
  const snap = (e.new ?? e.old) as Record<string, unknown> | null;
  const what = e.kind.startsWith("category_") ? (snap?.name as string | undefined) : (snap?.category as string | null | undefined) ?? (e.kind.startsWith("transaction_") || e.kind.startsWith("recurring_") ? "ללא קטגוריה" : undefined);
  const time = new Date(e.at).toLocaleTimeString("he-IL", { hour: "2-digit", minute: "2-digit" });
  return el(
    "div",
    { className: "row logentry" },
    el("span", { className: "logico", textContent: ico }),
    el(
      "div",
      { className: "grow" },
      el("div", { className: "logtitle" }, label, what ? el("span", { className: "muted", textContent: ` · ${what}` }) : null),
      ...logLines(e),
    ),
    el("span", { className: "muted logtime", textContent: time }),
  );
}

export function logView(): HTMLElement {
  const back = el("button", { className: "btn", textContent: "→ הגדרות", title: "חזרה להגדרות" });
  back.onclick = () => {
    goBack("settings");
  };
  const localDay = (at: string) => new Date(at).toLocaleDateString("sv-SE");
  const days = [...new Set(state.log.map((e) => localDay(e.at)))];
  const body = days.map((d) =>
    el(
      "div",
      {},
      el("div", { className: "daylabel", textContent: new Date(`${d}T00:00:00`).toLocaleDateString("he-IL", { weekday: "long", day: "numeric", month: "long", year: "numeric" }) }),
      el("div", { className: "card rows" }, ...state.log.filter((e) => localDay(e.at) === d).map(logEntryRow)),
    ),
  );
  const more = el("button", { className: "btn", textContent: "הצגת שינויים ישנים יותר" });
  more.onclick = async () => {
    more.disabled = true;
    try {
      const older = await api.log(state.log[state.log.length - 1]?.id, LOG_PAGE);
      state.log = [...state.log, ...older];
      state.logMore = older.length === LOG_PAGE;
      render();
    } catch {
      more.disabled = false;
    }
  };
  return el(
    "div",
    {},
    el("div", { className: "card filterbar" }, back, el("div", { className: "grow" }, el("div", { className: "name", textContent: "היסטוריית שינויים" }), el("div", { className: "muted", textContent: "שנה אחרונה" }))),
    ...(body.length ? body : [el("div", { className: "empty", textContent: "עוד לא נרשמו שינויים" })]),
    state.logMore ? el("div", { className: "actions", style: "justify-content:center;margin-top:12px" }, more) : null,
  );
}

export async function openLog() {
  state.log = await api.log(undefined, LOG_PAGE);
  state.logMore = state.log.length === LOG_PAGE;
  go("log");
}
