import { el, fmtDate, type Child } from "../ui";
import { dequeue } from "../offline";
import { businessOf, bizName } from "../business";
import { api, formatMinor, formatRate, type Transaction } from "../api";
import { state } from "../state";
import { IN_FLIGHT, load } from "../sync";
import { catName, catPath, dayTitle, fxOf, inFilter, inPeriod, money } from "../selectors";
import { openTransactionDialog } from "../dialogs/transaction";
import { goBack } from "../nav";
import { render } from "../render";
import { monthBar } from "./month";

/** Saving / saved / failed marker of a line in Paamonim; nothing when mirroring is off. */
export function syncMark(t: Transaction): HTMLElement | null {
  if (t.sync_state === "queued") return el("span", { className: "sync", title: "ממתין לחיבור כדי להישלח", textContent: "📴" });
  if (!state.mirror || t.upcoming) return null;
  if (IN_FLIGHT.has(t.sync_state)) return el("span", { className: "sync-ring", title: "שומר בפעמונים…", role: "status", ariaLabel: "שומר בפעמונים" });
  if (t.sync_state === "error") return el("span", { className: "sync", title: "לא נשמר בפעמונים", textContent: "⚠️" });
  // Can't go to Paamonim as it is (a top-level category made here, a zero amount, a refund on an expense item).
  if (t.sync_state === "skipped") return el("span", { className: "sync", title: "לא נשלח לפעמונים: אין לו מקום שם", textContent: "⚠️" });
  if ((state.justSynced.get(t.id) ?? 0) > Date.now()) return el("span", { className: "sync", title: "נשמר בפעמונים", textContent: "✅" });
  return null;
}

/** Under the category: what the line was entered as, and the rate that gave its shekel amount. */
export function fxLine(t: Transaction): HTMLElement | null {
  const f = fxOf(t);
  if (!f) return null;
  const orig = formatMinor(Math.abs(f.orig_minor), f.currency);
  if (f.rate === 0) return el("div", { className: "fxline", title: "השער ייקבע כשהתנועה תישלח", textContent: `${orig} · ממתין לשער` });
  const stub = f.estimated;
  return el("div", { className: stub ? "fxline estimated" : "fxline", title: stub ? "שער זמני: יתעדכן לשער של היום שבו התנועה בוצעה" : `שער ליום ${fmtDate(f.rate_date)}`, textContent: `${orig} · שער ${formatRate(f.rate)}${stub ? " (זמני)" : ""}` });
}

export function listView(): HTMLElement {
  const txs = inPeriod()
    .filter(inFilter)
    .filter((t) => state.bizFilter === "all" || (businessOf(state.biz, state.cats, t) != null) === (state.bizFilter === "biz"));
  const days = [...new Set(txs.map((t) => t.date))].sort().reverse();
  // The first day (newest first) with something that really happened: the list opens here, with later, upcoming days above it.
  const nowDay = days.find((d) => txs.some((t) => t.date === d && !t.upcoming));
  const body: Child[] = days.map((d) =>
    el(
      "div",
      d === nowDay ? { id: "now-anchor", className: "now-anchor" } : {},
      d === nowDay && days[0] !== d ? el("div", { className: "upcoming-divider", textContent: "▲ למעלה: תנועות עתידיות" }) : null,
      el("div", { className: "daylabel", textContent: dayTitle(d) }),
      el(
        "div",
        { className: "card rows" },
        ...txs
          .filter((t) => t.date === d)
          .map((t) => {
            // An upcoming line has no row of its own to delete; stopping it is in its edit dialog.
            const del = t.upcoming ? null : el("button", { className: "del", title: "מחיקה", textContent: "✕" });
            if (del)
              del.onclick = async () => {
                if (!confirm("למחוק את התנועה?")) return;
                if (t.sync_state === "queued") await dequeue(-t.id);
                else await api.deleteTransaction(t.id);
                await load();
              };
            const edit = t.sync_state === "queued" ? null : el("button", { className: "del", title: "עריכה", textContent: "✎" });
            if (edit) edit.onclick = () => openTransactionDialog(t);
            const biz = businessOf(state.biz, state.cats, t);
            return el(
              "div",
              { className: t.upcoming ? "row upcoming" : "row" },
              el(
                "div",
                { className: "grow" },
                el(
                  "div",
                  { className: "name" },
                  t.upcoming ? el("span", { className: "upcoming-mark", title: "טרם בוצע: ניתן לשנות מכאן והלאה", textContent: "⏳ " }) : t.recurring_series_id != null ? el("span", { title: "הוצאה חוזרת", textContent: "🔁 " }) : null,
                  catPath(t.category_id),
                ),
                biz != null ? el("span", { className: t.id > 0 && state.biz.tx.has(t.id) ? "bizchip" : "bizchip auto", title: t.id > 0 && state.biz.tx.has(t.id) ? "סומנה בעסק" : "בעסק לפי הקטגוריה", textContent: bizName(state.biz, biz) }) : null,
                fxLine(t),
                t.note ? el("div", { className: "muted", textContent: t.note }) : null,
              ),
              syncMark(t),
              fxOf(t)?.rate === 0 ? el("span", { className: `num ${fxOf(t)!.orig_minor < 0 ? "expense" : "income"}`, textContent: formatMinor(Math.abs(fxOf(t)!.orig_minor), fxOf(t)!.currency) }) : money(t.amount_agorot, t.amount_agorot < 0 ? "expense" : "income"),
              edit,
              del,
            );
          }),
      ),
    ),
  );
  let header: Node | null = null;
  if (state.filter) {
    const back = el("button", { className: "btn", textContent: "→ סיכום", title: "חזרה לסיכום" });
    back.onclick = () => {
      goBack("month");
    };
    const total = txs.reduce((s, t) => s + t.amount_agorot, 0);
    header = el(
      "div",
      { className: "card filterbar" },
      back,
      el("div", { className: "grow" }, el("div", { className: "name", textContent: catName(state.filter.id) }), el("div", { className: "muted", textContent: `${txs.length} תנועות` })),
      money(total, total < 0 ? "expense" : "income"),
    );
  }
  const hasBiz = state.biz.businesses.length > 0;
  const bizSeg = hasBiz
    ? el(
        "div",
        { className: "seg3b" },
        ...(
          [
            ["all", "הכול"],
            ["biz", "רק עסק"],
            ["none", "ללא עסק"],
          ] as const
        ).map(([k, label]) => {
          const b = el("button", { type: "button", textContent: label });
          b.setAttribute("aria-pressed", String(state.bizFilter === k));
          b.onclick = () => ((state.bizFilter = k), render());
          return b;
        }),
      )
    : null;
  return el("div", {}, header, monthBar(), bizSeg, ...(body.length ? body : [el("div", { className: "empty", textContent: "אין תנועות בתקופה זו" })]));
}
