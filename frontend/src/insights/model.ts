import { bounds, contains, shift, title as periodTitle, type Period } from "../period";
import { type Category, type Transaction } from "../api";
import { categoryName, categoryPath, isIncomeTx, sideAmount } from "../categories";

// The figures behind the dashboards: periods and comparisons, totals per category, colours. No DOM.

/** The two directions money moves. */
export type Side = "expense" | "income";
export type Dash = "overview" | "cats" | "trends" | "changes" | "fixed" | "rhythm";
/** What the selected period is compared with. */
export type Cmp = { kind: "prev" } | { kind: "avg"; n: number } | { kind: "custom"; period: Period };
/** Which insights screen is open: a dashboard, and inside "cats" optionally a category and a sub-category. */
export interface InsightsNav {
  dash: Dash;
  /** Which direction of money the categories dashboard shows; expenses unless chosen. */
  side?: Side;
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
  /** Change the open screen. "push" adds a history entry (back goes up one level). */
  navigate: (patch: Partial<InsightsNav>, mode: "push" | "replace") => void;
  setCmp: (c: Cmp) => void;
  setPeriod: (p: Period) => void;
  openList: (categoryId: number) => void;
  /** Open a category (and sub-category) where the category breakdown lives: the summary. */
  openCategory: (cat: number | null, sub?: number) => void;
}

/** Categories are told apart by colour in the donut and the stacked chart. */
export const PALETTE = ["#0f766e", "#4f46e5", "#d97706", "#db2777", "#0891b2", "#65a30d", "#9333ea"];
export const OTHER_COLOR = "#94a3b8";

export const todayIso = () => new Date().toISOString().slice(0, 10);

// ---- Periods and comparison -------------------------------------------------------------------

export const addDays = (iso: string, n: number) => new Date(Date.parse(`${iso}T00:00:00Z`) + n * 864e5).toISOString().slice(0, 10);
export const daysIn = (from: string, to: string) => Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / 864e5) + 1;

/** The period(s) the selected one is compared with, and how to word it. Several periods are averaged. */
export function basePeriods(p: Period, c: Cmp): { label: string; periods: Period[] } {
  const kind = c.kind;
  if (kind === "custom") return { label: `מול ${periodTitle(c.period)}`, periods: [c.period] };
  if (p.kind === "month") {
    if (kind === "avg") {
      const periods = Array.from({ length: c.n }, (_, i) => shift(p, -(i + 1))!);
      return { label: `מול ממוצע ${c.n} החודשים שלפני`, periods };
    }
    return { label: "מול החודש הקודם", periods: [shift(p, -1)!] };
  }
  if (p.kind === "year") {
    if (kind === "avg") return { label: `מול ממוצע ${c.n} השנים שלפני`, periods: Array.from({ length: c.n }, (_, i) => shift(p, -(i + 1))!) };
    return { label: "מול השנה הקודמת", periods: [shift(p, -1)!] };
  }
  const len = daysIn(p.from, p.to);
  const before = (k: number): Period => ({ kind: "range", from: addDays(p.from, -len * k), to: addDays(p.from, -len * (k - 1) - 1) });
  if (kind === "avg") return { label: `מול ממוצע ${c.n} הטווחים שלפני`, periods: Array.from({ length: c.n }, (_, i) => before(i + 1)) };
  return { label: "מול הטווח הקודם", periods: [before(1)] };
}

/** The selected period is still running: its totals are partial, so comparing them with a full period misleads. */
export function isPartial(p: Period): boolean {
  const { from, to } = bounds(p);
  const t = todayIso();
  return from <= t && to >= t;
}

// ---- Aggregation ---------------------------------------------------------------------------------

export interface Group {
  total: number;
  /** Amount booked on the top-level category itself, with no sub-category. */
  direct: number;
  kids: Map<number, number>;
}
/**
 * What the categories dashboard knows about a side. To narrow a side further (say, income by who earned it),
 * extend `pick`: every category figure goes through it.
 */
