import { describe, expect, it } from "vitest";
import { splitSumitNote } from "../sumitNote";

describe("splitSumitNote", () => {
  it("takes the products out of an imported receipt's note", () => {
    expect(splitSumitNote("קבלה 12 – דנה: שמלת ערב | תיקון [sumit:7]")).toEqual({ text: "קבלה 12 – דנה", products: ["שמלת ערב", "תיקון"] });
    expect(splitSumitNote("קבלה 12 – דנה: שמלה, כחולה [sumit:7]").products).toEqual(["שמלה, כחולה"]);
  });
  it("leaves other notes alone", () => {
    expect(splitSumitNote("קפה: עם חלב")).toEqual({ text: "קפה: עם חלב", products: [] });
    expect(splitSumitNote("קבלה 12 [sumit:7]").products).toEqual([]);
  });
});
