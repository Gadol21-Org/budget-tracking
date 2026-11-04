import { type Business, type BusinessData, type Category, type Transaction } from "./api";
import { contains, type Period } from "./period";

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

// ---- Figures -------------------------------------------------------------------------------------

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
  setView: (patch: Partial<BizView>) => void;
  back: () => void;
}

export interface Fig {
  income: number;
  expense: number;
  profit: number;
}
export const figures = (ts: Transaction[]): Fig => {
  const income = ts.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
  const expense = -ts.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);
  return { income, expense, profit: income - expense };
};

/** Transactions that count in the business `scope` ("all" = in any business). */
export const scoped = (ctx: Pick<BizCtx, "txs" | "cats" | "biz">, scope: "all" | number) =>
  ctx.txs.filter((t) => {
    const b = businessOf(ctx.biz, ctx.cats, t);
    return b != null && (scope === "all" || b === scope);
  });

/** The six months ending with the period's last month, oldest first. */
export function lastMonths(p: Period, n = 6): string[] {
  const end = p.kind === "month" ? p.month : p.kind === "year" ? `${p.year}-12` : p.to.slice(0, 7);
  const [y, m] = end.split("-").map(Number);
  return Array.from({ length: n }, (_, i) => new Date(Date.UTC(y, m - 1 - (n - 1 - i), 1)).toISOString().slice(0, 7));
}

export const scopeName = (ctx: Pick<BizCtx, "biz">, scope: "all" | number) => (scope === "all" ? "כל העסק" : bizName(ctx.biz, scope));

/** Costs shared by the businesses, split between the others by their share of the income. */
export function allocation(ctx: BizCtx, p: Period, id: number): number | null {
  const general = ctx.biz.businesses.find((b) => b.is_general);
  if (!general || general.id === id) return null;
  const inP = (b: number) => figures(scoped(ctx, b).filter((t) => contains(p, t.date)));
  const cost = -inP(general.id).profit;
  const others = ctx.biz.businesses.filter((b) => !b.is_general);
  const total = others.reduce((s, b) => s + inP(b.id).income, 0);
  if (cost === 0 || total <= 0) return null;
  return Math.round((cost * inP(id).income) / total);
}

/** Profit of every business in the period, for the card on the summary. */
export function summaryFigures(ctx: Pick<BizCtx, "txs" | "cats" | "biz" | "period">): { total: Fig; per: { b: Business; f: Fig }[] } {
  const per = ctx.biz.businesses.map((b) => ({ b, f: figures(scoped(ctx, b.id).filter((t) => contains(ctx.period, t.date))) }));
  return { total: figures(scoped(ctx, "all").filter((t) => contains(ctx.period, t.date))), per };
}

