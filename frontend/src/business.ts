import { formatShekels, formatShekelsWhole, shortMonth } from "./format";
import { api, type Business, type BusinessData, type Category, type Transaction } from "./api";
import { clickable, el, type Child } from "./ui";
import { contains, shift, title as periodTitle, type Period } from "./period";

/**
 * Businesses are a lens over the household's own transactions: nothing is moved or removed from
 * the household figures. A transaction counts in the business it is tagged with, or else the
 * business its category (or its parent category) is linked to.
 */
export interface BizData {
  businesses: Business[];
  /** transaction id -> business id */
  tx: Map<number, number>;
  /** category id -> business id */
  cat: Map<number, number>;
  /** categories whose new transactions must say which business they belong to */
  required: Set<number>;
}

export const emptyBiz = (): BizData => ({ businesses: [], tx: new Map(), cat: new Map(), required: new Set() });

export const toBizData = (d: BusinessData): BizData => ({
  businesses: d.businesses,
  tx: new Map(d.links.transactions),
  cat: new Map(d.links.categories),
  required: new Set(d.links.required),
});

/** The business a category is linked to, directly or through its parent. */
export function categoryBusiness(d: BizData, cats: Category[], categoryId: number | null): number | undefined {
  const c = cats.find((x) => x.id === categoryId);
  if (!c) return undefined;
  return d.cat.get(c.id) ?? (c.parent_id != null ? d.cat.get(c.parent_id) : undefined);
}

/** The business a transaction counts in: its own tag, else its category's link. */
export function businessOf(d: BizData, cats: Category[], t: Transaction): number | undefined {
  const own = t.id > 0 ? d.tx.get(t.id) : undefined;
  return own ?? categoryBusiness(d, cats, t.category_id);
}

/** Whether a new line in this category must say which business it belongs to (and none is given already). */
export function businessRequired(d: BizData, cats: Category[], categoryId: number | null): boolean {
  const c = cats.find((x) => x.id === categoryId);
  if (!c || categoryBusiness(d, cats, c.id) != null) return false;
  return d.required.has(c.id) || (c.parent_id != null && d.required.has(c.parent_id));
}

export const bizName = (d: BizData, id: number | undefined | null) => d.businesses.find((b) => b.id === id)?.name ?? "";

// ---- Dashboards ----------------------------------------------------------------------------------

export type BizTab = "overview" | "trends" | "cats" | "compare";
export interface BizView {
  tab: BizTab;
  /** "all" = the whole business, or one business. */
  scope: "all" | number;
}

export interface BizCtx {
  txs: Transaction[];
  cats: Category[];
  biz: BizData;
  period: Period;
  view: BizView;
  monthBar: () => HTMLElement;
  setView: (patch: Partial<BizView>) => void;
  openDialog: (build: (close: () => void) => HTMLElement) => void;
  back: () => void;
}

const TABS: [BizTab, string][] = [
  ["overview", "סקירה"],
  ["trends", "מגמות"],
  ["cats", "קטגוריות"],
  ["compare", "השוואה"],
];

const money = (a: number, cls = "") => el("span", { className: `num ${cls}`.trim(), textContent: formatShekelsWhole(a) });
const signed = (a: number) => money(a, a < 0 ? "expense" : "income");

interface Fig {
  income: number;
  expense: number;
  profit: number;
}
const figures = (ts: Transaction[]): Fig => {
  const income = ts.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
  const expense = -ts.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);
  return { income, expense, profit: income - expense };
};

/** Transactions that count in the business `scope` ("all" = in any business). */
const scoped = (ctx: Pick<BizCtx, "txs" | "cats" | "biz">, scope: "all" | number) =>
  ctx.txs.filter((t) => {
    const b = businessOf(ctx.biz, ctx.cats, t);
    return b != null && (scope === "all" || b === scope);
  });

/** The six months ending with the period's last month, oldest first. */
function lastMonths(p: Period, n = 6): string[] {
  const end = p.kind === "month" ? p.month : p.kind === "year" ? `${p.year}-12` : p.to.slice(0, 7);
  const [y, m] = end.split("-").map(Number);
  return Array.from({ length: n }, (_, i) => new Date(Date.UTC(y, m - 1 - (n - 1 - i), 1)).toISOString().slice(0, 7));
}

const scopeName = (ctx: Pick<BizCtx, "biz">, scope: "all" | number) => (scope === "all" ? "כל העסק" : bizName(ctx.biz, scope));

