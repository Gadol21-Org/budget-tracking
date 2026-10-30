import "./style.css";
import { contains, MONTHS, monthOf, shift, title as periodTitle, type Period } from "./period";
import { api, formatShekels, parseAgorot, type Category, type Transaction } from "./api";

type View = "month" | "list" | "settings";

const root = document.getElementById("app")!;
const state = {
  view: "month" as View,
  period: monthOf(new Date().toISOString().slice(0, 10)) as Period,
  txs: [] as Transaction[],
  cats: [] as Category[],
  /** Set when a summary row was opened: the list shows only that category (and its sub-categories). */
  /** Which side of the summary's category breakdown is shown. */
  kind: "expense" as "expense" | "income",
  /** Which side of the category manager in settings is shown (true = income). */
  catKind: false,
  filter: undefined as { id: number | null } | undefined,
};

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
            const del = el("button", { className: "del", title: "מחיקה", textContent: "✕" });
            del.onclick = async () => {
              if (!confirm("למחוק את התנועה?")) return;
              await api.deleteTransaction(t.id);
              await load();
            };
            const edit = el("button", { className: "del", title: "עריכה", textContent: "✎" });
            edit.onclick = () => openTransactionDialog(t);
            const badge: Record<string, string> = {
              modified: "נערך – לא עודכן בפעמונים",
              error: "לא נשלח לפעמונים",
            };
            return el(
              "div",
              { className: "row" },
              el(
                "div",
                { className: "grow" },
                el("div", { className: "name", textContent: catPath(t.category_id) }),
                t.note ? el("div", { className: "muted", textContent: t.note }) : null,
                badge[t.sync_state] ? el("div", { className: "muted", style: "font-size:.8rem", textContent: badge[t.sync_state] }) : null,
              ),
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
  return el("div", {}, monthBar(), header, ...(body.length ? body : [el("div", { className: "empty", textContent: "אין תנועות בתקופה זו" })]));
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
    const used = state.txs.filter((t) => t.category_id != null && doomed.has(t.category_id)).length;
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
        ? el("label", {}, `${used} תנועות משויכות לקטגוריה. להעביר אותן אל:`, target)
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
  );
}

function openTransactionDialog(existing?: Transaction) {
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

  const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
  cancel.onclick = () => dlg.close();
  const save = el("button", { type: "submit", className: "btn primary", textContent: existing ? "עדכון" : "שמירה" });
  const form = el(
    "form",
    { className: "add", method: "dialog" },
    el("div", { className: "seg" }, expBtn, incBtn),
    el("label", {}, "סכום (₪)", amount),
    el("label", {}, "קטגוריה", category),
    el("label", {}, "תאריך", date),
    el("label", {}, "הערה", note),
    err,
    el("div", { className: "actions" }, cancel, save),
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
      if (existing) await api.updateTransaction(existing.id, body);
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
    if (state.view === view) b.setAttribute("aria-current", "page");
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
  const view = state.view === "month" ? monthView() : state.view === "list" ? listView() : settingsView();
  const fab = el("button", { className: "fab", title: "הוספת תנועה", textContent: "+" });
  fab.onclick = () => openTransactionDialog();
  root.replaceChildren(el("div", { className: "shell" }, nav(), el("main", {}, view)), fab);
}

async function load() {
  [state.cats, state.txs] = await Promise.all([api.categories(), api.transactions()]);
  render();
}

root.replaceChildren(el("main", {}, el("div", { className: "empty", textContent: "טוען…" })));
load().catch((e) => {
  root.replaceChildren(el("main", {}, el("div", { className: "empty error", textContent: `לא ניתן לטעון נתונים: ${e}` })));
});

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});
