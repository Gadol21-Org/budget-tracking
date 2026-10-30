import "./style.css";
import { api, formatShekels, parseAgorot, type Category, type Transaction } from "./api";

type View = "month" | "list" | "settings";

const root = document.getElementById("app")!;
const state = {
  view: "month" as View,
  month: new Date().toISOString().slice(0, 7), // YYYY-MM
  txs: [] as Transaction[],
  cats: [] as Category[],
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

function shiftMonth(m: string, delta: number): string {
  const [y, mo] = m.split("-").map(Number);
  const d = new Date(Date.UTC(y, mo - 1 + delta, 1));
  return d.toISOString().slice(0, 7);
}
const monthTitle = (m: string) =>
  new Date(`${m}-01T00:00:00`).toLocaleDateString("he-IL", { month: "long", year: "numeric" });
const dayTitle = (d: string) =>
  new Date(`${d}T00:00:00`).toLocaleDateString("he-IL", { weekday: "long", day: "numeric", month: "long" });

const inMonth = () => state.txs.filter((t) => t.date.startsWith(state.month));

function monthBar(): HTMLElement {
  // In the RTL layout the first element sits on the right: → goes forward, ← goes back.
  const next = el("button", { className: "iconbtn", title: "החודש הבא", textContent: "→" });
  const prev = el("button", { className: "iconbtn", title: "החודש הקודם", textContent: "←" });
  next.onclick = () => ((state.month = shiftMonth(state.month, 1)), render());
  prev.onclick = () => ((state.month = shiftMonth(state.month, -1)), render());
  return el("div", { className: "monthbar" }, next, el("div", { className: "title", textContent: monthTitle(state.month) }), prev);
}

function monthView(): HTMLElement {
  const txs = inMonth();
  const income = txs.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
  const expense = txs.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);
  const stat = (label: string, value: HTMLElement) =>
    el("div", { className: "card" }, el("div", { className: "label", textContent: label }), el("div", { className: "value" }, value));

  // Group expenses by top-level category; sub-categories are listed under their parent.
  const groups = new Map<number | null, { total: number; kids: Map<number | null, number> }>();
  for (const t of txs.filter((t) => t.amount_agorot < 0)) {
    const own = catById(t.category_id);
    const top = own?.parent_id ?? t.category_id;
    const g = groups.get(top) ?? { total: 0, kids: new Map() };
    g.total += -t.amount_agorot;
    if (top !== t.category_id) g.kids.set(t.category_id, (g.kids.get(t.category_id) ?? 0) + -t.amount_agorot);
    groups.set(top, g);
  }
  const rows = [...groups.entries()].sort((a, b) => b[1].total - a[1].total);
  const max = rows[0]?.[1].total ?? 1;

  return el(
    "div",
    {},
    monthBar(),
    el("div", { className: "stats" }, stat("הכנסות", money(income, "income")), stat("הוצאות", money(-expense, "expense")), stat("יתרה", money(income + expense, income + expense < 0 ? "expense" : "income"))),
    el("h2", { textContent: "הוצאות לפי קטגוריה" }),
    rows.length === 0
      ? el("div", { className: "empty", textContent: "אין הוצאות בחודש זה" })
      : el(
          "div",
          { className: "card rows" },
          ...rows.map(([id, g]) =>
            el(
              "div",
              {},
              el(
                "div",
                { className: "row" },
                el("div", { className: "grow" }, el("div", { className: "name", textContent: catName(id) }), el("div", { className: "bar" }, el("span", { style: `width:${Math.max(3, (g.total / max) * 100)}%` }))),
                money(g.total),
              ),
              ...[...g.kids.entries()]
                .sort((a, b) => b[1] - a[1])
                .map(([kid, total]) =>
                  el("div", { className: "row sub" }, el("div", { className: "grow muted", textContent: catName(kid) }), money(total)),
                ),
            ),
          ),
        ),
  );
}

function listView(): HTMLElement {
  const txs = inMonth();
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
  return el("div", {}, monthBar(), ...(body.length ? body : [el("div", { className: "empty", textContent: "אין תנועות בחודש זה" })]));
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
      state.month = date.value.slice(0, 7);
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
    b.onclick = () => ((state.view = view), render());
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

load().catch((e) => {
  root.replaceChildren(el("main", {}, el("div", { className: "empty error", textContent: `לא ניתן לטעון נתונים: ${e}` })));
});

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});