/** Costs shared by the businesses, split between the others by their share of the income. */
function allocation(ctx: BizCtx, p: Period, id: number): number | null {
  const general = ctx.biz.businesses.find((b) => b.is_general);
  if (!general || general.id === id) return null;
  const inP = (b: number) => figures(scoped(ctx, b).filter((t) => contains(p, t.date)));
  const cost = -inP(general.id).profit;
  const others = ctx.biz.businesses.filter((b) => !b.is_general);
  const total = others.reduce((s, b) => s + inP(b.id).income, 0);
  if (cost === 0 || total <= 0) return null;
  return Math.round((cost * inP(id).income) / total);
}

function delta(label: string, diff: number): HTMLElement {
  return el("div", { className: "row" }, el("div", { className: "grow", textContent: label }), el("span", { className: `num ${diff < 0 ? "expense" : "income"}`, textContent: `${diff < 0 ? "▼" : "▲"} ${formatShekelsWhole(Math.abs(diff))}` }));
}

function hero(label: string, f: Fig): HTMLElement {
  const margin = f.income > 0 ? Math.round((f.profit / f.income) * 100) : null;
  return el(
    "div",
    { className: "bizhero" },
    el("div", { className: "small", textContent: label }),
    el("div", { className: "big num", textContent: formatShekelsWhole(f.profit) }),
    el("div", { className: "sub" }, el("span", { textContent: `הכנסות ${formatShekelsWhole(f.income)}` }), el("span", { textContent: `הוצאות ${formatShekelsWhole(f.expense)}` }), margin != null ? el("span", { textContent: `שולי רווח ${margin}%` }) : null),
  );
}

/** Income against expenses per month, with the profit above each pair. */
function chart(ctx: BizCtx, scope: "all" | number): HTMLElement {
  const months = lastMonths(ctx.period);
  const ts = scoped(ctx, scope);
  const data = months.map((m) => figures(ts.filter((t) => t.date.startsWith(m))));
  const max = Math.max(1, ...data.flatMap((d) => [d.income, d.expense]));
  return el(
    "div",
    { className: "card" },
    el(
      "div",
      { className: "bizchart" },
      ...data.map((d, i) =>
        el(
          "div",
          { className: "bizcol" },
          el("div", { className: `bizp num ${d.profit < 0 ? "expense" : "income"}`, textContent: `${d.profit < 0 ? "-" : ""}${(Math.abs(d.profit) / 100000).toFixed(1)}k` }),
          el("div", { className: "bizpair" }, el("i", { className: "in", style: `height:${(d.income / max) * 100}%` }), el("i", { className: "out", style: `height:${(d.expense / max) * 100}%` })),
          el("div", { className: "bizm muted", textContent: shortMonth(months[i]) }),
        ),
      ),
    ),
    el("div", { className: "bizlegend muted" }, el("span", {}, el("i", { className: "in" }), "הכנסות"), el("span", {}, el("i", { className: "out" }), "הוצאות"), el("span", { textContent: "מעל העמודות: רווח באלפי ₪" })),
  );
}

