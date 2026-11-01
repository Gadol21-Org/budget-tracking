import type { Category } from "./api";

export const NO_CATEGORY = "ללא קטגוריה";

export const categoryById = (cats: Category[], id: number | null): Category | undefined => cats.find((c) => c.id === id);

export const categoryName = (cats: Category[], id: number | null): string => categoryById(cats, id)?.name ?? NO_CATEGORY;

/** "parent › child" for sub-categories, the plain name otherwise. */
export function categoryPath(cats: Category[], id: number | null): string {
  const c = categoryById(cats, id);
  const p = c?.parent_id != null ? categoryById(cats, c.parent_id) : undefined;
  return p && c ? `${p.name} › ${c.name}` : categoryName(cats, id);
}
