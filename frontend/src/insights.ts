import { bounds, contains, shift, title as periodTitle, type Period } from "./period";
import { type Category, type Transaction } from "./api";
import { clickable, el, type Child } from "./ui";

export type Dash = "overview" | "cats" | "trends";
/** What the selected period is compared with. */
export type Cmp = { kind: "prev" } | { kind: "avg"; n: number } | { kind: "yoy" } | { kind: "custom"; period: Period };
/** Which insights screen is open: a dashboard, and inside "cats" optionally a category and a sub-category. */
export interface InsightsNav {
  dash: Dash;
  /** Top-level category being inspected (null = uncategorized); undefined = the category list. */
  cat?: number | null;
  sub?: number;
}
export interface InsightsState extends InsightsNav {
  cmp: Cmp;
}

export interface Ctx {
  txs: Transaction[];
  cats: Category[];
  period: Period;
  ins: InsightsState;
  monthBar: () => HTMLElement;
  /** Change the open screen. "push" adds a history entry (back goes up one level). */
  navigate: (patch: Partial<InsightsNav>, mode: "push" | "replace") => void;
  setCmp: (c: Cmp) => void;
  setPeriod: (p: Period) => void;
  openList: (categoryId: number) => void;
  openDialog: (build: (close: () => void) => HTMLElement) => void;
}

const DASHES: [Dash, string, string][] = [
  ["overview", "🏠", "סקירה"],
  ["cats", "🧩", "קטגוריות"],
  ["trends", "📈", "מגמות"],
];

/** Categories are told apart by colour in the donut and the stacked chart. */
const PALETTE = ["#0f766e", "#4f46e5", "#d97706", "#db2777", "#0891b2", "#65a30d", "#9333ea"];
const OTHER_COLOR = "#94a3b8";

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
const shekels = (agorot: number) => (agorot / 100).toLocaleString("he-IL", { style: "currency", currency: "ILS", maximumFractionDigits: 0 });
const money = (agorot: number, cls = "") => el("span", { className: `num ${cls}`.trim(), textContent: shekels(agorot) });
const todayIso = () => new Date().toISOString().slice(0, 10);
const shortMonth = (m: string) => new Date(`${m}-01T00:00:00`).toLocaleDateString("he-IL", { month: "short" });
const longMonth = (m: string) => new Date(`${m}-01T00:00:00`).toLocaleDateString("he-IL", { month: "long", year: "numeric" });

// ---- Periods and comparison -------------------------------------------------------------------

const addDays = (iso: string, n: number) => new Date(Date.parse(`${iso}T00:00:00Z`) + n * 864e5).toISOString().slice(0, 10);
const daysIn = (from: string, to: string) => Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / 864e5) + 1;
const yearBack = (iso: string, n: number) => `${Number(iso.slice(0, 4)) - n}${iso.slice(4)}`;

/** The period(s) the selected one is compared with, and how to word it. Several periods are averaged. */
export function basePeriods(p: Period, c: Cmp): { label: string; periods: Period[] } {
  const kind = c.kind;
  if (kind === "custom") return { label: `מול ${periodTitle(c.period)}`, periods: [c.period] };
  if (p.kind === "month") {
    if (kind === "avg") {
      const periods = Array.from({ length: c.n }, (_, i) => shift(p, -(i + 1))!);
      return { label: `מול ממוצע ${c.n} החודשים שלפני`, periods };
    }
    if (kind === "yoy") return { label: "מול אותו חודש אשתקד", periods: [shift(p, -12)!] };
    return { label: "מול החודש הקודם", periods: [shift(p, -1)!] };
  }
  if (p.kind === "year") {
    if (kind === "avg") return { label: `מול ממוצע ${c.n} השנים שלפני`, periods: Array.from({ length: c.n }, (_, i) => shift(p, -(i + 1))!) };
    return { label: "מול השנה הקודמת", periods: [shift(p, -1)!] };
  }
  const len = daysIn(p.from, p.to);
  const before = (k: number): Period => ({ kind: "range", from: addDays(p.from, -len * k), to: addDays(p.from, -len * (k - 1) - 1) });
  if (kind === "avg") return { label: `מול ממוצע ${c.n} הטווחים שלפני`, periods: Array.from({ length: c.n }, (_, i) => before(i + 1)) };
  if (kind === "yoy") return { label: "מול אותו טווח אשתקד", periods: [{ kind: "range", from: yearBack(p.from, 1), to: yearBack(p.to, 1) }] };
  return { label: "מול הטווח הקודם", periods: [before(1)] };
}

/** The selected period is still running: its totals are partial, so comparing them with a full period misleads. */
export function isPartial(p: Period): boolean {
  const { from, to } = bounds(p);
  const t = todayIso();
  return from <= t && to >= t;
}