function overview(ctx: BizCtx): Child[] {
  const { scope } = ctx.view;
  const ts = scoped(ctx, scope);
  const cur = figures(ts.filter((t) => contains(ctx.period, t.date)));
  const out: Child[] = [hero(`רווח ${scopeName(ctx, scope)}`, cur)];
  const rows: HTMLElement[] = [];
  if (cur.expense > 0 && cur.income > 0) rows.push(el("div", { className: "row" }, el("div", { className: "grow", textContent: "על כל ₪1 הוצאה מרוויחים" }), el("b", { className: "num", textContent: `${(cur.income / cur.expense).toFixed(2)}₪` })));
  const prevP = shift(ctx.period, -1);
  if (prevP) {
    const prev = figures(ts.filter((t) => contains(prevP, t.date)));
    rows.push(delta(ctx.period.kind === "year" ? "לעומת השנה הקודמת" : "לעומת התקופה הקודמת", cur.profit - prev.profit));
    if (ctx.period.kind === "month") {
      const avg = [1, 2, 3].reduce((s, k) => s + figures(ts.filter((t) => contains(shift(ctx.period, -k)!, t.date))).profit, 0) / 3;
      rows.push(delta("לעומת ממוצע 3 חודשים", Math.round(cur.profit - avg)));
    }
  }
  if (rows.length) out.push(el("div", { className: "card rows" }, ...rows));
  if (scope === "all") {
    const list = ctx.biz.businesses.map((b) => ({ b, f: figures(scoped(ctx, b.id).filter((t) => contains(ctx.period, t.date))) }));
    out.push(
      el("h2", { textContent: "לפי עסק" }),
      el(
        "div",
        { className: "card" },
        el(
          "table",
          { className: "biztable" },
          el("tr", {}, ...["עסק", "הכנסות", "הוצאות", "רווח"].map((h) => el("th", { className: "muted", textContent: h }))),
          ...list.map(({ b, f }) => {
            const tr = el("tr", {}, el("td", { textContent: `${b.is_general ? `${b.name} (משותף)` : b.name} ›` }), el("td", {}, money(f.income)), el("td", {}, money(f.expense)), el("td", {}, signed(f.profit)));
            clickable(tr as unknown as HTMLElement, () => ctx.setView({ scope: b.id }));
            return tr;
          }),
          el("tr", { className: "total" }, el("td", { textContent: "סך הכול" }), el("td", {}, money(cur.income)), el("td", {}, money(cur.expense)), el("td", {}, signed(cur.profit))),
        ),
      ),
    );
  } else {
    const share = allocation(ctx, ctx.period, scope);
    if (share != null)
      out.push(el("div", { className: "card rows" }, el("div", { className: "row" }, el("div", { className: "grow", textContent: "חלק מההוצאות המשותפות (לפי הכנסות)" }), signed(-share)), el("div", { className: "row" }, el("div", { className: "grow name", textContent: "רווח אחרי חלוקה" }), signed(cur.profit - share))));
  }
  if (!ts.length) out.push(el("div", { className: "empty", textContent: "עוד אין תנועות בעסק. תייגו תנועה, או קשרו קטגוריה לעסק." }));
  return out;
}

function trends(ctx: BizCtx): Child[] {
  const { scope } = ctx.view;
  const months = lastMonths(ctx.period);
  const ts = scoped(ctx, scope);
  const rows = months.map((m) => ({ m, f: figures(ts.filter((t) => t.date.startsWith(m))) })).reverse();
  const sum = figures(ts.filter((t) => months.some((m) => t.date.startsWith(m))));
  return [
    el("h2", { textContent: "הכנסות מול הוצאות · 6 חודשים" }),
    chart(ctx, scope),
    el(
      "div",
      { className: "card" },
      el(
        "table",
        { className: "biztable" },
        el("tr", {}, ...["חודש", "הכנסות", "הוצאות", "רווח"].map((h) => el("th", { className: "muted", textContent: h }))),
        ...rows.map(({ m, f }) => el("tr", {}, el("td", { textContent: periodTitle({ kind: "month", month: m }) }), el("td", {}, money(f.income)), el("td", {}, money(f.expense)), el("td", {}, signed(f.profit)))),
        el("tr", { className: "total" }, el("td", { textContent: "סה״כ" }), el("td", {}, money(sum.income)), el("td", {}, money(sum.expense)), el("td", {}, signed(sum.profit))),
      ),
    ),
  ];
}

function categories(ctx: BizCtx): Child[] {
  const { scope } = ctx.view;
  const mine = scoped(ctx, scope).filter((t) => contains(ctx.period, t.date));
  const all = ctx.txs.filter((t) => contains(ctx.period, t.date));
  const top = (t: Transaction) => {
    const c = ctx.cats.find((x) => x.id === t.category_id);
    return c?.parent_id ?? t.category_id;
  };
  const name = (id: number | null) => ctx.cats.find((c) => c.id === id)?.name ?? "ללא קטגוריה";
  const side = (sign: 1 | -1, title: string, empty: string): Child[] => {
    const of = (ts: Transaction[]) => {
      const m = new Map<number | null, number>();
      for (const t of ts.filter((t) => t.amount_agorot * sign > 0)) m.set(top(t), (m.get(top(t)) ?? 0) + t.amount_agorot * sign);
      return m;
    };
    const m = of(mine);
    const whole = of(all);
    const rows = [...m.entries()].sort((a, b) => b[1] - a[1]);
    const total = rows.reduce((s, [, v]) => s + v, 0);
    const max = rows[0]?.[1] ?? 1;
    return [
      el("h2", { textContent: `${title} לפי קטגוריה · ${formatShekelsWhole(total)}` }),
      rows.length
        ? el(
            "div",
            { className: "card rows" },
            ...rows.map(([id, v]) => {
              const part = whole.get(id) ?? 0;
              return el("div", { className: "row" }, el("div", { className: "grow" }, el("div", { className: "name", textContent: name(id) }), el("div", { className: "bar" }, el("span", { style: `width:${Math.max(3, (v / max) * 100)}%` })), part > v ? el("div", { className: "muted small", textContent: `${formatShekelsWhole(v)} מתוך ${formatShekelsWhole(part)} בקטגוריה` }) : null), money(v));
            }),
          )
        : el("div", { className: "empty", textContent: empty }),
    ];
  };
  return [...side(-1, "הוצאות", "אין הוצאות בעסק בתקופה זו"), ...side(1, "הכנסות", "אין הכנסות בעסק בתקופה זו")];
}

