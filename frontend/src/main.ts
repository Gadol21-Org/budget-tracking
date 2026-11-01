import "./style.css";
import { clickable, dateField, el, fmtDate, type Child } from "./ui";
import { insightsView, type Cmp, type InsightsNav, type InsightsState } from "./insights";
import { contains, MONTHS, monthOf, shift, title as periodTitle, type Period } from "./period";
import { dequeue, enqueue, isNetworkError, loadSnapshot, queuedList, requestPersistence, saveSnapshot, type Queued, type Snapshot } from "./offline";
import { businessChips, businessDashboard, businessesView, businessOf, businessRequired, bizName, categoryBusiness, emptyBiz, openCategoryBusinessDialog, openTagDialog, summaryFigures, toBizData, type BizData, type BizView } from "./business";
import { api, CURRENCIES, currencySymbol, decimals, formatMinor, formatRate, formatShekels, parseAgorot, parseMinor, type Category, type Currency, type Fx, type LogEntry, type RecurringSegment, type Transaction, type TxInput } from "./api";

type View = "month" | "list" | "insights" | "settings" | "log" | "categories" | "business" | "businesses";

const root = document.getElementById("app")!;
const state = {
  view: "month" as View,
  period: monthOf(new Date().toISOString().slice(0, 10)) as Period,
  /** Stored transactions plus the generated occurrences of repeating ones. */
  txs: [] as Transaction[],
  recurring: [] as RecurringSegment[],
  cats: [] as Category[],
  /** Set when a summary row was opened: the list shows only that category (and its sub-categories). */
  /** Which side of the summary's category breakdown is shown. */
  kind: "expense" as "expense" | "income",
  /** Which side of the category manager in settings is shown (true = income). */
  catKind: false,
  filter: undefined as { id: number | null } | undefined,
  /** The insights tab: open dashboard, drill-down position and what the period is compared with. */
  insights: { dash: "overview", cmp: { kind: "prev" } } as InsightsState,
  /** Change log entries loaded so far (newest first) and whether older ones remain. */
  log: [] as LogEntry[],
  logMore: false,
  /** Changes are written to Paamonim (PAAMONIM_MIRROR=1). */
  mirror: false,
  /** sync_state of each transaction at the last load, to notice lines that just got saved. */
  syncSeen: new Map<number, string>(),
  /** Lines just saved in Paamonim, shown with ✅ until this time (ms). */
  justSynced: new Map<number, number>(),
  /** The businesses, and which transactions and categories count in them. */
  biz: emptyBiz() as BizData,
  /** The business dashboards: open tab and which business they show. */
  bizView: { tab: "overview", scope: "all" } as BizView,
  /** The transaction list: all, only business lines or only the rest. */
  bizFilter: "all" as "all" | "biz" | "none",
  /** Choosing several lines to tag them as a business's. */
  selecting: false,
  selected: new Set<number>(),
  /** Transactions entered in USD or JPY, by transaction id (queued lines by -queue id): original amount and rate. */
  fx: new Map<number, Fx>(),
  /** The server could not be reached: the last saved data is shown, and new lines wait in the queue. */
  offline: false,
};

/** Browser history: every screen is a history entry, so the phone's back button/gesture goes to the previous screen (and closes a dialog first) instead of leaving the app. */
type Screen = { view: View; filter?: { id: number | null }; ins?: InsightsNav; dlg?: boolean; n: number };
function screenState(extra: Partial<Screen> = {}): Screen {
  return { view: state.view, filter: state.filter, ins: { dash: state.insights.dash, side: state.insights.side, cat: state.insights.cat, sub: state.insights.sub }, n: history.state?.n ?? 0, ...extra };
}
history.replaceState(screenState({ n: 0 }), "");
/** Go to a screen as a new history entry. */
function go(view: View, filter?: { id: number | null }) {
  if (view === state.view && filter?.id === state.filter?.id && (filter === undefined) === (state.filter === undefined)) return;
  state.view = view;
  state.filter = filter;
  history.pushState(screenState({ n: (history.state?.n ?? 0) + 1 }), "");
  render();
}
/** The in-app back buttons: the same as the phone's back, or a fresh step when there is nothing to go back to. */
function goBack(view: View) {
  if ((history.state?.n ?? 0) > 0 && !history.state?.dlg) history.back();
  else {
    state.view = view;
    state.filter = undefined;
    render();
  }
}
/** A modal takes a history entry while open: back closes it, and closing it any other way drops the entry. */
function trackDialog(dlg: HTMLDialogElement) {
  history.pushState(screenState({ dlg: true }), "");
  dlg.addEventListener("close", () => {
    if (!dlg.dataset.viaBack && history.state?.dlg) history.back();
  });
}
window.addEventListener("popstate", (e) => {
  const open = [...document.querySelectorAll<HTMLDialogElement>("dialog[open]")];
  const top = open[open.length - 1];
  if (top) {
    top.dataset.viaBack = "1";
    top.close();
    return;
  }
  const st = e.state as Screen | null;
  if (st?.dlg) return;
  state.view = st?.view ?? "month";
  state.filter = st?.filter;
  restoreInsights(st?.ins);
  render();
});

function restoreInsights(ins?: InsightsNav) {
  state.insights.dash = ins?.dash ?? "overview";
  state.insights.side = ins?.side;
  state.insights.cat = ins?.cat;
  state.insights.sub = ins?.sub;
}
/** Move inside the insights tab. A drill-down step is a history entry (back goes up a level); switching dashboards replaces it. */
function navigateInsights(patch: Partial<InsightsNav>, mode: "push" | "replace") {
  Object.assign(state.insights, { cat: undefined, sub: undefined }, patch);
  const st = screenState(mode === "push" ? { n: (history.state?.n ?? 0) + 1 } : {});
  if (mode === "push") history.pushState(st, "");
  else history.replaceState(st, "");
  window.scrollTo(0, 0);
  render();
}

/** States of a line whose change is on its way to Paamonim. */
const IN_FLIGHT = new Set(["pending", "modified", "verifying"]);
const SYNCED_SHOWN_MS = 2500;

/** Saving / saved / failed marker of a line in Paamonim; nothing when mirroring is off. */
function syncMark(t: Transaction): HTMLElement | null {
  if (t.sync_state === "queued") return el("span", { className: "sync", title: "ממתין לחיבור כדי להישלח", textContent: "📴" });
  if (!state.mirror || t.upcoming) return null;
  if (IN_FLIGHT.has(t.sync_state)) return el("span", { className: "sync-ring", title: "שומר בפעמונים…", role: "status", ariaLabel: "שומר בפעמונים" });
  if (t.sync_state === "error") return el("span", { className: "sync", title: "לא נשמר בפעמונים", textContent: "⚠️" });
  // Can't go to Paamonim as it is (a top-level category made here, a zero amount, a refund on an expense item).
  if (t.sync_state === "skipped") return el("span", { className: "sync", title: "לא נשלח לפעמונים: אין לו מקום שם", textContent: "⚠️" });
  if ((state.justSynced.get(t.id) ?? 0) > Date.now()) return el("span", { className: "sync", title: "נשמר בפעמונים", textContent: "✅" });
  return null;
}

/** The foreign-currency details of a line, if it was entered in USD or JPY. */
const fxOf = (t: Transaction): Fx | undefined => (t.upcoming ? undefined : state.fx.get(t.id));
/** Under the category: what the line was entered as, and the rate that gave its shekel amount. */
function fxLine(t: Transaction): HTMLElement | null {
  const f = fxOf(t);
  if (!f) return null;
  const orig = formatMinor(Math.abs(f.orig_minor), f.currency);
  if (f.rate === 0) return el("div", { className: "fxline", title: "השער ייקבע כשהתנועה תישלח", textContent: `${orig} · ממתין לשער` });
  const stub = f.estimated;
  return el("div", { className: stub ? "fxline estimated" : "fxline", title: stub ? "שער זמני: יתעדכן לשער של היום שבו התנועה בוצעה" : `שער ליום ${fmtDate(f.rate_date)}`, textContent: `${orig} · שער ${formatRate(f.rate)}${stub ? " (זמני)" : ""}` });
}
const money = (agorot: number, cls = "") => el("span", { className: `num ${cls}`.trim(), textContent: formatShekels(agorot) });
const catById = (id: number | null) => state.cats.find((c) => c.id === id);
const catName = (id: number | null) => catById(id)?.name ?? "ללא קטגוריה";
/** "parent › child" for sub-categories. */
const catPath = (id: number | null) => {
  const c = catById(id);
  const p = c?.parent_id != null ? catById(c.parent_id) : undefined;
  return p && c ? `${p.name} › ${c.name}` : catName(id);
};

const dayTitle = (d: string) =>
  new Date(`${d}T00:00:00`).toLocaleDateString("he-IL", { weekday: "long", day: "numeric", month: "long" });