export const SIDES: Record<Side, { title: string; total: string; empty: string; cls: "expense" | "income"; higherIsBad: boolean; pick: (t: Transaction, cats: Category[]) => boolean }> = {
  expense: { title: "הוצאות", total: "מההוצאות בתקופה", empty: "אין הוצאות בתקופה זו", cls: "expense", higherIsBad: true, pick: (t, cats) => !isIncomeTx(cats, t) },
  income: { title: "הכנסות", total: "מההכנסות בתקופה", empty: "אין הכנסות בתקופה זו", cls: "income", higherIsBad: false, pick: (t, cats) => isIncomeTx(cats, t) },
};
export const sideOf = (ctx: Pick<Ctx, "ins">): Side => ctx.ins.side ?? "expense";

/** Totals of a period, and one side's money (as positive agorot) grouped by top-level category. */
export interface Agg {
  income: number;
  expense: number;
  /** Sum of the chosen side after `pick`. */
  sideTotal: number;
  groups: Map<number | null, Group>;
}

export function aggregate(ctx: Pick<Ctx, "txs" | "cats">, p: Period, side: Side = "expense"): Agg {
  const agg: Agg = { income: 0, expense: 0, sideTotal: 0, groups: new Map() };
  const pick = SIDES[side].pick;
  for (const t of ctx.txs) {
    if (!contains(p, t.date)) continue;
    // On its own side: a refund (negative income) lowers the income.
    const v = sideAmount(ctx.cats, t);
    if (isIncomeTx(ctx.cats, t)) agg.income += v;
    else agg.expense += v;
    if (!pick(t, ctx.cats)) continue;
    agg.sideTotal += v;
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
export function baseValue(ctx: Ctx, f: (a: Agg) => number): number {
  const { periods } = basePeriods(ctx.period, ctx.ins.cmp);
  return periods.reduce((s, p) => s + f(aggregate(ctx, p)), 0) / periods.length;
}

/** The 12 months ending with the month the period ends in. */
export function monthsEnding(p: Period): string[] {
  const end = bounds(p).to.slice(0, 7);
  return Array.from({ length: 12 }, (_, i) => (shift({ kind: "month", month: end }, i - 11) as { month: string }).month);
}

/** Every item gets its own colour: the fixed palette first, then evenly spread hues. */
export const colorAt = (i: number) => (i < PALETTE.length ? PALETTE[i] : `hsl(${Math.round((i * 137.5) % 360)} 55% 45%)`);
export const catColor = (ctx: Ctx, id: number | null): string => {
  const own = ctx.cats.find((c) => c.id === id);
  const tops = ctx.cats.filter((c) => c.parent_id == null && c.is_income === (own?.is_income ?? false)).map((c) => c.id).sort((a, b) => a - b);
  const i = tops.indexOf(id as number);
  return i < 0 ? OTHER_COLOR : colorAt(i);
};

export const catName = (ctx: Ctx, id: number | null) => categoryName(ctx.cats, id);

export function highlightSet(ctx: Ctx, months: string[]): Set<number> {
  return new Set(months.flatMap((m, i) => (contains(ctx.period, `${m}-15`) ? [i] : [])));
}

/** Fixed = charges that come from a repeating series set up in the app (rent, subscriptions…), settled or still ahead. */
export const isFixed = (t: Transaction) => t.recurring_series_id != null;
export const daysInMonth = (month: string) => new Date(Date.UTC(Number(month.slice(0, 4)), Number(month.slice(5, 7)), 0)).getUTCDate();

export const WEEKDAYS = ["א׳", "ב׳", "ג׳", "ד׳", "ה׳", "ו׳", "ש׳"];
export const dayOfWeek = (iso: string) => new Date(`${iso}T00:00:00Z`).getUTCDay();

/** Variable (non-repeating) spending per day of a month, in agorot; index 0 = the 1st. */
export function dayTotals(ctx: Ctx, month: string): number[] {
  const out = Array<number>(daysInMonth(month)).fill(0);
  for (const t of ctx.txs) if (!isIncomeTx(ctx.cats, t) && !isFixed(t) && t.date.startsWith(month)) out[Number(t.date.slice(8)) - 1] += -t.amount_agorot;
  return out;
}

export const catPath = (ctx: Ctx, id: number | null) => categoryPath(ctx.cats, id);