function compare(ctx: BizCtx): Child[] {
  const months = lastMonths(ctx.period);
  const rows = ctx.biz.businesses.map((b) => {
    const ts = scoped(ctx, b.id);
    const series = months.map((m) => figures(ts.filter((t) => t.date.startsWith(m))).profit);
    const cur = figures(ts.filter((t) => contains(ctx.period, t.date)));
    const max = Math.max(1, ...series.map(Math.abs));
    const margin = cur.income > 0 ? `שולי רווח ${Math.round((cur.profit / cur.income) * 100)}%` : "אין הכנסות";
    const row = el(
      "div",
      { className: "row" },
      el("div", { className: "grow" }, el("div", { className: "name", textContent: b.is_general ? `${b.name} (משותף)` : b.name }), el("div", { className: "muted small", textContent: margin })),
      el("div", { className: "bizmini" }, ...series.map((v) => el("i", { className: v < 0 ? "neg" : "", style: `height:${Math.max(6, (Math.abs(v) / max) * 100)}%` }))),
      el("div", { className: "bizamt" }, signed(cur.profit)),
    );
    return clickable(row, () => ctx.setView({ tab: "overview", scope: b.id }));
  });
  return [
    el("h2", { textContent: "רווח לפי עסק · 6 חודשים" }),
    el("div", { className: "card rows" }, ...rows),
    el("div", { className: "note muted", textContent: "ההוצאות המשותפות מוצגות בנפרד ולא מחולקות. בסקירה של כל עסק אפשר לראות את הרווח אחרי חלוקה." }),
  ];
}

/** The business dashboards: scope (the whole business or one of its businesses) and four tabs. */
export function businessDashboard(ctx: BizCtx): HTMLElement {
  const back = el("button", { className: "btn", textContent: "→ סיכום", title: "חזרה לסיכום" });
  back.onclick = ctx.back;
  const scopeBtn = el("button", { className: "btn", textContent: `${scopeName(ctx, ctx.view.scope)} ▾`, title: "בחירת עסק" });
  scopeBtn.onclick = () =>
    ctx.openDialog((close) => {
      const item = (label: string, scope: "all" | number, cls = "") => {
        const b = el("button", { type: "button", className: `scopeitem ${cls}`.trim(), textContent: `${label}${scope === ctx.view.scope ? "  ✓" : ""}` });
        b.onclick = () => (close(), ctx.setView({ scope }));
        return b;
      };
      return el("div", { className: "add" }, el("h2", { textContent: "הצגת נתונים של" }), item("🏢 כל העסק", "all"), ...ctx.biz.businesses.filter((b) => !b.is_general).map((b) => item(b.name, b.id, "ind")), ...ctx.biz.businesses.filter((b) => b.is_general).map((b) => item(`${b.name} (משותף)`, b.id, "ind")));
    });
  const tabs = el(
    "div",
    { className: "seg4" },
    ...TABS.map(([k, label]) => {
      const b = el("button", { type: "button", textContent: label });
      b.setAttribute("aria-pressed", String(ctx.view.tab === k));
      b.onclick = () => ctx.setView({ tab: k });
      return b;
    }),
  );
  const body = { overview, trends, cats: categories, compare }[ctx.view.tab](ctx);
  return el("div", { className: "bizdash" }, el("div", { className: "card filterbar" }, back, el("div", { className: "grow" }, el("div", { className: "name", textContent: "העסק" })), scopeBtn), tabs, ctx.monthBar(), ...body);
}

/** Profit of every business in the period, for the card on the summary. */
export function summaryFigures(ctx: Pick<BizCtx, "txs" | "cats" | "biz" | "period">): { total: Fig; per: { b: Business; f: Fig }[] } {
  const per = ctx.biz.businesses.map((b) => ({ b, f: figures(scoped(ctx, b.id).filter((t) => contains(ctx.period, t.date))) }));
  return { total: figures(scoped(ctx, "all").filter((t) => contains(ctx.period, t.date))), per };
}

