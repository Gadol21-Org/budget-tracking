import { type Product, type ProductsWire, type Category, type Transaction } from "./api";
import { contains, type Period } from "./period";

/**
 * Products are a lens over the household's own transactions: nothing is moved or removed from
 * the household figures. A transaction counts in the product it is tagged with, or else the
 * product its category (or its parent category) is linked to.
 */
export interface ProductsData {
  products: Product[];
  /** transaction id -> product id */
  tx: Map<number, number>;
  /** category id -> product id */
  cat: Map<number, number>;
  /** categories whose new transactions must say which product they belong to */
  required: Set<number>;
}

export const emptyProducts = (): ProductsData => ({ products: [], tx: new Map(), cat: new Map(), required: new Set() });

export const toProductsData = (d: ProductsWire): ProductsData => ({
  products: d.businesses,
  tx: new Map(d.links.transactions),
  cat: new Map(d.links.categories),
  required: new Set(d.links.required),
});

/** The product a category is linked to, directly or through its parent. */
export function categoryProduct(d: ProductsData, cats: Category[], categoryId: number | null): number | undefined {
  const c = cats.find((x) => x.id === categoryId);
  if (!c) return undefined;
  return d.cat.get(c.id) ?? (c.parent_id != null ? d.cat.get(c.parent_id) : undefined);
}

/** The product a transaction counts in: its own tag, else its category's link. */
export function productOf(d: ProductsData, cats: Category[], t: Transaction): number | undefined {
  const own = t.id > 0 ? d.tx.get(t.id) : undefined;
  return own ?? categoryProduct(d, cats, t.category_id);
}

/** Whether a new line in this category must say which product it belongs to (and none is given already). */
export function productRequired(d: ProductsData, cats: Category[], categoryId: number | null): boolean {
  const c = cats.find((x) => x.id === categoryId);
  if (!c || categoryProduct(d, cats, c.id) != null) return false;
  return d.required.has(c.id) || (c.parent_id != null && d.required.has(c.parent_id));
}

export const productName = (d: ProductsData, id: number | undefined | null) => d.products.find((b) => b.id === id)?.name ?? "";

// ---- Figures -------------------------------------------------------------------------------------

export type BusinessTab = "overview" | "trends" | "cats" | "compare";
export interface BusinessView {
  tab: BusinessTab;
  /** "all" = the whole business, or one product. */
  scope: "all" | number;
}

export interface BusinessCtx {
  txs: Transaction[];
  cats: Category[];
  products: ProductsData;
  period: Period;
  view: BusinessView;
  setView: (patch: Partial<BusinessView>) => void;
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

/** Transactions that count in the product `scope` ("all" = in any product). */
export const scoped = (ctx: Pick<BusinessCtx, "txs" | "cats" | "products">, scope: "all" | number) =>
  ctx.txs.filter((t) => {
    const b = productOf(ctx.products, ctx.cats, t);
    return b != null && (scope === "all" || b === scope);
  });

/** The six months ending with the period's last month, oldest first. */
export function lastMonths(p: Period, n = 6): string[] {
  const end = p.kind === "month" ? p.month : p.kind === "year" ? `${p.year}-12` : p.to.slice(0, 7);
  const [y, m] = end.split("-").map(Number);
  return Array.from({ length: n }, (_, i) => new Date(Date.UTC(y, m - 1 - (n - 1 - i), 1)).toISOString().slice(0, 7));
}

export const scopeName = (ctx: Pick<BusinessCtx, "products">, scope: "all" | number) => (scope === "all" ? "כל העסק" : productName(ctx.products, scope));

/** Costs shared by the products, split between the others by their share of the income. */
export function allocation(ctx: BusinessCtx, p: Period, id: number): number | null {
  const general = ctx.products.products.find((b) => b.is_general);
  if (!general || general.id === id) return null;
  const inP = (b: number) => figures(scoped(ctx, b).filter((t) => contains(p, t.date)));
  const cost = -inP(general.id).profit;
  const others = ctx.products.products.filter((b) => !b.is_general);
  const total = others.reduce((s, b) => s + inP(b.id).income, 0);
  if (cost === 0 || total <= 0) return null;
  return Math.round((cost * inP(id).income) / total);
}

/** Profit of every product in the period, for the card on the summary. */
export function summaryFigures(ctx: Pick<BusinessCtx, "txs" | "cats" | "products" | "period">): { total: Fig; per: { b: Product; f: Fig }[] } {
  const per = ctx.products.products.map((b) => ({ b, f: figures(scoped(ctx, b.id).filter((t) => contains(ctx.period, t.date))) }));
  return { total: figures(scoped(ctx, "all").filter((t) => contains(ctx.period, t.date))), per };
}