// ---- Aggregation ---------------------------------------------------------------------------------

interface Group {
  total: number;
  /** Amount booked on the top-level category itself, with no sub-category. */
  direct: number;
  kids: Map<number, number>;
}
/** Expenses (as positive agorot) and income in a period; expenses grouped by top-level category. */
interface Agg {
  income: number;
  expense: number;
  groups: Map<number | null, Group>;
}

function aggregate(ctx: Pick<Ctx, "txs" | "cats">, p: Period): Agg {
  const agg: Agg = { income: 0, expense: 0, groups: new Map() };
  for (const t of ctx.txs) {
    if (!contains(p, t.date)) continue;
    if (t.amount_agorot > 0) {
      agg.income += t.amount_agorot;
      continue;
    }
    const v = -t.amount_agorot;
    agg.expense += v;
    const own = ctx.cats.find((c) => c.id === t.category_id);
    const top = own?.parent_id ?? t.category_id;
    const g = agg.groups.get(top) ?? { total: 0, direct: 0, kids: new Map() };
    g.total += v;
    if (own?.parent_id != null) g.kids.set(own.id, (g.kids.get(own.id) ?? 0) + v);
    else g.direct += v;
    agg.groups.set(top, g);
  }
  return agg;
}

/** Mean of `f` over the comparison periods (one period = just its value). */
function baseValue(ctx: Ctx, f: (a: Agg) => number): number {
  const { periods } = basePeriods(ctx.period, ctx.ins.cmp);
  return periods.reduce((s, p) => s + f(aggregate(ctx, p)), 0) / periods.length;
}

/** The 12 months ending with the month the period ends in. */
function monthsEnding(p: Period): string[] {
  const end = bounds(p).to.slice(0, 7);
  return Array.from({ length: 12 }, (_, i) => (shift({ kind: "month", month: end }, i - 11) as { month: string }).month);
}

// ---- Small pieces --------------------------------------------------------------------------------

/** "▲ 12%" against the comparison; muted (no red/green) while the period is still running. */
function delta(cur: number, base: number, higherIsBad: boolean, muted: boolean): HTMLElement | null {
  if (base === 0) return cur === 0 ? null : el("span", { className: "delta flat", textContent: "חדש" });
  const pct = Math.round(((cur - base) / base) * 100);
  const flat = muted || Math.abs(pct) < 3;
  const bad = higherIsBad ? pct > 0 : pct < 0;
  const arrow = pct > 0 ? "▲" : pct < 0 ? "▼" : "■";
  return el("span", { className: `delta ${flat ? "flat" : bad ? "bad" : "good"}` }, el("span", { className: "num", textContent: `${pct > 0 ? "+" : ""}${pct}%` }), ` ${arrow}`);
}

function kpi(label: string, value: HTMLElement, sub?: Child): HTMLElement {
  return el("div", { className: "kpi" }, el("div", { className: "l", textContent: label }), el("div", { className: "v" }, value), el("div", {}, sub || el("span", { innerHTML: "&nbsp;" })));
}

function svgBox(html: string, onTap?: (i: number) => void): HTMLElement {
  const box = el("div", { className: "chart", innerHTML: html });
  if (onTap)
    box.addEventListener("click", (e) => {
      const hit = (e.target as Element).closest("[data-i]");
      if (hit) onTap(Number((hit as HTMLElement).dataset.i));
    });
  return box;
}

interface BarOpts {
  labels: string[];
  tips: string[];
  /** series[j][i]: value (agorot) of series j in slot i. */
  series: number[][];
  colors: string[];
  mode?: "single" | "grouped" | "stacked";
  highlight: Set<number>;
  avg?: number;
  height?: number;
}