/** After saving, keep the period the user is viewing; switch to the item's month only if it would otherwise be out of view. */
function showPeriodFor(date: string) {
  if (!contains(state.period, date)) state.period = monthOf(date);
}

const inPeriod = () => state.txs.filter((t) => contains(state.period, t.date));

function inFilter(t: Transaction): boolean {
  if (!state.filter) return true;
  const id = state.filter.id;
  return t.category_id === id || (id !== null && catById(t.category_id)?.parent_id === id);
}

function showCategory(id: number | null) {
  go("list", { id });
}

function monthBar(): HTMLElement {
  // RTL: the first element is on the right and goes back in time, the last (left) goes forward.
  const prev = el("button", { className: "iconbtn", title: "הקודם", textContent: "→" });
  const next = el("button", { className: "iconbtn", title: "הבא", textContent: "←" });
  const go = (delta: number) => () => {
    state.period = shift(state.period, delta) ?? state.period;
    render();
  };
  prev.onclick = go(-1);
  next.onclick = go(1);
  if (state.period.kind === "range") {
    // A custom range has no neighbours; keep the layout steady.
    prev.style.visibility = next.style.visibility = "hidden";
  }
  const label = el("button", { className: "title periodbtn", title: "בחירת תקופה", textContent: `${periodTitle(state.period)} ▾` });
  label.onclick = openPeriodDialog;
  return el("div", { className: "monthbar" }, prev, label, next);
}

function openPeriodDialog() {
  const p = state.period;
  let kind: Period["kind"] = p.kind;
  const dlg = el("dialog");
  const thisYear = new Date().getFullYear();
  const dataYears = state.txs.map((t) => Number(t.date.slice(0, 4)));
  const minYear = Math.min(thisYear, ...dataYears);
  const maxYear = Math.max(thisYear, ...dataYears);
  const years = Array.from({ length: maxYear - minYear + 1 }, (_, i) => maxYear - i);
  const yearSelect = (value: number) =>
    el("select", { value: String(value) }, ...years.map((y) => el("option", { value: String(y), textContent: String(y), selected: y === value })));

  const curMonth = p.kind === "month" ? p.month : new Date().toISOString().slice(0, 7);
  const monthSel = el("select", {}, ...MONTHS.map((n, i) => el("option", { value: String(i + 1), textContent: n, selected: i + 1 === Number(curMonth.slice(5)) })));
  const monthYear = yearSelect(Number(curMonth.slice(0, 4)));
  const yearOnly = yearSelect(p.kind === "year" ? p.year : thisYear);
  const today = new Date().toISOString().slice(0, 10);
  const from = el("input", { type: "date", value: p.kind === "range" ? p.from : `${today.slice(0, 7)}-01` });
  const to = el("input", { type: "date", value: p.kind === "range" ? p.to : today });

  const fields = el("div", { style: "display:flex;flex-direction:column;gap:12px" });
  const tabs = (["month", "year", "range"] as const).map((k) => {
    const b = el("button", { type: "button", textContent: { month: "חודש", year: "שנה", range: "טווח" }[k] });
    b.onclick = () => ((kind = k), draw());
    return [k, b] as const;
  });
  const labelled = (text: string, input: Node) => el("label", {}, text, input);
  const draw = () => {
    for (const [k, b] of tabs) b.setAttribute("aria-pressed", String(k === kind));
    fields.replaceChildren(
      ...(kind === "month"
        ? [labelled("חודש", monthSel), labelled("שנה", monthYear)]
        : kind === "year"
          ? [labelled("שנה", yearOnly)]
          : [labelled("מתאריך", dateField(from)), labelled("עד תאריך", dateField(to))]),
    );
  };
  draw();

  const err = el("div", { className: "error" });
  const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
  cancel.onclick = () => dlg.close();
  const form = el(
    "form",
    { className: "add", method: "dialog" },
    el("div", { className: "seg3" }, ...tabs.map(([, b]) => b)),
    fields,
    err,
    el("div", { className: "actions" }, cancel, el("button", { type: "submit", className: "btn primary", textContent: "הצגה" })),
  );
  form.onsubmit = (e) => {
    e.preventDefault();
    if (kind === "month") {
      state.period = { kind, month: `${monthYear.value}-${monthSel.value.padStart(2, "0")}` };
    } else if (kind === "year") {
      state.period = { kind, year: Number(yearOnly.value) };
    } else {
      if (!from.value || !to.value || from.value > to.value) {
        err.textContent = "טווח התאריכים אינו תקין";
        return;
      }
      state.period = { kind, from: from.value, to: to.value };
    }
    dlg.close();
    render();
  };
  dlg.append(form);
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  trackDialog(dlg);
  dlg.showModal();
}

/** "Of which the businesses": profit of the businesses in the period; they stay in the figures above. */
function businessCard(): HTMLElement | null {
  if (!state.biz.businesses.length) return null;
  const { total, per } = summaryFigures({ txs: state.txs, cats: state.cats, biz: state.biz, period: state.period });
  if (!state.txs.some((t) => businessOf(state.biz, state.cats, t) != null)) return null;
  // Always opens the whole business; a single business is picked from there.
  const open = () => {
    state.bizView = { tab: "overview", scope: "all" };
    go("business");
  };
  const card = el(
    "div",
    { className: "card bizcard", style: "margin-top:10px" },
    el("div", { className: "head" }, el("div", { className: "grow" }, el("div", { className: "bizname", textContent: "מתוכם העסקים" }), el("div", { className: "muted small", textContent: "כלול בסכומים למעלה" })), el("b", {}, el("span", { textContent: "רווח " }), money(total.profit, total.profit < 0 ? "expense" : "income")), el("span", { className: "muted", textContent: "›" })),
    ...per.map(({ b, f }) => el("div", { className: "row sub" }, el("div", { className: "grow", textContent: b.is_general ? `${b.name} (משותף)` : b.name }), money(f.profit, f.profit < 0 ? "expense" : "income"))),
  );
  return clickable(card, open);
}

function monthView(): HTMLElement {
  const txs = inPeriod();
  const income = txs.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
  const expense = txs.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);
  const stat = (label: string, value: HTMLElement, kind?: "expense" | "income") => {
    const card = el("div", { className: "card" }, el("div", { className: "label", textContent: label }), el("div", { className: "value" }, value));
    if (!kind) return card;
    card.classList.toggle("selected", state.kind === kind);
    return clickable(card, () => ((state.kind = kind), render()));
  };
  const isIncome = state.kind === "income";
  const sign = isIncome ? 1 : -1;

  // Group the chosen side (expenses or income) by top-level category; sub-categories are listed under their parent.
  const groups = new Map<number | null, { total: number; kids: Map<number | null, number> }>();
  for (const t of txs.filter((t) => t.amount_agorot * sign > 0)) {
    const own = catById(t.category_id);
    const top = own?.parent_id ?? t.category_id;
    const g = groups.get(top) ?? { total: 0, kids: new Map() };
    g.total += t.amount_agorot * sign;
    if (top !== t.category_id) g.kids.set(t.category_id, (g.kids.get(t.category_id) ?? 0) + t.amount_agorot * sign);
    groups.set(top, g);
  }
  const rows = [...groups.entries()].sort((a, b) => b[1].total - a[1].total);
  const max = rows[0]?.[1].total ?? 1;

  return el(
    "div",
    {},
    monthBar(),
    el("div", { className: "stats" }, stat("הכנסות", money(income, "income"), "income"), stat("הוצאות", money(-expense, "expense"), "expense"), stat("יתרה", money(income + expense, income + expense < 0 ? "expense" : "income"))),
    businessCard(),
    el("h2", { textContent: isIncome ? "הכנסות לפי קטגוריה" : "הוצאות לפי קטגוריה" }),
    rows.length === 0
      ? el("div", { className: "empty", textContent: isIncome ? "אין הכנסות בתקופה זו" : "אין הוצאות בתקופה זו" })
      : el(
          "div",
          { className: "card rows" },
          ...rows.map(([id, g]) =>
            el(
              "div",
              {},
              clickable(
                el(
                  "div",
                  { className: "row" },
                  el("div", { className: "grow" }, el("div", { className: "name", textContent: catName(id) }), el("div", { className: "bar" }, el("span", { style: `width:${Math.max(3, (g.total / max) * 100)}%` }))),
                  money(g.total),
                ),
                () => showCategory(id),
              ),
              ...[...g.kids.entries()]
                .sort((a, b) => b[1] - a[1])
                .map(([kid, total]) =>
                  clickable(
                    el("div", { className: "row sub" }, el("div", { className: "grow muted", textContent: catName(kid) }), money(total)),
                    () => showCategory(kid),
                  ),
                ),
            ),
          ),
        ),
  );
}

