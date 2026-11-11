import type { Category } from "./api";

export const NO_CATEGORY = "ללא קטגוריה";

export const categoryById = (cats: Category[], id: number | null): Category | undefined => cats.find((c) => c.id === id);

/**
 * Income or expense: decided by the category, not by the sign. A refund is a negative income, so
 * its amount is below zero on an income category. Without a category the sign decides.
 */
export const isIncomeTx = (cats: Category[], t: { category_id: number | null; amount_agorot: number }): boolean => categoryById(cats, t.category_id)?.is_income ?? t.amount_agorot > 0;

/** The amount on its own side: money in for income (below zero for a refund), money out for an expense (below zero for a return). */
export const sideAmount = (cats: Category[], t: { category_id: number | null; amount_agorot: number }): number => (isIncomeTx(cats, t) ? t.amount_agorot : -t.amount_agorot);

export const categoryName = (cats: Category[], id: number | null): string => categoryById(cats, id)?.name ?? NO_CATEGORY;

/** "parent › child" for sub-categories, the plain name otherwise. */
export function categoryPath(cats: Category[], id: number | null): string {
  const c = categoryById(cats, id);
  const p = c?.parent_id != null ? categoryById(cats, c.parent_id) : undefined;
  return p && c ? `${p.name} › ${c.name}` : categoryName(cats, id);
}