/** Bar chart as inline SVG; every slot is tappable (data-i). Always drawn left to right in time. */
function barChart(o: BarOpts): string {
  const W = 360, padX = 6, H = o.height ?? 120, top = 16;
  const n = o.labels.length, slot = (W - 2 * padX) / n;
  const mode = o.mode ?? "single";
  const totals = o.labels.map((_, i) => (mode === "grouped" ? Math.max(...o.series.map((s) => s[i])) : o.series.reduce((a, s) => a + s[i], 0)));
  const max = Math.max(1, ...totals, o.avg ?? 0) * 1.05;
  const y = (v: number) => (v / max) * H;
  let out = `<svg viewBox="0 0 ${W} ${H + top + 18}" width="100%" role="img" style="direction:ltr">`;
  o.labels.forEach((label, i) => {
    const x0 = padX + i * slot;
    const hot = o.highlight.size === 0 || o.highlight.has(i);
    const op = hot ? 1 : 0.45;
    if (mode === "grouped") {
      const bw = (slot * 0.8) / o.series.length;
      o.series.forEach((s, j) => {
        const h = y(s[i]);
        out += `<rect x="${x0 + slot * 0.1 + j * bw}" y="${top + H - h}" width="${bw - 1}" height="${h}" rx="2" fill="${o.colors[j]}" opacity="${op}"/>`;
      });
    } else {
      let base = top + H;
      o.series.forEach((s, j) => {
        const h = y(s[i]);
        base -= h;
        out += `<rect x="${x0 + slot * 0.14}" y="${base}" width="${slot * 0.72}" height="${h}" rx="${mode === "stacked" ? 0 : 3}" fill="${o.colors[j]}" opacity="${op}"/>`;
      });
    }
    if (o.highlight.has(i) && o.highlight.size < n) out += `<text x="${x0 + slot / 2}" y="${top + H - y(totals[i]) - 4}" font-size="9" font-weight="700" text-anchor="middle" fill="currentColor">${Math.round(totals[i] / 100).toLocaleString("he-IL")}</text>`;
    out += `<text x="${x0 + slot / 2}" y="${top + H + 14}" font-size="9" text-anchor="middle" fill="${o.highlight.has(i) && o.highlight.size < n ? "var(--brand)" : "var(--muted)"}" font-weight="${o.highlight.has(i) && o.highlight.size < n ? 700 : 400}">${esc(label)}</text>`;
    out += `<rect data-i="${i}" x="${x0}" y="0" width="${slot}" height="${H + top + 18}" fill="transparent" style="cursor:pointer"><title>${esc(o.tips[i])}</title></rect>`;
  });
  if (o.avg) out += `<line x1="${padX}" x2="${W - padX}" y1="${top + H - y(o.avg)}" y2="${top + H - y(o.avg)}" stroke="#d97706" stroke-dasharray="4 3" stroke-width="1.5"/>`;
  return out + "</svg>";
}

function lineChart(values: number[], labels: string[], color: string): string {
  const W = 360, H = 90, padX = 10, top = 10;
  const max = Math.max(0, ...values), min = Math.min(0, ...values), span = max - min || 1;
  const x = (i: number) => padX + (i * (W - 2 * padX)) / Math.max(1, values.length - 1);
  const y = (v: number) => top + ((max - v) / span) * H;
  const pts = values.map((v, i) => `${x(i)},${y(v)}`).join(" ");
  const last = values.length - 1;
  return (
    `<svg viewBox="0 0 ${W} ${H + top + 18}" width="100%" role="img" style="direction:ltr">` +
    `<line x1="${padX}" x2="${W - padX}" y1="${y(0)}" y2="${y(0)}" stroke="var(--line)" stroke-width="1"/>` +
    `<polyline fill="none" stroke="${color}" stroke-width="2.2" stroke-linejoin="round" points="${pts}"/>` +
    `<circle cx="${x(last)}" cy="${y(values[last])}" r="4" fill="${color}"/>` +
    labels.map((l, i) => (i % 2 === 0 || i === last ? `<text x="${x(i)}" y="${top + H + 14}" font-size="9" text-anchor="middle" fill="var(--muted)">${esc(l)}</text>` : "")).join("") +
    "</svg>"
  );
}

function donut(parts: { value: number; color: string }[], center: string): string {
  const total = parts.reduce((s, p) => s + p.value, 0) || 1;
  const R = 56, r = 36, c = 70;
  let a0 = -Math.PI / 2;
  let paths = "";
  for (const p of parts) {
    if (p.value <= 0) continue;
    const a1 = a0 + Math.min(0.9999, p.value / total) * 2 * Math.PI;
    const big = a1 - a0 > Math.PI ? 1 : 0;
    const pt = (a: number, rad: number) => `${c + rad * Math.cos(a)},${c + rad * Math.sin(a)}`;
    paths += `<path d="M${pt(a0, R)} A${R},${R} 0 ${big} 1 ${pt(a1, R)} L${pt(a1, r)} A${r},${r} 0 ${big} 0 ${pt(a0, r)}Z" fill="${p.color}"/>`;
    a0 = a1;
  }
  return `<svg viewBox="0 0 140 140" width="132" role="img">${paths}<text x="70" y="66" text-anchor="middle" font-size="9" fill="var(--muted)">הוצאות</text><text x="70" y="82" text-anchor="middle" font-size="12" font-weight="700" fill="currentColor">${esc(center)}</text></svg>`;
}

// ---- Shared frame (chips, compare-to, partial note) ------------------------------------------