function listView(): HTMLElement {
  const txs = inPeriod()
    .filter(inFilter)
    .filter((t) => state.bizFilter === "all" || (businessOf(state.biz, state.cats, t) != null) === (state.bizFilter === "biz"));
  const days = [...new Set(txs.map((t) => t.date))].sort().reverse();
  const body: Child[] = days.map((d) =>
    el(
      "div",
      {},
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
            const pickBox = state.selecting && !t.upcoming && t.sync_state !== "queued" ? el("input", { type: "checkbox", className: "selbox", checked: state.selected.has(t.id), ariaLabel: "בחירה" }) : null;
            if (pickBox)
              pickBox.onchange = () => {
                if (pickBox.checked) state.selected.add(t.id);
                else state.selected.delete(t.id);
                render();
              };
            return el(
              "div",
              { className: t.upcoming ? "row upcoming" : "row" },
              pickBox,
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
  const selectBtn = hasBiz ? el("button", { className: "btn", type: "button", textContent: state.selecting ? "ביטול בחירה" : "בחירה וסימון לעסק" }) : null;
  if (selectBtn)
    selectBtn.onclick = () => {
      state.selecting = !state.selecting;
      state.selected.clear();
      render();
    };
  const tag = el("button", { className: "btn primary", type: "button", textContent: `סימון לעסק (${state.selected.size})` });
  tag.disabled = state.selected.size === 0;
  tag.onclick = () => openTagDialog(manageCtx(), [...state.selected], () => ((state.selecting = false), state.selected.clear()));
  const selBar = state.selecting
    ? el("div", { className: "selbar" }, el("div", { className: "grow muted", textContent: "בחרו תנועות, גם ישנות" }), tag)
    : null;
  return el("div", {}, header, monthBar(), bizSeg, selectBtn ? el("div", { className: "actions", style: "justify-content:flex-start;margin-bottom:8px" }, selectBtn) : null, ...(body.length ? body : [el("div", { className: "empty", textContent: "אין תנועות בתקופה זו" })]), selBar);
}

function manageCtx() {
  return { biz: state.biz, cats: state.cats, txs: state.txs, openDialog, reload: load, back: () => goBack("settings") };
}

/** Open a modal built by `build`, which gets a way to close it. */
function openDialog(build: (close: () => void) => HTMLElement) {
  const dlg = el("dialog");
  dlg.append(build(() => dlg.close()));
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  trackDialog(dlg);
  dlg.showModal();
}

/** Paamonim top-level categories of a kind: what a top-level category made here can stand in for. */
const paamonimTops = (isIncome: boolean) => state.cats.filter((c) => c.parent_id == null && c.from_paamonim && c.is_income === isIncome);

/** The Paamonim top-level category a top-level category's lines go under, if any. */
const paamonimHome = (c: Category): number | null => (c.parent_id != null ? null : c.from_paamonim ? c.id : c.linked_to);

function paamonimSelect(isIncome: boolean): HTMLSelectElement {
  const sel = el("select");
  sel.append(el("option", { value: "", textContent: "בחירת קטגוריה בפעמונים…" }), ...paamonimTops(isIncome).map((c) => el("option", { value: String(c.id), textContent: c.name })));
  return sel;
}

function openAddCategoryDialog(isIncome: boolean, parentId: number | null) {
  openDialog((close) => {
    const name = el("input", { type: "text", required: true, maxLength: 100, autocomplete: "off" });
    const parent = el("select");
    parent.append(
      el("option", { value: "", textContent: "קטגוריה ראשית" }),
      ...state.cats.filter((c) => c.parent_id == null && c.is_income === isIncome).map((c) => el("option", { value: String(c.id), textContent: `תת-קטגוריה של ${c.name}` })),
    );
    parent.value = parentId != null ? String(parentId) : "";
    // Paamonim has no way to add a top-level category, so a new one stands in for one there.
    const hasTops = paamonimTops(isIncome).length > 0;
    const link = paamonimSelect(isIncome);
    const linkRow = el("label", {}, "קטגוריה בפעמונים", link);
    const note = el("div", { className: "note muted" });
    const sync = () => {
      const top = !parent.value;
      linkRow.style.display = top && hasTops ? "" : "none";
      link.required = top && hasTops;
      const p = state.cats.find((c) => String(c.id) === parent.value);
      note.textContent = top
        ? hasTops
          ? "פעמונים לא מאפשר קטגוריות ראשיות חדשות. תתי-הקטגוריות של קטגוריה זו יירשמו בפעמונים תחת הקטגוריה שנבחרה."
          : "קטגוריה שנוספה כאן קיימת רק באפליקציה: תנועות עליה לא יועתקו לפעמונים."
        : p && paamonimHome(p) != null
          ? "תת-הקטגוריה תירשם בפעמונים כסעיף באותו שם (אם כבר יש סעיף בשם זה, הוא ישמש)."
          : "קטגוריה שנוספה כאן קיימת רק באפליקציה: תנועות עליה לא יועתקו לפעמונים.";
    };
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "הוספה" });
    // Greyed out until there is a name and, for a top-level category, its Paamonim category.
    const syncSave = () => (save.disabled = !name.value.trim() || (!parent.value && hasTops && !link.value));
    parent.onchange = () => (sync(), syncSave());
    link.onchange = syncSave;
    name.oninput = syncSave;
    sync();
    syncSave();
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: isIncome ? "הוספת קטגוריית הכנסה" : "הוספת קטגוריית הוצאה" }),
      el("label", {}, "שם", name),
      el("label", {}, "סוג", parent),
      linkRow,
      note,
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        const top = !parent.value;
        await api.addCategory({
          name: name.value,
          is_income: isIncome,
          parent_id: top ? null : Number(parent.value),
          linked_to: top && link.value ? Number(link.value) : null,
        });
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        syncSave();
      }
    };
    setTimeout(() => name.focus());
    return form;
  });
}

/** Choose which Paamonim category a top-level category made here stands in for. */
function openLinkCategoryDialog(cat: Category) {
  openDialog((close) => {
    const link = paamonimSelect(cat.is_income);
    link.required = true;
    link.value = cat.linked_to != null ? String(cat.linked_to) : "";
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "שמירה" });
    const syncSave = () => (save.disabled = !link.value || link.value === String(cat.linked_to));
    link.onchange = syncSave;
    syncSave();
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `"${cat.name}" בפעמונים` }),
      el("label", {}, "קטגוריה בפעמונים", link),
      el("div", { className: "note muted", textContent: "תתי-הקטגוריות של קטגוריה זו יירשמו בפעמונים תחת הקטגוריה שנבחרה." }),
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        await api.linkCategory(cat.id, Number(link.value));
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        syncSave();
      }
    };
    return form;
  });
}

/** Move a sub-category, with its transactions and repeating rules, under another top-level category. */
function openMoveCategoryDialog(cat: Category) {
  openDialog((close) => {
    const from = state.cats.find((c) => c.id === cat.parent_id);
    const home = from ? paamonimHome(from) : null;
    // An item in Paamonim stays where it is there, so it can only move within its Paamonim category.
    const targets = state.cats.filter(
      (c) => c.parent_id == null && c.is_income === cat.is_income && c.id !== cat.parent_id && (!cat.from_paamonim || paamonimHome(c) === home),
    );
    const lines = state.txs.filter((t) => t.category_id === cat.id && !t.upcoming).length;
    const target = el("select");
    target.required = true;
    target.append(el("option", { value: "", textContent: "בחירת קטגוריה…" }), ...targets.map((c) => el("option", { value: String(c.id), textContent: c.name })));
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "העברה" });
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `העברת "${cat.name}"` }),
      el("p", { className: "muted", textContent: `כרגע תחת "${from?.name ?? ""}". ${lines} התנועות שלה והתנועות החוזרות עוברות איתה.` }),
      targets.length
        ? el("label", {}, "העברה אל", target)
        : el("p", { className: "muted", textContent: "אין קטגוריה ראשית אחרת שאפשר להעביר אליה." }),
      cat.from_paamonim && el("div", { className: "note muted", textContent: "זהו סעיף בפעמונים, ולכן אפשר להעביר אותו רק לקטגוריות המקושרות לאותה קטגוריה בפעמונים. בפעמונים דבר לא משתנה." }),
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    const syncSave = () => (save.disabled = !target.value);
    target.onchange = syncSave;
    syncSave();
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        await api.moveCategory(cat.id, Number(target.value));
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        syncSave();
      }
    };
    return form;
  });
}

