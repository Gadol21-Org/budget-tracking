import "./style.css";
import { contains, MONTHS, monthOf, shift, title as periodTitle, type Period } from "./period";
import { api, formatShekels, parseAgorot, type Category, type LogEntry, type RecurringSegment, type Transaction } from "./api";

type View = "month" | "list" | "settings" | "log";

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
  /** Change log entries loaded so far (newest first) and whether older ones remain. */
  log: [] as LogEntry[],
  logMore: false,
  /** Changes are written to Paamonim (PAAMONIM_MIRROR=1). */
  mirror: false,
  /** sync_state of each transaction at the last load, to notice lines that just got saved. */
  syncSeen: new Map<number, string>(),
  /** Lines just saved in Paamonim, shown with ✅ until this time (ms). */
  justSynced: new Map<number, number>(),
};

/** States of a line whose change is on its way to Paamonim. */
const IN_FLIGHT = new Set(["pending", "modified", "verifying"]);
const SYNCED_SHOWN_MS = 2500;

/** Saving / saved / failed marker of a line in Paamonim; nothing when mirroring is off. */
function syncMark(t: Transaction): HTMLElement | null {
  if (!state.mirror || t.upcoming) return null;
  if (IN_FLIGHT.has(t.sync_state)) return el("span", { className: "sync spinning", title: "שומר בפעמונים…", textContent: "🌀" });
  if (t.sync_state === "error") return el("span", { className: "sync", title: "לא נשמר בפעמונים", textContent: "⚠️" });
  // Can't go to Paamonim as it is (a top-level category made here, a zero amount, a refund on an expense item).
  if (t.sync_state === "skipped") return el("span", { className: "sync", title: "לא נשלח לפעמונים: אין לו מקום שם", textContent: "⚠️" });
  if ((state.justSynced.get(t.id) ?? 0) > Date.now()) return el("span", { className: "sync", title: "נשמר בפעמונים", textContent: "✅" });
  return null;
}

type Child = Node | string | null | false;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Record<string, unknown> = {},
  ...kids: Child[]
): HTMLElementTagNameMap[K] {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...kids.filter((k): k is Node | string => !!k));
  return node;
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

const inPeriod = () => state.txs.filter((t) => contains(state.period, t.date));

function inFilter(t: Transaction): boolean {
  if (!state.filter) return true;
  const id = state.filter.id;
  return t.category_id === id || (id !== null && catById(t.category_id)?.parent_id === id);
}

/** Make a row act as a button: click, Enter and Space. */
function clickable<T extends HTMLElement>(node: T, onActivate: () => void): T {
  node.classList.add("clickable");
  node.tabIndex = 0;
  node.setAttribute("role", "button");
  node.onclick = onActivate;
  node.onkeydown = (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onActivate();
    }
  };
  return node;
}

function showCategory(id: number | null) {
  state.filter = { id };
  state.view = "list";
  render();
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
          : [labelled("מתאריך", from), labelled("עד תאריך", to)]),
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
  dlg.showModal();
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
  const txs = inPeriod().filter(inFilter);
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
                await api.deleteTransaction(t.id);
                await load();
              };
            const edit = el("button", { className: "del", title: "עריכה", textContent: "✎" });
            edit.onclick = () => openTransactionDialog(t);
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
                t.note ? el("div", { className: "muted", textContent: t.note }) : null,
              ),
              syncMark(t),
              money(t.amount_agorot, t.amount_agorot < 0 ? "expense" : "income"),
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
      state.filter = undefined;
      state.view = "month";
      render();
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
  return el("div", {}, header, monthBar(), ...(body.length ? body : [el("div", { className: "empty", textContent: "אין תנועות בתקופה זו" })]));
}