export function insightsView(ctx: Ctx): HTMLElement {
  const chips = el(
    "div",
    { className: "chips", role: "tablist" },
    ...DASHES.map(([id, ico, label]) => {
      const b = el("button", { className: "chip", type: "button", textContent: `${ico} ${label}` });
      b.setAttribute("aria-pressed", String(ctx.ins.dash === id));
      b.onclick = () => ctx.navigate({ dash: id, cat: undefined, sub: undefined }, "replace");
      return b;
    }),
  );
  const base = basePeriods(ctx.period, ctx.ins.cmp);
  const cmpBtn = el("button", { className: "cmpbtn", type: "button", textContent: `${base.label} ▾` });
  cmpBtn.onclick = () => openCompareDialog(ctx);
  const partial = isPartial(ctx.period);
  const body = { overview, cats, trends }[ctx.ins.dash](ctx, base.label, partial);
  const hasCompare = ctx.ins.dash !== "trends";
  return el(
    "div",
    {},
    ctx.monthBar(),
    chips,
    hasCompare && el("div", { className: "cmpbar" }, el("span", { className: "muted", textContent: "השוואה:" }), cmpBtn),
    hasCompare && partial && el("div", { className: "partialnote", textContent: "התקופה טרם הסתיימה: הסכומים חלקיים, ולכן אחוזי השינוי אינם צבועים. לתמונה מלאה בחרו חודש שהסתיים." }),
    body,
  );
}

function openCompareDialog(ctx: Ctx) {
  ctx.openDialog((close) => {
    const p = ctx.period;
    const cur = ctx.ins.cmp;
    const unit = p.kind === "month" ? "חודשים" : p.kind === "year" ? "שנים" : "טווחים";
    const options: [string, string][] = [
      ["prev", p.kind === "month" ? "החודש הקודם" : p.kind === "year" ? "השנה הקודמת" : "הטווח הקודם"],
      ["avg3", `ממוצע 3 ה${unit} שלפני`],
      ["avg6", `ממוצע 6 ה${unit} שלפני`],
      ["avg12", `ממוצע 12 ה${unit} שלפני`],
    ];
    if (p.kind !== "year") options.push(["yoy", p.kind === "month" ? "אותו חודש אשתקד" : "אותו טווח אשתקד"]);
    if (p.kind !== "range") options.push(["custom", p.kind === "month" ? "חודש אחר…" : "שנה אחרת…"]);
    const sel = el("select", {}, ...options.map(([v, t]) => el("option", { value: v, textContent: t })));
    sel.value = cur.kind === "avg" ? `avg${cur.n}` : cur.kind;
    if (!options.some(([v]) => v === sel.value)) sel.value = "prev";
    const monthIn = el("input", { type: "month", value: cur.kind === "custom" && cur.period.kind === "month" ? cur.period.month : (shift(p, -1) as { month: string } | null)?.month ?? "" });
    const yearIn = el("input", { type: "number", min: "2000", max: "2100", value: String(cur.kind === "custom" && cur.period.kind === "year" ? cur.period.year : p.kind === "year" ? p.year - 1 : new Date().getFullYear() - 1) });
    const custom = el("label", {}, p.kind === "month" ? "חודש להשוואה" : "שנה להשוואה", p.kind === "month" ? monthIn : yearIn);
    const sync = () => (custom.hidden = sel.value !== "custom");
    sel.onchange = sync;
    sync();
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `השוואת ${periodTitle(p)} אל…` }),
      el("label", {}, "השוואה", sel),
      custom,
      el("div", { className: "actions" }, cancel, el("button", { type: "submit", className: "btn primary", textContent: "החלה" })),
    );
    form.onsubmit = (e) => {
      e.preventDefault();
      const v = sel.value;
      if (v === "custom") {
        const period: Period | null = p.kind === "month" ? (monthIn.value ? { kind: "month", month: monthIn.value } : null) : yearIn.value ? { kind: "year", year: Number(yearIn.value) } : null;
        if (!period) return;
        ctx.setCmp({ kind: "custom", period });
      } else if (v.startsWith("avg")) ctx.setCmp({ kind: "avg", n: Number(v.slice(3)) });
      else ctx.setCmp({ kind: v as "prev" | "yoy" });
      close();
    };
    return form;
  });
}

// ---- Dashboard: overview ---------------------------------------------------------------------

type Body = (ctx: Ctx, baseLabel: string, partial: boolean) => HTMLElement;

const catName = (ctx: Ctx, id: number | null) => ctx.cats.find((c) => c.id === id)?.name ?? "ללא קטגוריה";

function monthTapper(ctx: Ctx, months: string[]) {
  return (i: number) => ctx.setPeriod({ kind: "month", month: months[i] });
}
function highlightSet(ctx: Ctx, months: string[]): Set<number> {
  return new Set(months.flatMap((m, i) => (contains(ctx.period, `${m}-15`) ? [i] : [])));
}