function openDeleteCategoryDialog(cat: Category) {
  openDialog((close) => {
    const subs = state.cats.filter((c) => c.parent_id === cat.id);
    const doomed = new Set([cat.id, ...subs.map((c) => c.id)]);
    // Occurrences not yet settled are not in txs, so repeating rules are counted separately.
    const lines = state.txs.filter((t) => t.category_id != null && doomed.has(t.category_id) && !t.upcoming).length;
    const rules = new Set(state.recurring.filter((r) => r.category_id != null && doomed.has(r.category_id)).map((r) => r.series_id)).size;
    const used = lines + rules;
    const target = el("select");
    const opt = (c: Category) => el("option", { value: String(c.id), textContent: catPath(c.id) });
    target.append(
      el("option", { value: "", textContent: "בחירת קטגוריה…" }),
      ...state.cats.filter((c) => c.is_income === cat.is_income && !doomed.has(c.id)).map(opt),
    );
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const del = el("button", { type: "submit", className: "btn primary", textContent: "מחיקה" });
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `מחיקת "${cat.name}"` }),
      subs.length > 0 && el("p", { className: "muted", textContent: `יימחקו גם ${subs.length} תתי-קטגוריות.` }),
      used > 0
        ? el("label", {}, `${lines} תנועות ו-${rules} הוצאות חוזרות משויכות לקטגוריה. להעביר אותן אל:`, target)
        : el("p", { className: "muted", textContent: "אין תנועות משויכות." }),
      err,
      el("div", { className: "actions" }, cancel, del),
    );
    if (used > 0) target.required = true;
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        del.disabled = true;
        await api.deleteCategory(cat.id, target.value ? Number(target.value) : undefined);
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        del.disabled = false;
      }
    };
    return form;
  });
}

function categoriesView(): HTMLElement {
  const back = el("button", { className: "btn", textContent: "→ הגדרות", title: "חזרה להגדרות" });
  back.onclick = () => {
    goBack("settings");
  };
  return el("div", {}, el("div", { className: "actions", style: "justify-content:flex-start" }, back), el("h1", { textContent: "קטגוריות" }), categoriesCard());
}

function categoriesLinkCard(): HTMLElement {
  const open = el("button", { className: "btn", textContent: "עריכת קטגוריות" });
  open.onclick = () => {
    go("categories");
  };
  return el("div", { className: "card" }, el("h2", { textContent: "קטגוריות" }), el("p", { className: "muted", textContent: "הוספה ומחיקה של קטגוריות ותתי-קטגוריות." }), el("div", { className: "actions", style: "justify-content:flex-start" }, open));
}

function businessesLinkCard(): HTMLElement {
  const open = el("button", { className: "btn", textContent: "ניהול עסקים" });
  open.onclick = () => go("businesses");
  return el("div", { className: "card" }, el("h2", { textContent: "עסקים" }), el("p", { className: "muted", textContent: "עסקים ותתי-עסקים לצפייה ברווח והפסד, בלי להוציא את התנועות מסיכום משק הבית." }), el("div", { className: "actions", style: "justify-content:flex-start" }, open));
}

function categoriesCard(): HTMLElement {
  const kinds = [
    [false, "הוצאות"],
    [true, "הכנסות"],
  ] as const;
  const tabs = kinds.map(([inc, label]) => {
    const b = el("button", { type: "button", textContent: label });
    b.onclick = () => ((state.catKind = inc), render());
    b.setAttribute("aria-pressed", String(state.catKind === inc));
    return b;
  });
  const ofKind = state.cats.filter((c) => c.is_income === state.catKind);
  const iconBtn = (text: string, title: string, on: () => void) => {
    const b = el("button", { type: "button", className: "btn", textContent: text, title });
    b.onclick = on;
    return b;
  };
  const linkedName = (c: Category) => state.cats.find((x) => x.id === c.linked_to)?.name;
  const row = (c: Category, sub: boolean) =>
    el(
      "div",
      { className: sub ? "row sub" : "row" },
      el(
        "div",
        { className: "grow" },
        el("div", { className: "name", textContent: c.name }),
        !sub && c.linked_to != null && el("div", { className: "muted", textContent: `בפעמונים: ${linkedName(c) ?? ""}` }),
        !sub && !c.from_paamonim && c.linked_to == null && el("div", { className: "muted", textContent: "לא מקושרת לפעמונים" }),
        state.biz.cat.has(c.id) ? el("div", {}, el("span", { className: "bizchip", textContent: `עסק: ${bizName(state.biz, state.biz.cat.get(c.id))}` })) : null,
      ),
      state.biz.businesses.length > 0 && iconBtn("💼", "עסק של הקטגוריה", () => openCategoryBusinessDialog(manageCtx(), c)),
      !sub && !c.from_paamonim && iconBtn("🔗", "קטגוריה בפעמונים", () => openLinkCategoryDialog(c)),
      !sub && iconBtn("+", "הוספת תת-קטגוריה", () => openAddCategoryDialog(c.is_income, c.id)),
      sub && iconBtn("⇄", "העברה לקטגוריה אחרת", () => openMoveCategoryDialog(c)),
      iconBtn("🗑", "מחיקה", () => openDeleteCategoryDialog(c)),
    );
  const rows = ofKind
    .filter((c) => c.parent_id == null)
    .flatMap((c) => [row(c, false), ...ofKind.filter((k) => k.parent_id === c.id).map((k) => row(k, true))]);
  return el(
    "div",
    { className: "card" },
    el("div", { className: "seg" }, ...tabs),
    el("div", { className: "rows" }, ...(rows.length ? rows : [el("div", { className: "empty", textContent: "אין קטגוריות" })])),
    el("div", { className: "actions", style: "justify-content:flex-start" }, iconBtn("הוספת קטגוריה", "", () => openAddCategoryDialog(state.catKind, null))),
  );
}

function settingsView(): HTMLElement {
  const out = el("div", { className: "note muted" });
  const run = (btn: HTMLButtonElement, work: () => Promise<string>) => async () => {
    btn.disabled = true;
    out.className = "note muted";
    out.textContent = "מבצע…";
    try {
      out.textContent = await work();
      await load();
    } catch (e) {
      out.className = "note error";
      out.textContent = `שגיאה: ${e instanceof Error ? e.message : e}`;
    } finally {
      btn.disabled = false;
    }
  };
  const imp = el("button", { className: "btn", textContent: "ייבוא מפעמונים" });
  imp.onclick = run(imp, async () => {
    const s = await api.importPaamonim("2000-01-01", new Date().toISOString().slice(0, 10));
    return `יובאו ${s.transactions_added} תנועות חדשות (${s.transactions_skipped} כבר קיימות), ${s.categories_added} קטגוריות חדשות.`;
  });
  const exp = el("button", { className: "btn", textContent: "ייצוא ל-Google Sheets" });
  exp.onclick = run(exp, async () => {
    const s = await api.exportSheets();
    return `יוצאו ${s.transactions} תנועות.`;
  });
  return el(
    "div",
    {},
    el("h1", { textContent: "הגדרות" }),
    el("div", { className: "card" }, el("p", { className: "muted", textContent: "ייבוא מושך את כל ההיסטוריה מפעמונים ואינו יוצר כפילויות. ייצוא כותב עותק לגיליון Google." }), el("div", { className: "actions", style: "justify-content:flex-start" }, imp, exp), out),
    categoriesLinkCard(),
    businessesLinkCard(),
    historyCard(),
  );
}

function historyCard(): HTMLElement {
  const open = el("button", { className: "btn", textContent: "צפייה בהיסטוריית שינויים" });
  open.onclick = () => void openLog().catch(() => alert("טעינת ההיסטוריה נכשלה"));
  return el(
    "div",
    { className: "card" },
    el("h2", { textContent: "היסטוריית שינויים" }),
    el("p", { className: "muted", textContent: "כל הוספה, עריכה ומחיקה של תנועות, קטגוריות והוצאות חוזרות, עם הערכים לפני ואחרי. נשמרת שנה אחת." }),
    el("div", { className: "actions", style: "justify-content:flex-start" }, open),
  );
}

const LOG_PAGE = 100;