/** Open a modal built by `build`, which gets a way to close it. */
function openDialog(build: (close: () => void) => HTMLElement) {
  const dlg = el("dialog");
  dlg.append(build(() => dlg.close()));
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  dlg.showModal();
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
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "הוספה" });
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: isIncome ? "הוספת קטגוריית הכנסה" : "הוספת קטגוריית הוצאה" }),
      el("label", {}, "שם", name),
      el("label", {}, "סוג", parent),
      el("div", { className: "note muted", textContent: "קטגוריה שנוספה כאן קיימת רק באפליקציה: תנועות עליה לא יועתקו לפעמונים." }),
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        await api.addCategory({ name: name.value, is_income: isIncome, parent_id: parent.value ? Number(parent.value) : null });
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        save.disabled = false;
      }
    };
    setTimeout(() => name.focus());
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
  const row = (c: Category, sub: boolean) =>
    el(
      "div",
      { className: sub ? "row sub" : "row" },
      el("div", { className: "grow" }, el("div", { className: "name", textContent: c.name })),
      !sub && iconBtn("+", "הוספת תת-קטגוריה", () => openAddCategoryDialog(c.is_income, c.id)),
      iconBtn("🗑", "מחיקה", () => openDeleteCategoryDialog(c)),
    );
  const rows = ofKind
    .filter((c) => c.parent_id == null)
    .flatMap((c) => [row(c, false), ...ofKind.filter((k) => k.parent_id === c.id).map((k) => row(k, true))]);
  return el(
    "div",
    { className: "card" },
    el("h2", { textContent: "קטגוריות" }),
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
    categoriesCard(),
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
  if (key === "date") return new Date(`${v}T00:00:00`).toLocaleDateString("he-IL");
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
  if (e.old && e.new && ["transaction_update", "recurring_update", "recurring_override"].includes(e.kind)) {
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
    state.view = "settings";
    render();
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
  state.view = "log";
  render();
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

const monthsBetween = (from: string, to: string) => (Number(to.slice(0, 4)) - Number(from.slice(0, 4))) * 12 + Number(to.slice(5)) - Number(from.slice(5));
const addMonths = (month: string, n: number) => new Date(Date.UTC(Number(month.slice(0, 4)), Number(month.slice(5)) - 1 + n, 1)).toISOString().slice(0, 7);

/**
 * Add or edit a transaction. An `existing` generated occurrence of a repeating transaction edits
 * the series from that occurrence's month on; earlier months are left as they were.
 */
function openTransactionDialog(existing?: Transaction) {
  const seriesId = existing?.upcoming ? existing.recurring_series_id! : undefined;
  const segment = state.recurring.find((s) => s.id === existing?.recurring_segment_id);
  /** The month of the occurrence being edited; the date field may move the charge out of it. */
  const month = existing?.date.slice(0, 7) ?? "";
  let isIncome = existing ? existing.amount_agorot > 0 : false;
  const dlg = el("dialog");
  const amount = el("input", {
    type: "text",
    inputMode: "decimal",
    placeholder: "0.00",
    required: true,
    autocomplete: "off",
    value: existing ? String(Math.abs(existing.amount_agorot) / 100) : "",
  });
  const date = el("input", { type: "date", required: true, value: existing?.date ?? new Date().toISOString().slice(0, 10) });
  const category = el("select");
  const note = el("input", { type: "text", placeholder: "אופציונלי", maxLength: 35, value: existing?.note ?? "" });
  const err = el("div", { className: "error" });

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

  const expBtn = el("button", { type: "button", textContent: "הוצאה" });
  const incBtn = el("button", { type: "button", textContent: "הכנסה" });
  const fillCats = () => {
    expBtn.setAttribute("aria-pressed", String(!isIncome));
    incBtn.setAttribute("aria-pressed", String(isIncome));
    const keep = category.value || (existing?.category_id != null ? String(existing.category_id) : "");
    const opt = (c: Category) => el("option", { value: String(c.id), textContent: c.name });
    const ofKind = state.cats.filter((c) => c.is_income === isIncome);
    const tops = ofKind.filter((c) => c.parent_id == null);
    category.replaceChildren(
      el("option", { value: "", textContent: "ללא קטגוריה" }),
      ...tops.map((c) => {
        const kids = ofKind.filter((k) => k.parent_id === c.id);
        // A parent is only a heading; transactions go on its sub-categories.
        return kids.length ? el("optgroup", { label: c.name }, ...kids.map(opt)) : opt(c);
      }),
    );
    category.value = keep;
  };
  expBtn.onclick = () => ((isIncome = false), fillCats());
  incBtn.onclick = () => ((isIncome = true), fillCats());
  fillCats();

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
        category_id: category.value ? Number(category.value) : null,
        payment_method_id: existing?.payment_method_id ?? null,
        note: note.value,
      });
      state.period = monthOf(date.value);
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
    el("label", {}, "סכום (₪)", amount),
    el("label", {}, "קטגוריה", category),
    el("label", {}, "תאריך", date),
    el("label", {}, "הערה", note),
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
  form.onsubmit = async (e) => {
    e.preventDefault();
    try {
      const abs = Math.abs(parseAgorot(amount.value));
      if (abs === 0) throw new Error("יש להזין סכום");
      save.disabled = true;
      const body = {
        date: date.value,
        amount_agorot: isIncome ? abs : -abs,
        category_id: category.value ? Number(category.value) : null,
        payment_method_id: existing?.payment_method_id ?? null,
        note: note.value,
      };
      if (repeat.checked) {
        const n = Number(months.value);
        if (forever.value === "months" && !(Number.isInteger(n) && n >= 1)) throw new Error("מספר החודשים אינו תקין");
        // A series change starts from the occurrence's own month, whatever date was typed.
        const start = seriesId !== undefined ? month : date.value.slice(0, 7);
        const rule = {
          start_month: start,
          end_month: forever.value === "months" ? addMonths(start, n - 1) : null,
          day: seriesId !== undefined && date.value.slice(0, 7) !== month ? (segment?.day ?? 1) : Number(date.value.slice(8)),
          amount_agorot: body.amount_agorot,
          category_id: body.category_id,
          payment_method_id: body.payment_method_id,
          note: body.note,
        };
        if (seriesId !== undefined) await api.changeRecurring(seriesId, rule);
        else await api.addRecurring(rule);
      } else if (existing) await api.updateTransaction(existing.id, body);
      else await api.addTransaction(body);
      state.period = monthOf(date.value);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = e2 instanceof Error ? e2.message : String(e2);
      save.disabled = false;
    }
  };
  dlg.append(form);
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  dlg.showModal();
  amount.focus();
}