// ---- Choosing a business ------------------------------------------------------------------------------

/**
 * One-tap buttons for choosing a business (or none). Most lines are not business lines, so with
 * `collapsed` the buttons stay behind a small link until asked for (or a choice is already made).
 * It can also show the business a category already gives, with nothing to choose.
 */
export function businessChips(biz: BizData, initial: number | null, collapsed = false) {
  /** undefined = nothing chosen yet (only while a choice is required). */
  let value: number | null | undefined = initial;
  let required = false;
  let onChange = () => {};
  let expanded = !collapsed;
  let given: string | null = null;
  const root = el("div", { className: "bizpick" });
  const head = el("div", { className: "muted small", textContent: "עסק" });
  const toggle = el("button", { type: "button", className: "linkbtn", textContent: "+ סימון לעסק" });
  const row = el("div", { className: "qchips" });
  const givenChip = el("div", {});
  toggle.onclick = () => ((expanded = true), draw());
  root.append(toggle, head, row, givenChip);
  const draw = () => {
    const showRow = given == null && (expanded || required || value != null);
    toggle.hidden = given != null || showRow;
    head.hidden = !(showRow || given != null);
    head.textContent = required && given == null ? "עסק (חובה לבחור)" : "עסק";
    row.hidden = !showRow;
    givenChip.hidden = given == null;
    givenChip.replaceChildren(...(given != null ? [el("span", { className: "bizchip big", textContent: given }), el("span", { className: "muted small", textContent: " לפי הקטגוריה" })] : []));
    row.replaceChildren(
      ...[{ id: null as number | null, name: "ללא" }, ...biz.businesses.map((b) => ({ id: b.id as number | null, name: b.name }))].map((o) => {
        const b = el("button", { type: "button", className: "qchip", textContent: o.name });
        b.setAttribute("aria-pressed", String(o.id === value));
        b.onclick = () => ((value = o.id), draw(), onChange());
        return b;
      }),
    );
  };
  draw();
  return {
    root,
    get value(): number | null {
      return value ?? null;
    },
    /** A choice was made (None counts). Always true unless a choice is required and still open. */
    get chosen() {
      return value !== undefined;
    },
    set(v: number | null) {
      value = v;
      draw();
    },
    /** A line must say which business it is in: the buttons show with nothing chosen. */
    setRequired(on: boolean) {
      if (on === required) return;
      required = on;
      value = on ? undefined : null;
      draw();
    },
    get required() {
      return required;
    },
    /** Forget the choice (when the category changes and a new one is needed). */
    clear() {
      if (required) {
        value = undefined;
        draw();
      }
    },
    set onchange(f: () => void) {
      onChange = f;
    },
    /** Show the business the category already gives (no choice), or null to go back to the choice. */
    lock(name: string | null) {
      given = name;
      draw();
    },
  };
}

// ---- Managing businesses ------------------------------------------------------------------------------

export interface ManageCtx {
  biz: BizData;
  cats: Category[];
  txs: Transaction[];
  openDialog: (build: (close: () => void) => HTMLElement) => void;
  reload: () => Promise<void>;
  back: () => void;
}

function nameDialog(ctx: ManageCtx, existing?: Business) {
  ctx.openDialog((close) => {
    const name = el("input", { type: "text", required: true, maxLength: 60, autocomplete: "off", value: existing?.name ?? "" });
    const hasGeneral = ctx.biz.businesses.some((b) => b.is_general);
    const general = el("input", { type: "checkbox", disabled: hasGeneral });
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: existing ? "שמירה" : "הוספה" });
    const sync = () => (save.disabled = !name.value.trim() || name.value.trim() === existing?.name);
    name.oninput = sync;
    sync();
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: existing ? "שינוי שם" : "עסק חדש" }),
      el("label", {}, "שם", name),
      !existing && el("label", { style: "flex-direction:row;align-items:center;gap:8px" }, general, hasGeneral ? "כבר יש עסק משותף" : "הוצאות משותפות (לא שייכות לעסק מסוים)"),
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        if (existing) await api.renameBusiness(existing.id, name.value.trim());
        else await api.addBusiness(name.value.trim(), general.checked);
        close();
        await ctx.reload();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        sync();
      }
    };
    setTimeout(() => name.focus());
    return form;
  });
}