/** Icon and Hebrew name of each kind of change. */
const LOG_KINDS: Record<string, [string, string]> = {
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

const LOG_FIELDS: Record<string, string> = {
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

const monthLabel = (m: string) => `${m.slice(5)}/${m.slice(0, 4)}`;

function logValue(key: string, v: unknown): string {
  if (v == null || v === "" || (Array.isArray(v) && !v.length)) return key === "end_month" ? "ללא סיום" : "—";
  if (key === "amount_agorot") return formatShekels(v as number);
  if (key === "date") return fmtDate(String(v));
  if (key.endsWith("_month")) return monthLabel(v as string);
  if (key === "is_income") return v ? "הכנסה" : "הוצאה";
  if (Array.isArray(v)) return v.join(", ");
  return String(v);
}

/** The lines under a log entry: what changed (old ← new) for an edit, otherwise the values recorded. */
function logLines(e: LogEntry): HTMLElement[] {
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

function logEntryRow(e: LogEntry): HTMLElement {
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

function logView(): HTMLElement {
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

async function openLog() {
  state.log = await api.log(undefined, LOG_PAGE);
  state.logMore = state.log.length === LOG_PAGE;
  go("log");
}

/** A button that opens a small menu of choices above it. */
function menuButton(label: string, items: [string, () => void][], className = "btn"): HTMLElement {
  const menu = el("div", { className: "menu", hidden: true }, ...items.map(([text, run]) => {
    const item = el("button", { type: "button", textContent: text });
    item.onclick = () => ((menu.hidden = true), run());
    return item;
  }));
  const button = el("button", { type: "button", className, textContent: `${label} ▾` });
  button.onclick = () => (menu.hidden = !menu.hidden);
  return el("div", { className: "menuwrap" }, button, menu);
}


/**
 * When a pick list (category, sub-category, currency) opens, scroll its label to the top of the dialog so the list under
 * the field is not hidden. Plain fields (amount, note, date) are left to the browser.
 */
function revealField(input: HTMLElement) {
  const dlg = input.closest("dialog");
  const top = input.closest("label") ?? input;
  const go = () => dlg && dlg.scrollTo({ top: dlg.scrollTop + top.getBoundingClientRect().top - dlg.getBoundingClientRect().top - dlg.clientTop, behavior: "smooth" });
  go();
  // The keyboard opens a moment after the field is focused: scroll again once it has.
  setTimeout(go, 350);
}
document.addEventListener("focusin", (e) => e.target instanceof HTMLElement && e.target.getAttribute("role") === "combobox" && e.target.closest("dialog") && revealField(e.target));

interface ComboItem {
  value: string;
  label: string;
}
/**
 * A text field with a drop-down list that narrows as you type. Picking an item sets `value`;
 * leaving the field with text that matches nothing puts the picked label back.
 */
function combo(placeholder: string) {
  let items: ComboItem[] = [];
  let value = "";
  const input = el("input", { type: "text", placeholder, autocomplete: "off", spellcheck: false });
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-expanded", "false");
  const list = el("div", { className: "combo-list", hidden: true });
  list.setAttribute("role", "listbox");
  const root = el("div", { className: "combo" }, input, list);
  const labelOf = (v: string) => items.find((i) => i.value === v)?.label ?? "";
  const close = () => {
    list.hidden = true;
    input.setAttribute("aria-expanded", "false");
    input.value = labelOf(value);
    document.removeEventListener("click", outside, true);
  };
  /** Only a tap elsewhere closes the list: dismissing the phone keyboard also blurs the field and must not, and neither must scrolling (a scroll gesture fires pointerdown but no click). */
  const outside = (e: Event) => (root.isConnected ? root.contains(e.target as Node) || close() : document.removeEventListener("click", outside, true));
  /** When the last item was picked: the tap's own click may land on the field once the layout shifts, and must not reopen the list. */
  let pickedAt = 0;
  const pick = (v: string) => {
    value = v;
    input.value = labelOf(v);
    close();
  };
  const show = (filter: string) => {
    const q = filter.trim().toLowerCase();
    const shown = items.filter((i) => !q || i.label.toLowerCase().includes(q));
    list.replaceChildren(
      ...(shown.length
        ? shown.map((i) => {
            const b = el("button", { type: "button", textContent: i.label });
            b.setAttribute("role", "option");
            b.setAttribute("aria-selected", String(i.value === value));
            // mousedown, so the pick lands before the input's blur closes the list.
            b.onmousedown = (e) => (e.preventDefault(), (pickedAt = Date.now()), pick(i.value), input.blur(), input.dispatchEvent(new Event("change")));
            return b;
          })
        : [el("div", { className: "combo-empty", textContent: "אין תוצאות" })]),
    );
    list.hidden = false;
    input.setAttribute("aria-expanded", "true");
    document.addEventListener("click", outside, true);
  };
  const justPicked = () => Date.now() - pickedAt <= 500;
  input.onfocus = () => {
    // The layout can shift after a pick (the shekel box hides) so the tap's click lands on the field: stay closed.
    if (justPicked()) return input.blur();
    input.select();
    show("");
  };
  input.onclick = () => {
    if (justPicked()) return input.blur();
    if (list.hidden) show("");
  };
  input.oninput = () => show(input.value);
  input.onkeydown = (e) => {
    if (e.key === "Tab" && !list.hidden) close();
    if (e.key === "Escape" && !list.hidden) (e.stopPropagation(), e.preventDefault(), close());
    if (e.key === "Enter" && !list.hidden) {
      // Enter takes the first match instead of submitting the form.
      e.preventDefault();
      const first = list.querySelector<HTMLButtonElement>("button");
      if (first) first.dispatchEvent(new MouseEvent("mousedown", { cancelable: true }));
    }
  };
  return {
    root,
    input,
    get value() {
      return value;
    },
    set(newItems: ComboItem[], v = "") {
      items = newItems;
      pick(v);
    },
    pick,
    setDisabled(d: boolean, ph = placeholder) {
      input.disabled = d;
      input.placeholder = ph;
      if (d) close();
    },
  };
}

const monthsBetween = (from: string, to: string) => (Number(to.slice(0, 4)) - Number(from.slice(0, 4))) * 12 + Number(to.slice(5)) - Number(from.slice(5));
const addMonths = (month: string, n: number) => new Date(Date.UTC(Number(month.slice(0, 4)), Number(month.slice(5)) - 1 + n, 1)).toISOString().slice(0, 7);

/**
 * Add or edit a transaction. An `existing` generated occurrence of a repeating transaction edits
 * the series from that occurrence's month on; earlier months are left as they were.
 */
function openTransactionDialog(existing?: Transaction, startAsIncome = false) {
  const seriesId = existing?.upcoming ? existing.recurring_series_id! : undefined;
  const segment = state.recurring.find((s) => s.id === existing?.recurring_segment_id);
  /** The month of the occurrence being edited; the date field may move the charge out of it. */
  const month = existing?.date.slice(0, 7) ?? "";
  let isIncome = existing ? existing.amount_agorot > 0 : startAsIncome;
  const dlg = el("dialog");
  // The shekel is the default currency, always; a line already entered in USD or JPY opens in that one.
  const startFx = existing && !existing.upcoming && existing.id > 0 ? state.fx.get(existing.id) : undefined;
  let currency: Currency = startFx?.currency ?? "ILS";
  const amount = el("input", {
    type: "text",
    inputMode: "decimal",
    placeholder: "0.00",
    required: true,
    autocomplete: "off",
    value: startFx ? String(Math.abs(startFx.orig_minor) / 10 ** decimals(startFx.currency)) : existing ? String(Math.abs(existing.amount_agorot) / 100) : "",
  });
  const date = el("input", { type: "date", required: true, value: existing?.date ?? new Date().toISOString().slice(0, 10) });
  const topPick = combo("חיפוש קטגוריה…");
  const subPick = combo("תת-קטגוריה");
  const note = el("input", { type: "text", placeholder: "אופציונלי", maxLength: 35, value: existing?.note ?? "" });
  const err = el("div", { className: "error" });
  // The business the line counts in: its own tag, or the one its category gives (then there is nothing to choose).
  const ownTag = existing && existing.id > 0 ? (state.biz.tx.get(existing.id) ?? null) : null;
  const bizPick = businessChips(state.biz, ownTag, true);
  const bizRow = el("div", {}, bizPick.root);
  const lastKey = (cat: number) => `biz-last-${cat}`;
  const syncBiz = () => {
    let cat: number | null = null;
    try {
      cat = categoryId();
    } catch {
      const top = topPick.value;
      cat = top ? Number(top) : null;
    }
    const given = cat != null ? categoryBusiness(state.biz, state.cats, cat) : undefined;
    bizPick.lock(given != null && ownTag == null ? bizName(state.biz, given) : null);
    // A new line in a category that needs a business must pick one (None is a choice); nothing is chosen for it.
    bizPick.setRequired(!existing && cat != null && !repeat.checked && businessRequired(state.biz, state.cats, cat));
    bizRow.style.display = state.biz.businesses.length === 0 || repeat.checked ? "none" : "";
  };
  /** A new line starts with the business last used in the same category. */
  const rememberedBiz = () => {
    if (existing || bizPick.required) return;
    try {
      const sub = subPick.value || topPick.value;
      const v = sub ? localStorage.getItem(lastKey(Number(sub))) : null;
      bizPick.set(v && state.biz.businesses.some((b) => String(b.id) === v) ? Number(v) : null);
    } catch {
      /* storage unavailable */
    }
  };

  // Repeating monthly: for a new transaction it is opt-in; for an occurrence it is always on.
  const repeat = el("input", { type: "checkbox", checked: seriesId !== undefined, disabled: seriesId !== undefined });
  const forever = el("select", {}, el("option", { value: "forever", textContent: "ללא הגבלה" }), el("option", { value: "months", textContent: "למשך מספר חודשים" }));
  const months = el("input", { type: "number", min: "1", step: "1", inputMode: "numeric", value: "12" });
  if (segment?.end_month) {
    forever.value = "months";
    months.value = String(monthsBetween(existing!.date.slice(0, 7), segment.end_month) + 1);
  }
  const repeatBox = el("div", { style: "display:flex;flex-direction:column;gap:12px" }, el("label", {}, "משך", forever), el("label", {}, "מספר חודשים (כולל החודש הזה)", months));
  const showRepeat = () => {
    repeatBox.style.display = repeat.checked ? "flex" : "none";
    months.parentElement!.style.display = forever.value === "months" ? "" : "none";
  };
  repeat.onchange = forever.onchange = showRepeat;
  showRepeat();

  // Foreign currency (USD, JPY): the shekel amount follows the rate for the transaction's day.
  /** The currency in force: repeating charges are in shekels only. */
  const cur = (): Currency => (repeat.checked ? "ILS" : currency);
  const amountLbl = el("span", { textContent: "סכום (₪)" });
  const curPick = combo("חיפוש מטבע…");
  curPick.set(CURRENCIES.map((c) => ({ value: c.code, label: `${c.symbol} ${c.name} ${c.code}` })), currency);
  curPick.input.addEventListener("change", () => {
    currency = (curPick.value || "ILS") as Currency;
    syncFx();
    void refreshQuote();
    syncSave();
  });
  const curSeg = el("label", { className: "curfield" }, "מטבע", curPick.root);
  const ils = el("input", { type: "text", inputMode: "decimal", placeholder: "0.00", autocomplete: "off" });
  const fxNote = el("div", { className: "fxnote muted" });
  const refreshBtn = el("button", { type: "button", className: "btn small", textContent: "עדכון שער" });
  const fxBox = el("div", { className: "fxbox" }, el("label", {}, "סכום בשקלים (₪)", ils), fxNote, startFx ? refreshBtn : null);
  /** The rate for the chosen day (null: unknown, e.g. offline: the server converts when the line is sent). */
  let quote: { rate: number; date: string; estimated: boolean } | null = startFx ? { rate: startFx.rate, date: startFx.rate_date, estimated: startFx.estimated } : null;
  let quoteSeq = 0;
  /** The shekel amount was typed in (what the card really charged): it is used as it is. */
  let ilsTouched = false;
  /** "Update rate" was pressed: look the rate up again instead of keeping the saved one. */
  let forceLookup = false;
  if (startFx) ils.value = String(Math.abs(existing!.amount_agorot) / 100);
  const origMinor = () => {
    try {
      return Math.abs(parseMinor(amount.value, cur()));
    } catch {
      return 0;
    }
  };
  /** A saved foreign line whose currency, amount and day are as they were keeps its saved rate. */
  const fxUnchanged = () => !!startFx && cur() === startFx.currency && origMinor() === Math.abs(startFx.orig_minor) && date.value === existing!.date;
  const recalc = () => {
    if (cur() === "ILS") return;
    const sym = currencySymbol(cur());
    if (!quote) {
      fxNote.textContent = "אין שער זמין כרגע: השער ייקבע כשהתנועה תישלח";
      return;
    }
    if (!ilsTouched) {
      const minor = origMinor();
      ils.value = minor ? ((minor / 10 ** decimals(cur())) * quote.rate).toFixed(2) : "";
    }
    fxNote.textContent = ilsTouched ? `סכום החיוב הוזן ידנית · השער יחושב ממנו` : `שער: 1 ${sym} = ₪${formatRate(quote.rate)} · ${fmtDate(quote.date)}${quote.estimated ? " · זמני, יתעדכן אוטומטית כשהיום יסתיים" : ""}`;
  };
  const refreshQuote = async () => {
    if (cur() === "ILS") return;
    const seq = ++quoteSeq;
    fxNote.textContent = "מביא שער…";
    try {
      const q = await api.fxRate(cur(), date.value);
      if (seq === quoteSeq) quote = q;
    } catch {
      if (seq !== quoteSeq) return;
      quote = null;
    }
    if (seq === quoteSeq) recalc();
  };
  const syncFx = () => {
    const c = cur();
    if (curPick.value !== c) curPick.pick(c);
    curSeg.style.display = repeat.checked ? "none" : "";
    fxBox.style.display = c === "ILS" ? "none" : "";
    amountLbl.textContent = `סכום (${currencySymbol(c)})`;
    if (c !== "ILS") recalc();
  };
  amount.addEventListener("input", recalc);
  ils.addEventListener("input", () => {
    ilsTouched = ils.value.trim() !== "";
    recalc();
  });
  date.addEventListener("change", () => void refreshQuote());
  refreshBtn.onclick = () => {
    ilsTouched = false;
    forceLookup = true;
    void refreshQuote();
  };
  repeat.addEventListener("change", () => (syncFx(), syncSave()));
  const expBtn = el("button", { type: "button", textContent: "הוצאה" });
  const incBtn = el("button", { type: "button", textContent: "הכנסה" });
  const kindCats = () => state.cats.filter((c) => c.is_income === isIncome);
  const kidsOf = (topId: string) => kindCats().filter((k) => String(k.parent_id) === topId);
  /**
   * The sub-category list follows the category: just its sub-categories once one is picked, otherwise every
   * sub-category (named with its parent). It is only locked for a category that has none.
   */
  const fillSubs = (keepSub = "") => {
    const top = topPick.value;
    const kids = top ? kidsOf(top) : kindCats().filter((k) => k.parent_id != null);
    subPick.set(
      kids.map((k) => ({ value: String(k.id), label: top ? k.name : catPath(k.id) })),
      kids.some((k) => String(k.id) === keepSub) ? keepSub : "",
    );
    subPick.setDisabled(top !== "" && kids.length === 0, top && kids.length === 0 ? "אין תתי-קטגוריות" : "חיפוש תת-קטגוריה…");
  };
  topPick.input.addEventListener("change", () => ((err.textContent = ""), fillSubs()));
  // Picking a sub-category first fills in its category.
  subPick.input.addEventListener("change", () => {
    err.textContent = "";
    const sub = subPick.value;
    const parent = catById(Number(sub))?.parent_id;
    if (sub && parent != null && topPick.value !== String(parent)) {
      topPick.pick(String(parent));
      fillSubs(sub);
    }
  });
  /** The category the transaction goes on: always a sub-category (top-level entries are not mirrored to Paamonim). */
  const categoryId = (): number => {
    const top = topPick.value;
    if (!top) throw new Error("יש לבחור קטגוריה");
    if (subPick.value) return Number(subPick.value);
    // Only a transaction already on a top-level category (from before sub-categories were required) keeps it.
    if (existing?.category_id === Number(top)) return Number(top);
    throw new Error("יש לבחור תת-קטגוריה");
  };
  /** Saving stays greyed out until the category (and its sub-category, if it has any) is chosen. */
  const categoryReady = () => {
    try {
      categoryId();
      return true;
    } catch {
      return false;
    }
  };
  /** A usable amount: a number that is not zero (to the agora, cent or yen). */
  const amountReady = () => {
    try {
      return parseMinor(amount.value, cur()) !== 0 && amount.value.trim() !== "";
    } catch {
      return false;
    }
  };
  const syncSave = () => {
    const ready = categoryReady() && amountReady() && bizPick.chosen;
    save.disabled = !ready;
    for (const b of dlg.querySelectorAll<HTMLButtonElement>(".needs-category")) b.disabled = !ready;
  };
  amount.addEventListener("input", syncSave);
  topPick.input.addEventListener("change", syncSave);
  subPick.input.addEventListener("change", syncSave);
  let firstFill = true;
  const fillCats = (sync = true) => {
    expBtn.setAttribute("aria-pressed", String(!isIncome));
    incBtn.setAttribute("aria-pressed", String(isIncome));
    // First time: the transaction's own category; after switching kind it no longer applies unless it fits.
    let top = topPick.value;
    let sub = subPick.value;
    if (firstFill && existing?.category_id != null) {
      const own = catById(existing.category_id);
      top = String(own?.parent_id ?? existing.category_id);
      sub = own?.parent_id != null ? String(own.id) : "";
    }
    firstFill = false;
    // A category without sub-categories cannot hold transactions, so it is not offered (unless one already sits on it).
    const tops = kindCats().filter((c) => c.parent_id == null && (kidsOf(String(c.id)).length > 0 || c.id === existing?.category_id));
    if (!tops.some((c) => String(c.id) === top)) (top = ""), (sub = "");
    topPick.set(tops.map((c) => ({ value: String(c.id), label: c.name })), top);
    fillSubs(sub);
    if (sync) syncSave();
  };
  expBtn.onclick = () => ((isIncome = false), fillCats());
  incBtn.onclick = () => ((isIncome = true), fillCats());
  fillCats(false);
  for (const c of [topPick.input, subPick.input]) c.addEventListener("change", () => (bizPick.clear(), syncBiz(), rememberedBiz(), syncSave()));
  repeat.addEventListener("change", () => (syncBiz(), syncSave()));
  bizPick.onchange = syncSave;
  syncBiz();

  const cancel = el("button", { type: "button", className: "btn", textContent: seriesId !== undefined ? "סגירה" : "ביטול" });
  cancel.onclick = () => dlg.close();
  const save = el("button", { type: "submit", className: "btn primary", textContent: existing ? "עדכון" : "שמירה" });
  // Only this month: becomes a transaction with these values; later months are unchanged.
  const updateOnce = async () => {
    try {
      const abs = Math.abs(parseAgorot(amount.value));
      if (abs === 0) throw new Error("יש להזין סכום");
      await api.settleRecurringMonth(seriesId!, month, {
        date: date.value,
        amount_agorot: isIncome ? abs : -abs,
        category_id: categoryId(),
        payment_method_id: existing?.payment_method_id ?? null,
        note: note.value,
      });
      showPeriodFor(date.value);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = e2 instanceof Error ? e2.message : String(e2);
    }
  };
  // Stop the series: this month and later disappear, earlier months stay.
  const stopFromHere = async () => {
    if (!confirm("לבטל את ההוצאה החוזרת מחודש זה והלאה? חודשים קודמים לא ישתנו.")) return;
    try {
      await api.cancelRecurring(seriesId!, month);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = e2 instanceof Error ? e2.message : String(e2);
    }
  };
  // Skip only this month; the series carries on around it.
  const skipOnce = async () => {
    if (!confirm("לדלג על החיוב של חודש זה בלבד?")) return;
    try {
      await api.skipRecurringMonth(seriesId!, month);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = e2 instanceof Error ? e2.message : String(e2);
    }
  };
  const form = el(
    "form",
    { className: "add", method: "dialog" },
    el("div", { className: "seg" }, expBtn, incBtn),
    el("div", { className: "amountrow" }, el("label", {}, amountLbl, amount), curSeg),
    fxBox,
    el("label", {}, "קטגוריה", topPick.root),
    el("label", {}, "תת-קטגוריה", subPick.root),
    el("label", {}, "תאריך", dateField(date)),
    el("label", {}, "הערה", note),
    bizRow,
    !existing ? el("label", { style: "flex-direction:row;align-items:center;gap:8px" }, repeat, "חוזר מדי חודש") : null,
    repeatBox,
    err,
    seriesId !== undefined
      ? el(
          "div",
          { className: "actions" },
          cancel,
          menuButton("ביטול חיוב", [["חודש זה בלבד", skipOnce], ["מכאן והלאה", stopFromHere]]),
          menuButton("עדכון", [["חודש זה בלבד", updateOnce], ["מכאן והלאה", () => dlg.querySelector("form")!.requestSubmit()]], "btn primary"),
        )
      : el("div", { className: "actions" }, cancel, save),
  );
  // The update menu of a repeating charge is greyed out the same way.
  form.querySelectorAll(".menuwrap > button.primary").forEach((b) => b.classList.add("needs-category"));
  dlg.append(form);
  syncFx();
  if (!startFx) void refreshQuote();
  syncSave();
  form.onsubmit = async (e) => {
    e.preventDefault();
    try {
      const abs = Math.abs(parseAgorot(amount.value));
      if (abs === 0) throw new Error("יש להזין סכום");
      save.disabled = true;
      const sign = isIncome ? 1 : -1;
      const catId = categoryId();
      const body: TxInput = { date: date.value, amount_agorot: sign * abs, category_id: catId, payment_method_id: existing?.payment_method_id ?? null, note: note.value };
      if (cur() !== "ILS") {
        // Foreign: send the original amount; the server converts it at the rate for the day unless
        // the shekel amount was typed in, or the saved line is unchanged (then its rate is kept).
        delete body.amount_agorot;
        body.currency = cur();
        body.orig_minor = sign * origMinor();
        if (ilsTouched) {
          const a = Math.abs(parseAgorot(ils.value));
          if (a === 0) throw new Error("יש להזין סכום בשקלים");
          body.amount_agorot = sign * a;
        } else if (!forceLookup && fxUnchanged()) body.amount_agorot = existing!.amount_agorot;
      }
      if (repeat.checked) {
        const n = Number(months.value);
        if (forever.value === "months" && !(Number.isInteger(n) && n >= 1)) throw new Error("מספר החודשים אינו תקין");
        // A series change starts from the occurrence's own month, whatever date was typed.
        const start = seriesId !== undefined ? month : date.value.slice(0, 7);
        const rule = {
          start_month: start,
          end_month: forever.value === "months" ? addMonths(start, n - 1) : null,
          day: seriesId !== undefined && date.value.slice(0, 7) !== month ? (segment?.day ?? 1) : Number(date.value.slice(8)),
          amount_agorot: sign * abs,
          category_id: catId,
          payment_method_id: body.payment_method_id,
          note: body.note,
        };
        if (seriesId !== undefined) await api.changeRecurring(seriesId, rule);
        else await api.addRecurring(rule);
      } else if (existing) {
        await api.updateTransaction(existing.id, body);
        if (bizPick.value !== ownTag) await api.tagTransactions([existing.id], bizPick.value);
      } else {
        const chosen = bizPick.value;
        try {
          const id = await api.addTransaction(body);
          if (chosen != null) await api.tagTransactions([id], chosen);
        } catch (e3) {
          // No connection: keep the line on this device and send it when the connection is back.
          if (!isNetworkError(e3)) throw e3;
          await enqueue(body, chosen);
          requestPersistence();
        }
      }
      try {
        localStorage.setItem(lastKey(catId), bizPick.value == null ? "" : String(bizPick.value));
      } catch {
        /* storage unavailable */
      }
      showPeriodFor(date.value);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = isNetworkError(e2) ? "אין חיבור: בלי חיבור אפשר רק להוסיף תנועה חדשה (לא חוזרת)" : e2 instanceof Error ? e2.message : String(e2);
      save.disabled = false;
    }
  };
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  trackDialog(dlg);
  // Do not put the cursor (and the keyboard) on a field when the dialog opens.
  dlg.tabIndex = -1;
  dlg.setAttribute("autofocus", "");
  dlg.showModal();
  if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
}

function nav(): HTMLElement {
  const item = (view: View, ico: string, label: string) => {
    const b = el("button", {}, el("span", { className: "ico", textContent: ico }), label);
    if (state.view === view || (view === "settings" && (state.view === "log" || state.view === "categories" || state.view === "businesses")) || (view === "month" && state.view === "business")) b.setAttribute("aria-current", "page");
    b.onclick = () => {
      if (view === "insights") {
        if (state.view === "insights") {
          if (state.insights.cat !== undefined) navigateInsights({}, "push");
          return;
        }
        Object.assign(state.insights, { cat: undefined, sub: undefined });
      }
      go(view);
    };
    return b;
  };
  return el("nav", { className: "nav" }, item("month", "📊", "סיכום"), item("insights", "📈", "תובנות"), item("list", "📋", "תנועות"), item("settings", "⚙️", "הגדרות"));
}

function insightsScreen(): HTMLElement {
  return insightsView({
    txs: state.txs,
    cats: state.cats,
    period: state.period,
    ins: state.insights,
    monthBar,
    navigate: navigateInsights,
    setCmp: (c: Cmp) => ((state.insights.cmp = c), render()),
    setPeriod: (p) => ((state.period = p), render()),
    openList: showCategory,
    openDialog,
  });
}

function businessScreen(): HTMLElement {
  return businessDashboard({
    txs: state.txs,
    cats: state.cats,
    biz: state.biz,
    period: state.period,
    view: state.bizView,
    monthBar,
    setView: (patch) => ((state.bizView = { ...state.bizView, ...patch }), render()),
    openDialog,
    back: () => goBack("month"),
  });
}

let renderedView: View | undefined;
function render() {
  const view = { month: monthView, list: listView, insights: insightsScreen, log: logView, categories: categoriesView, settings: settingsView, business: businessScreen, businesses: () => businessesView(manageCtx()) }[state.view]();
  // A different screen opens at its top, not wherever the previous one was scrolled; re-rendering the same one keeps the position.
  if (renderedView !== state.view) window.scrollTo(0, 0);
  renderedView = state.view;
  const fab = el("button", { className: "fab", title: "הוספת תנועה", textContent: "+" });
  fab.onclick = () => openTransactionDialog();
  const banner = state.offline ? el("div", { className: "banner", role: "status", textContent: "אין חיבור: מוצגים הנתונים האחרונים. תנועות חדשות יישמרו וישלחו כשהחיבור יחזור." }) : null;
  const main = el("main", {}, banner, view);
  main.dataset.view = state.view;
  root.replaceChildren(el("div", { className: "shell" }, nav(), main), fab);
}

/** Send lines saved while offline, oldest first. Stops at the first one the server can't be reached for. */
async function flushQueue() {
  for (const q of await queuedList()) {
    try {
      const id = await api.addTransaction(q.tx);
      if (q.business != null) await api.tagTransactions([id], q.business).catch(() => {});
    } catch (e) {
      if (isNetworkError(e)) return;
      // The server refused this line (it would be refused again): drop it rather than block the rest.
    }
    await dequeue(q.qid);
  }
}

type ServerData = Pick<Snapshot, "cats" | "txs" | "recurring" | "occurrences" | "mirror" | "fx">;
async function fetchAll(): Promise<ServerData & { log?: LogEntry[] }> {
  const [cats, txs, recurring, occurrences, log, mirror, fx] = await Promise.all([
    api.categories(),
    api.transactions(),
    api.recurring(),
    api.occurrences(),
    state.view === "log" ? api.log(undefined, Math.max(LOG_PAGE, state.log.length)) : undefined,
    api.mirrorStatus().catch(() => ({ enabled: false })),
    api.foreign().catch((e) => (isNetworkError(e) ? Promise.reject(e) : undefined)),
  ]);
  return { cats, txs, recurring, occurrences, mirror: mirror.enabled, log, fx };
}

async function load() {
  await flushQueue();
  let data: ServerData & { log?: LogEntry[] };
  try {
    data = await fetchAll();
    state.offline = false;
    void saveSnapshot({ ...data, savedAt: Date.now() });
    requestPersistence();
  } catch (e) {
    // Offline: show what was last seen (if anything) instead of an error.
    const snap = isNetworkError(e) ? await loadSnapshot() : undefined;
    if (!snap) throw e;
    data = snap;
    state.offline = true;
  }
  const { cats, recurring, occurrences, log } = data;
  // Businesses are optional: a failure (or being offline) leaves the last ones in place.
  try {
    state.biz = toBizData(await api.businesses());
  } catch {
    /* keep what we had */
  }
  // Foreign-currency details (fetched with the rest; the server may just have re-priced stand-in rates).
  if (data.fx) state.fx = new Map(data.fx.map((f) => [f.transaction_id, f]));
  const queued = await queuedList();
  // A queued foreign line has no shekel amount yet: the server converts it when it is sent.
  const queuedFx = new Map<number, Fx>();
  const txs: Transaction[] = [
    ...data.txs,
    ...queued.map((q: Queued) => {
      if (q.tx.currency && q.tx.orig_minor != null) queuedFx.set(-q.qid, { transaction_id: -q.qid, currency: q.tx.currency, orig_minor: q.tx.orig_minor, rate: 0, rate_date: q.tx.date, estimated: true });
      return { id: -q.qid, ...q.tx, amount_agorot: q.tx.amount_agorot ?? 0, sync_state: "queued" };
    }),
  ];
  for (const [id, f] of queuedFx) state.fx.set(id, f);
  state.mirror = data.mirror;
  trackSync(txs);
  if (log) {
    state.log = log;
    state.logMore = log.length >= LOG_PAGE;
  }
  state.cats = cats;
  state.recurring = recurring;
  const generated: Transaction[] = occurrences.map((o) => ({
    id: 0,
    date: o.date,
    amount_agorot: o.amount_agorot,
    category_id: o.category_id,
    payment_method_id: o.payment_method_id,
    note: o.note,
    sync_state: "synced",
    recurring_series_id: o.series_id,
    recurring_segment_id: o.segment_id,
    upcoming: true,
  }));
  state.txs = [...txs, ...generated];
  render();
}

/** Note lines that went from in flight to saved (✅ for a moment), and keep checking while any
 * line is still on its way to Paamonim, so the marks update without a manual refresh. */
let syncTimer: ReturnType<typeof setTimeout> | undefined;
let syncPolls = 0;
function trackSync(txs: Transaction[]) {
  const now = Date.now();
  for (const t of txs) {
    const before = state.syncSeen.get(t.id);
    if (before && IN_FLIGHT.has(before) && t.sync_state === "synced") state.justSynced.set(t.id, now + SYNCED_SHOWN_MS);
  }
  state.syncSeen = new Map(txs.map((t) => [t.id, t.sync_state]));
  clearTimeout(syncTimer);
  const waiting = state.mirror && txs.some((t) => IN_FLIGHT.has(t.sync_state));
  syncPolls = waiting ? syncPolls + 1 : 0;
  if (waiting) {
    // Quick at first, then slower if Paamonim takes a while (e.g. waiting for the next sweep).
    const poll = async () => {
      // A refresh is skipped while a dialog is open: try again shortly.
      if (!(await refresh())) syncTimer = setTimeout(poll, 3000);
    };
    syncTimer = setTimeout(poll, syncPolls < 40 ? 1500 : 15000);
  } else if ([...state.justSynced.values()].some((until) => until > now)) {
    // Take the ✅ away once its moment is over.
    const next = Math.min(...[...state.justSynced.values()].filter((u) => u > now));
    syncTimer = setTimeout(() => {
      for (const [id, until] of state.justSynced) if (until <= Date.now()) state.justSynced.delete(id);
      render();
    }, next - now + 50);
  }
}

/** Reload everything without disturbing an open dialog; a failure keeps the data already shown. */
let refreshing: Promise<boolean> | undefined;
function refresh(): Promise<boolean> {
  if (document.querySelector("dialog[open]")) return Promise.resolve(false);
  refreshing ??= load()
    .then(() => true)
    .catch(() => false)
    .finally(() => (refreshing = undefined));
  return refreshing;
}

// An installed PWA is usually resumed from the background, not reloaded, so fetch again whenever it
// becomes visible (also after a bfcache restore or regained connectivity). Throttled against bursts.
let lastLoad = Date.now();
function refreshIfStale() {
  if (document.visibilityState !== "visible" || Date.now() - lastLoad < 5000) return;
  lastLoad = Date.now();
  void refresh();
}
document.addEventListener("visibilitychange", refreshIfStale);
window.addEventListener("pageshow", (e) => e.persisted && refreshIfStale());
window.addEventListener("online", () => void refresh());
window.addEventListener("offline", () => void refresh());

// iOS Safari ignores user-scalable=no and touch-action for pinch, but still fires these gesture events.
for (const g of ["gesturestart", "gesturechange"]) document.addEventListener(g, (e) => e.preventDefault());

// Pull to refresh (touch screens only): the page follows the finger from the top, the spinner rides
// along, a short haptic tick marks the threshold (navigator.vibrate; unavailable on iOS Safari),
// and release glides back with an eased transition.
const PULL_THRESHOLD = 72;
const PULL_MAX = 120;
const spinner = el("div", { className: "ptr" }, el("div", { className: "ptr-dot" }));
document.body.append(spinner);
let pullStart: number | undefined;
let pullDist = 0;
let armed = false;
function setPull(dist: number) {
  const r = document.documentElement.style;
  r.setProperty("--pull", `${dist}px`);
  r.setProperty("--pull-p", String(Math.min(1, dist / PULL_THRESHOLD)));
}
function glideTo(dist: number) {
  root.classList.add("gliding");
  setPull(dist);
}
document.addEventListener(
  "touchstart",
  (e) => {
    pullStart = window.scrollY <= 0 && e.touches.length === 1 && !document.querySelector("dialog[open]") && !refreshing ? e.touches[0].clientY : undefined;
    pullDist = 0;
    armed = false;
    root.classList.remove("gliding");
  },
  { passive: true },
);
document.addEventListener(
  "touchmove",
  (e) => {
    if (pullStart === undefined) return;
    const dy = e.touches[0].clientY - pullStart;
    if (dy <= 0 || window.scrollY > 0) {
      if (pullDist === 0) pullStart = undefined;
      else setPull((pullDist = 0));
      return;
    }
    e.preventDefault();
    // Rubber band: resistance grows as the page is pulled further.
    pullDist = PULL_MAX * (1 - Math.exp(-dy / (PULL_MAX * 1.6)));
    setPull(pullDist);
    if (!armed && pullDist >= PULL_THRESHOLD) navigator.vibrate?.(12);
    armed = pullDist >= PULL_THRESHOLD;
    spinner.classList.toggle("ready", armed);
  },
  { passive: false },
);
async function endPull() {
  if (pullStart === undefined) return;
  pullStart = undefined;
  if (!armed) return glideTo(0);
  spinner.classList.add("spin");
  glideTo(PULL_THRESHOLD * 0.8);
  lastLoad = Date.now();
  const [ok] = await Promise.all([refresh(), new Promise((r) => setTimeout(r, 600))]);
  if (!ok) navigator.vibrate?.([30, 40, 30]);
  spinner.classList.remove("spin", "ready");
  glideTo(0);
  armed = false;
}
document.addEventListener("touchend", endPull);
document.addEventListener("touchcancel", endPull);

root.replaceChildren(el("main", {}, el("div", { className: "empty", textContent: "טוען…" })));
load()
  .then(openShortcutAction)
  .catch((e) => {
    root.replaceChildren(el("main", {}, el("div", { className: "empty error", textContent: `לא ניתן לטעון נתונים: ${e}` })));
  });

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});

/** App-icon shortcuts (see manifest.webmanifest) start the app at /?action=add-expense or /?action=add-income. */
function openShortcutAction() {
  const action = new URLSearchParams(location.search).get("action");
  if (action !== "add-expense" && action !== "add-income") return;
  history.replaceState(screenState({ n: 0 }), "", location.pathname);
  openTransactionDialog(undefined, action === "add-income");
}