const overview: Body = (ctx, baseLabel, partial) => {
  const cur = aggregate(ctx, ctx.period);
  const bal = cur.income - cur.expense;
  const bExp = baseValue(ctx, (a) => a.expense);
  const bInc = baseValue(ctx, (a) => a.income);
  const bBal = baseValue(ctx, (a) => a.income - a.expense);
  const rate = cur.income > 0 ? Math.round((bal / cur.income) * 100) : null;
  const months = monthsEnding(ctx.period);
  const series = months.map((m) => aggregate(ctx, { kind: "month", month: m }).expense);
  const full = series.filter((_, i) => months[i] < todayIso().slice(0, 7));
  const avg = full.length ? full.reduce((a, b) => a + b, 0) / full.length : undefined;

  // Plain-language insights.
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => aggregate(ctx, p));
  const changes = [...cur.groups.entries()]
    .map(([id, g]) => ({ id, now: g.total, was: bases.reduce((s, b) => s + (b.groups.get(id)?.total ?? 0), 0) / bases.length }))
    .map((x) => ({ ...x, d: x.now - x.was }));
  const lines: HTMLElement[] = [];
  const row = (ico: string, ...kids: Child[]) => el("div", { className: "insight" }, el("span", { className: "ico", textContent: ico }), el("div", {}, ...kids));
  const up = [...changes].sort((a, b) => b.d - a.d)[0];
  const down = [...changes].sort((a, b) => a.d - b.d)[0];
  if (up && up.d > 0 && up.was > 0) lines.push(row("📈", el("b", { textContent: catName(ctx, up.id) }), ": ", money(up.d, "expense"), ` יותר (${Math.round((up.d / up.was) * 100)}%) ${baseLabel}.`));
  if (down && down.d < 0 && down !== up) lines.push(row("📉", el("b", { textContent: catName(ctx, down.id) }), ": ", money(-down.d, "income"), ` פחות ${baseLabel}.`));
  const upcoming = ctx.txs.filter((t) => t.upcoming && contains(ctx.period, t.date));
  if (upcoming.length) {
    const out = -upcoming.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);
    const inc = upcoming.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
    lines.push(row("⏳", "הסכומים כוללים ", money(out), " חיובים חוזרים", ...(inc ? [" ו-", money(inc), " הכנסות חוזרות"] : []), " שטרם בוצעו. יתרה צפויה בסוף התקופה: ", money(bal, bal < 0 ? "expense" : "income"), "."));
  }
  const biggest = ctx.txs.filter((t) => t.amount_agorot < 0 && contains(ctx.period, t.date)).sort((a, b) => a.amount_agorot - b.amount_agorot)[0];
  if (biggest) {
    const c = ctx.cats.find((x) => x.id === biggest.category_id);
    lines.push(row("🧾", "ההוצאה הגדולה ביותר: ", el("b", { textContent: biggest.note || c?.name || "ללא קטגוריה" }), " – ", money(-biggest.amount_agorot), "."));
  }

  return el(
    "div",
    {},
    el(
      "div",
      { className: "kpis" },
      kpi("הוצאות", money(cur.expense, "expense"), delta(cur.expense, bExp, true, partial)),
      kpi("הכנסות", money(cur.income, "income"), delta(cur.income, bInc, false, partial)),
      kpi("יתרה", money(bal, bal < 0 ? "expense" : "income"), el("span", { className: `num delta ${partial || Math.abs(bal - bBal) < 100 ? "flat" : bal > bBal ? "good" : "bad"}` }, `${bal >= bBal ? "+" : "−"}${shekels(Math.abs(bal - bBal))}`)),
      kpi("שיעור חיסכון", el("span", { className: "num", textContent: rate == null ? "–" : `${rate}%` }), el("span", { className: "delta flat", textContent: "מההכנסות" })),
    ),
    el("h2", { textContent: "תובנות" }),
    el("div", { className: "card" }, ...(lines.length ? lines : [el("div", { className: "empty", textContent: "אין עדיין נתונים בתקופה זו" })])),
    el("h2", { textContent: "הוצאות ב-12 החודשים האחרונים" }),
    el(
      "div",
      { className: "card" },
      svgBox(
        barChart({ labels: months.map(shortMonth), tips: months.map((m, i) => `${longMonth(m)}: ${shekels(series[i])}`), series: [series], colors: ["var(--expense)"], highlight: highlightSet(ctx, months), avg }),
        monthTapper(ctx, months),
      ),
      el("div", { className: "legend" }, el("span", {}, el("i", { style: "background:var(--expense)" }), "הוצאות"), avg != null && el("span", {}, el("i", { style: "background:#d97706" }), "ממוצע החודשים שהסתיימו"), el("span", { textContent: "הקשה על עמודה בוחרת את החודש" })),
    ),
  );
};

