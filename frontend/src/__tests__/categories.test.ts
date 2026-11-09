import { describe, expect, it } from "vitest";
import type { Category } from "../api";
import { categoryName, categoryPath, isIncomeTx, NO_CATEGORY, sideAmount } from "../categories";

const cat = (id: number, name: string, parent_id: number | null = null) => ({ id, name, parent_id }) as Category;
const cats = [cat(1, "מזון"), cat(2, "סופר", 1)];

describe("categories", () => {
  it("names a category, or says there is none", () => {
    expect(categoryName(cats, 1)).toBe("מזון");
    expect(categoryName(cats, null)).toBe(NO_CATEGORY);
    expect(categoryName(cats, 99)).toBe(NO_CATEGORY);
  });

  it("tells income from expense by the category, so a refund is a negative income", () => {
    const kinds = [{ ...cat(10, "הכנסה"), is_income: true }, { ...cat(11, "הוצאה"), is_income: false }];
    const t = (category_id: number | null, amount_agorot: number) => ({ category_id, amount_agorot });
    expect(isIncomeTx(kinds, t(10, 5000))).toBe(true);
    expect(isIncomeTx(kinds, t(10, -5000))).toBe(true);
    expect(isIncomeTx(kinds, t(11, -5000))).toBe(false);
    // No category: the sign decides.
    expect(isIncomeTx(kinds, t(null, 5000))).toBe(true);
    expect(isIncomeTx(kinds, t(null, -5000))).toBe(false);
    // On its own side: a refund lowers the income.
    expect(sideAmount(kinds, t(10, -5000))).toBe(-5000);
    expect(sideAmount(kinds, t(11, -5000))).toBe(5000);
  });

  it("shows the parent for a sub-category only", () => {
    expect(categoryPath(cats, 2)).toBe("מזון › סופר");
    expect(categoryPath(cats, 1)).toBe("מזון");
  });
});