export function businessesView(ctx: ManageCtx): HTMLElement {
  const back = el("button", { className: "btn", textContent: "→ הגדרות", title: "חזרה להגדרות" });
  back.onclick = ctx.back;
  const btn = (text: string, title: string, on: () => void) => {
    const b = el("button", { type: "button", className: "btn", textContent: text, title });
    b.onclick = on;
    return b;
  };
  const tagged = (id: number) => ctx.txs.filter((t) => businessOf(ctx.biz, ctx.cats, t) === id && !t.upcoming).length;
  const rows = ctx.biz.businesses.map((b) =>
    el(
      "div",
      { className: "row" },
      el("div", { className: "grow" }, el("div", { className: "name", textContent: b.is_general ? `${b.name} (משותף)` : b.name }), el("div", { className: "muted", textContent: `${tagged(b.id)} תנועות` })),
      btn("✎", "שינוי שם", () => nameDialog(ctx, b)),
      btn("🗑", "מחיקה", async () => {
        if (!confirm(`למחוק את "${b.name}"? התנועות נשארות, רק הסימון שלהן לעסק יוסר.`)) return;
        await api.deleteBusiness(b.id);
        await ctx.reload();
      }),
    ),
  );
  return el(
    "div",
    {},
    el("div", { className: "actions", style: "justify-content:flex-start" }, back),
    el("h1", { textContent: "עסקים" }),
    el(
      "div",
      { className: "card" },
      el("p", { className: "muted", textContent: "כל התנועות נשארות בסיכום משק הבית. עסק הוא רק מבט נוסף: אפשר לראות רווח והפסד לכל עסק ולכל העסקים יחד. הוצאה מסומנת בעסק בעת ההוספה, והכנסות נספרות לפי הקטגוריה שמקושרת לעסק (כפתור 💼 בהגדרות הקטגוריות)." }),
      el("div", { className: "rows" }, ...(rows.length ? rows : [el("div", { className: "empty", textContent: "עוד אין עסקים" })])),
      el("div", { className: "actions", style: "justify-content:flex-start" }, btn("הוספת עסק", "", () => nameDialog(ctx))),
    ),
  );
}

/** Choose the business for a category (all its transactions count in it), or none. */
export function openCategoryBusinessDialog(ctx: ManageCtx, cat: Category) {
  ctx.openDialog((close) => {
    const pick = businessChips(ctx.biz, ctx.biz.cat.get(cat.id) ?? null);
    const needed = el("input", { type: "checkbox", checked: ctx.biz.required.has(cat.id) });
    const neededRow = el("label", { style: "flex-direction:row;align-items:center;gap:8px" }, needed, "חובה לבחור עסק בכל תנועה (ללא עסק קבוע)");
    const syncNeeded = () => {
      needed.disabled = pick.value != null;
      if (pick.value != null) needed.checked = false;
    };
    pick.onchange = syncNeeded;
    syncNeeded();
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "שמירה" });
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `עסק של "${cat.name}"` }),
      el("p", { className: "muted", textContent: cat.parent_id == null ? "כל התנועות בקטגוריה ובתתי-הקטגוריות שלה (שלא קושרו לעסק אחר) ייספרו בעסק, בלי תיוג נוסף." : "כל התנועות בתת-קטגוריה ייספרו בעסק, בלי תיוג נוסף." }),
      pick.root,
      neededRow,
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        await api.linkCategoryBusiness(cat.id, pick.value);
        await api.requireCategoryBusiness(cat.id, needed.checked);
        close();
        await ctx.reload();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        save.disabled = false;
      }
    };
    return form;
  });
}

/** Tag several transactions with a business, or remove their tags. */
export function openTagDialog(ctx: ManageCtx, ids: number[], done: () => void) {
  ctx.openDialog((close) => {
    const pick = businessChips(ctx.biz, null);
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "סימון" });
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `סימון ${ids.length} תנועות` }),
      el("p", { className: "muted", textContent: 'בחירת "ללא" מסירה את הסימון שלהן; אז הן נספרות לפי הקטגוריה, אם היא מקושרת לעסק. גם תנועות ישנות אפשר לסמן: הרווח מתעדכן אחורה.' }),
      pick.root,
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        await api.tagTransactions(ids, pick.value);
        close();
        done();
        await ctx.reload();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        save.disabled = false;
      }
    };
    return form;
  });
}

export const fmt = formatShekels;