// ---- Dashboard: categories and drill-down -----------------------------------------------------

const catColor = (ctx: Ctx, id: number | null): string => {
  const tops = ctx.cats.filter((c) => c.parent_id == null && !c.is_income).map((c) => c.id).sort((a, b) => a - b);
  const i = tops.indexOf(id as number);
  if (i < 0) return OTHER_COLOR;
  // Every category gets its own colour: the fixed palette first, then evenly spread hues.
  return i < PALETTE.length ? PALETTE[i] : `hsl(${Math.round((i * 137.5) % 360)} 55% 45%)`;
};

const cats: Body = (ctx, baseLabel, partial) => {
  if (ctx.ins.cat !== undefined && ctx.ins.sub !== undefined) return subDetail(ctx, partial);
  if (ctx.ins.cat !== undefined) return catDetail(ctx, baseLabel, partial);
  const cur = aggregate(ctx, ctx.period);
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => aggregate(ctx, p));
  const rows = [...cur.groups.entries()].sort((a, b) => b[1].total - a[1].total);
  const max = rows[0]?.[1].total ?? 1;
  const parts = rows.map(([id, g]) => ({ value: g.total, color: catColor(ctx, id) }));
  const legend = el(
    "div",
    { className: "donutlegend" },
    ...rows.map(([id, g]) => el("div", {}, el("i", { style: `background:${catColor(ctx, id)}` }), `${catName(ctx, id)} `, el("span", { className: "muted num", textContent: `${Math.round((g.total / cur.expense) * 100)}%` }))),
  );
  return el(
    "div",
    {},
    rows.length === 0
      ? el("div", { className: "empty", textContent: "אין הוצאות בתקופה זו" })
      : el(
          "div",
          {},
          el("div", { className: "card donutcard" }, svgBox(donut(parts, shekels(cur.expense))), legend),
          el("h2", { textContent: "הקשה על קטגוריה פותחת אותה" }),
          el(
            "div",
            { className: "card rows" },
            ...rows.map(([id, g]) => {
              const was = bases.reduce((s, b) => s + (b.groups.get(id)?.total ?? 0), 0) / bases.length;
              return clickable(
                el(
                  "div",
                  { className: "row" },
                  el("div", { className: "grow" }, el("div", { className: "name", textContent: catName(ctx, id) }), el("div", { className: "bar" }, el("span", { style: `width:${Math.max(3, (g.total / max) * 100)}%;background:${catColor(ctx, id)}` }))),
                  el("div", { className: "amt" }, money(g.total), el("div", {}, delta(g.total, was, true, partial), " ", el("span", { className: "muted num", textContent: `${Math.round((g.total / cur.expense) * 100)}%` }))),
                  el("span", { className: "chev muted", textContent: "‹" }),
                ),
                () => ctx.navigate({ dash: "cats", cat: id, sub: undefined }, "push"),
              );
            }),
          ),
        ),
  );
};

function crumbs(ctx: Ctx, ...parts: { text: string; to?: Partial<InsightsNav> }[]): HTMLElement {
  const nodes: Child[] = [];
  parts.forEach((p, i) => {
    if (i) nodes.push(el("span", { className: "muted", textContent: "›" }));
    if (p.to) {
      const b = el("button", { type: "button", className: "crumb", textContent: p.text });
      b.onclick = () => ctx.navigate({ dash: "cats", ...p.to }, "push");
      nodes.push(b);
    } else nodes.push(el("b", { textContent: p.text }));
  });
  return el("div", { className: "crumbs" }, ...nodes);
}

/** The monthly chart of one category or sub-category. */
function trendCard(ctx: Ctx, value: (a: Agg) => number, color: string): { node: HTMLElement; avg: number } {
  const months = monthsEnding(ctx.period);
  const series = months.map((m) => value(aggregate(ctx, { kind: "month", month: m })));
  const full = series.filter((_, i) => months[i] < todayIso().slice(0, 7));
  const avg = full.length ? full.reduce((a, b) => a + b, 0) / full.length : series.reduce((a, b) => a + b, 0) / series.length;
  return {
    avg,
    node: el(
      "div",
      { className: "card" },
      svgBox(
        barChart({ labels: months.map(shortMonth), tips: months.map((m, i) => `${longMonth(m)}: ${shekels(series[i])}`), series: [series], colors: [color], highlight: highlightSet(ctx, months), avg }),
        monthTapper(ctx, months),
      ),
      el("div", { className: "legend" }, el("span", {}, el("i", { style: "background:#d97706" }), "ממוצע החודשים שהסתיימו")),
    ),
  };
}