function nav(): HTMLElement {
  const item = (view: View, ico: string, label: string) => {
    const b = el("button", {}, el("span", { className: "ico", textContent: ico }), label);
    if (state.view === view || (view === "settings" && state.view === "log")) b.setAttribute("aria-current", "page");
    b.onclick = () => {
      state.view = view;
      state.filter = undefined;
      render();
    };
    return b;
  };
  return el("nav", { className: "nav" }, item("month", "📊", "סיכום"), item("list", "📋", "תנועות"), item("settings", "⚙️", "הגדרות"));
}

function render() {
  const view = state.view === "month" ? monthView() : state.view === "list" ? listView() : state.view === "log" ? logView() : settingsView();
  const fab = el("button", { className: "fab", title: "הוספת תנועה", textContent: "+" });
  fab.onclick = () => openTransactionDialog();
  root.replaceChildren(el("div", { className: "shell" }, nav(), el("main", {}, view)), fab);
}

async function load() {
  const [cats, txs, recurring, occurrences, log, mirror] = await Promise.all([
    api.categories(),
    api.transactions(),
    api.recurring(),
    api.occurrences(),
    state.view === "log" ? api.log(undefined, Math.max(LOG_PAGE, state.log.length)) : undefined,
    api.mirrorStatus().catch(() => ({ enabled: false })),
  ]);
  state.mirror = mirror.enabled;
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
window.addEventListener("online", refreshIfStale);

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
load().catch((e) => {
  root.replaceChildren(el("main", {}, el("div", { className: "empty error", textContent: `לא ניתן לטעון נתונים: ${e}` })));
});

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});
