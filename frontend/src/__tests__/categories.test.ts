import { describe, expect, it } from "vitest";
import type { Category } from "../api";
import { categoryName, categoryPath, NO_CATEGORY } from "../categories";

const cat = (id: number, name: string, parent_id: number | null = null) => ({ id, name, parent_id }) as Category;
const cats = [cat(1, "מזון"), cat(2, "סופר", 1)];

describe("categories", () => {
  it("names a category, or says there is none", () => {
    expect(categoryName(cats, 1)).toBe("מזון");
    expect(categoryName(cats, null)).toBe(NO_CATEGORY);
    expect(categoryName(cats, 99)).toBe(NO_CATEGORY);
  });

  it("shows the parent for a sub-category only", () => {
    expect(categoryPath(cats, 2)).toBe("מזון › סופר");
    expect(categoryPath(cats, 1)).toBe("מזון");
  });
});