function catDetail(ctx: Ctx, baseLabel: string, partial: boolean): HTMLElement {
  const id = ctx.ins.cat!;
  const cur = aggregate(ctx, ctx.period);
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => aggregate(ctx, p));
  const g = cur.groups.get(id) ?? { total: 0, direct: 0, kids: new Map<number, number>() };
  const was = bases.reduce((s, b) => s + (b.groups.get(id)?.total ?? 0), 0) / bases.length;
  const trend = trendCard(ctx, (a) => a.groups.get(id)?.total ?? 0, catColor(ctx, id));
  const kids = [...g.kids.entries()].sort((a, b) => b[1] - a[1]);
  const kidMax = Math.max(1, kids[0]?.[1] ?? 0, g.direct);
  const kidRow = (name: string, v: number, was: number, onTap?: () => void) => {
    const r = el(
      "div",
      { className: "row" },
      el("div", { className: "grow" }, el("div", { className: "name", textContent: name }), el("div", { className: "bar" }, el("span", { style: `width:${Math.max(3, (v / kidMax) * 100)}%;background:${catColor(ctx, id)}` }))),
      el("div", { className: "amt" }, money(v), el("div", {}, delta(v, was, true, partial))),
      onTap ? el("span", { className: "chev muted", textContent: "‹" }) : null,
    );
    return onTap ? clickable(r, onTap) : r;
  };
  const wasKid = (kid: number) => bases.reduce((s, b) => s + (b.groups.get(id)?.kids.get(kid) ?? 0), 0) / bases.length;
  const wasDirect = bases.reduce((s, b) => s + (b.groups.get(id)?.direct ?? 0), 0) / bases.length;
  const listBtn = el("button", { type: "button", className: "btn", textContent: `הצגת התנועות של ${catName(ctx, id)} ‹` });
  listBtn.onclick = () => id != null && ctx.openList(id);
  return el(
    "div",
    {},
    crumbs(ctx, { text: "קטגוריות", to: { cat: undefined, sub: undefined } }, { text: catName(ctx, id) }),
    el(
      "div",
      { className: "kpis" },
      kpi(periodTitle(ctx.period), money(g.total, "expense"), el("span", {}, delta(g.total, was, true, partial), ` ${baseLabel}`)),
      kpi("ממוצע חודשי", money(Math.round(trend.avg)), el("span", { className: "delta flat", textContent: cur.expense ? `${Math.round((g.total / cur.expense) * 100)}% מההוצאות בתקופה` : "" })),
    ),
    el("h2", { textContent: `${catName(ctx, id)} לאורך זמן` }),
    trend.node,
    el("h2", { textContent: kids.length ? "תתי-קטגוריות" : "" }),
    kids.length || g.direct
      ? el("div", { className: "card rows" }, ...kids.map(([k, v]) => kidRow(catName(ctx, k), v, wasKid(k), () => ctx.navigate({ dash: "cats", cat: id, sub: k }, "push"))), g.direct > 0 && kids.length > 0 && kidRow("ללא תת-קטגוריה", g.direct, wasDirect))
      : el("div", { className: "empty", textContent: "אין הוצאות בקטגוריה זו בתקופה" }),
    id != null && listBtn,
  );
}

function subDetail(ctx: Ctx, partial: boolean): HTMLElement {
  const id = ctx.ins.cat!, sub = ctx.ins.sub!;
  const value = (a: Agg) => a.groups.get(id)?.kids.get(sub) ?? 0;
  const cur = value(aggregate(ctx, ctx.period));
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => value(aggregate(ctx, p)));
  const was = bases.reduce((a, b) => a + b, 0) / bases.length;
  const trend = trendCard(ctx, value, catColor(ctx, id));
  const txs = ctx.txs.filter((t) => t.category_id === sub && t.amount_agorot < 0 && contains(ctx.period, t.date)).sort((a, b) => a.amount_agorot - b.amount_agorot).slice(0, 8);
  const listBtn = el("button", { type: "button", className: "btn", textContent: `הצגת כל התנועות של ${catName(ctx, sub)} ‹` });
  listBtn.onclick = () => ctx.openList(sub);
  return el(
    "div",
    {},
    crumbs(ctx, { text: "קטגוריות", to: { cat: undefined, sub: undefined } }, { text: catName(ctx, id), to: { cat: id, sub: undefined } }, { text: catName(ctx, sub) }),
    el(
      "div",
      { className: "kpis" },
      kpi(periodTitle(ctx.period), money(cur, "expense"), delta(cur, was, true, partial)),
      kpi("ממוצע חודשי", money(Math.round(trend.avg)), el("span", { className: "delta flat", textContent: trend.avg ? `${cur >= trend.avg ? "+" : ""}${Math.round(((cur - trend.avg) / trend.avg) * 100)}% מהממוצע` : "" })),
    ),
    el("h2", { textContent: `${catName(ctx, sub)} לאורך זמן` }),
    trend.node,
    el("h2", { textContent: "התנועות הגדולות בתקופה" }),
    txs.length
      ? el(
          "div",
          { className: "card rows" },
          ...txs.map((t) =>
            el(
              "div",
              { className: "row" },
              el("div", { className: "grow" }, el("div", { className: "name", textContent: t.note || catName(ctx, sub) }), el("div", { className: "muted small", textContent: `${t.upcoming ? "⏳ " : ""}${new Date(`${t.date}T00:00:00`).toLocaleDateString("he-IL", { day: "numeric", month: "long" })}` })),
              money(-t.amount_agorot, "expense"),
            ),
          ),
        )
      : el("div", { className: "empty", textContent: "אין תנועות בתקופה" }),
    listBtn,
  );
}

// ---- Dashboard: trends ---------------------------------------------------------------------------

const trends: Body = (ctx) => {
  const months = monthsEnding(ctx.period);
  const aggs = months.map((m) => aggregate(ctx, { kind: "month", month: m }));
  const income = aggs.map((a) => a.income), expense = aggs.map((a) => a.expense);
  const labels = months.map(shortMonth);
  const tips = months.map((m, i) => `${longMonth(m)}: הכנסות ${shekels(income[i])}, הוצאות ${shekels(expense[i])}`);
  const hi = highlightSet(ctx, months);
  const tap = monthTapper(ctx, months);

  // Averages count only months that have ended and hold data.
  const thisMonth = todayIso().slice(0, 7);
  const done = months.map((m, i) => i).filter((i) => months[i] < thisMonth && (income[i] || expense[i]));
  const mean = (xs: number[]) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : 0);
  const avgExp = mean(done.map((i) => expense[i])), avgSave = mean(done.map((i) => income[i] - expense[i]));

  let run = 0;
  const cumulative = months.map((_, i) => (run += income[i] - expense[i]));

  // Top categories over the 12 months, the rest as "other".
  const totals = new Map<number | null, number>();
  for (const a of aggs) for (const [id, g] of a.groups) totals.set(id, (totals.get(id) ?? 0) + g.total);
  const order = [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([id]) => id);
  const shown = order.slice(0, 6);
  const stackSeries = [...shown.map((id) => aggs.map((a) => a.groups.get(id)?.total ?? 0)), aggs.map((a) => [...a.groups].filter(([id]) => !shown.includes(id)).reduce((s, [, g]) => s + g.total, 0))];
  const stackColors = [...shown.map((id) => catColor(ctx, id)), OTHER_COLOR];

  return el(
    "div",
    {},
    el(
      "div",
      { className: "kpis" },
      kpi("הוצאות בחודש (ממוצע)", money(Math.round(avgExp), "expense"), el("span", { className: "delta flat", textContent: `${done.length} חודשים שהסתיימו` })),
      kpi("חיסכון בחודש (ממוצע)", money(Math.round(avgSave), avgSave < 0 ? "expense" : "income"), el("span", { className: "delta flat", textContent: "הכנסות פחות הוצאות" })),
    ),
    el("h2", { textContent: "הכנסות מול הוצאות" }),
    el(
      "div",
      { className: "card" },
      svgBox(barChart({ labels, tips, series: [income, expense], colors: ["var(--income)", "var(--expense)"], mode: "grouped", highlight: hi }), tap),
      el("div", { className: "legend" }, el("span", {}, el("i", { style: "background:var(--income)" }), "הכנסות"), el("span", {}, el("i", { style: "background:var(--expense)" }), "הוצאות")),
    ),
    el("h2", { textContent: "חיסכון מצטבר" }),
    el("div", { className: "card" }, svgBox(lineChart(cumulative, labels, "var(--brand)")), el("div", { className: "muted small", textContent: "הקו עולה כשהחודש נסגר בפלוס, ויורד כשבמינוס." })),
    el("h2", { textContent: "הוצאות לפי קטגוריה" }),
    el(
      "div",
      { className: "card" },
      svgBox(barChart({ labels, tips, series: stackSeries, colors: stackColors, mode: "stacked", highlight: hi, height: 140 }), tap),
      el("div", { className: "legend wrap" }, ...shown.map((id, i) => el("span", {}, el("i", { style: `background:${stackColors[i]}` }), catName(ctx, id))), stackSeries.at(-1)!.some((v) => v > 0) && el("span", {}, el("i", { style: `background:${OTHER_COLOR}` }), "אחר")),
    ),
    el("div", { className: "muted small pad", textContent: "הקשה על חודש בגרף בוחרת אותו כתקופה בכל המסכים." }),
  );
};
