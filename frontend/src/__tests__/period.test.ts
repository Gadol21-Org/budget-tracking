import { describe, expect, it } from "vitest";
import { addMonths, bounds, contains, monthOf, monthsBetween, shift } from "../period";

describe("period", () => {
  it("bounds a month, a year and a range inclusively", () => {
    expect(bounds({ kind: "month", month: "2026-02" })).toEqual({ from: "2026-02-01", to: "2026-02-31" });
    expect(bounds({ kind: "year", year: 2026 })).toEqual({ from: "2026-01-01", to: "2026-12-31" });
    expect(bounds({ kind: "range", from: "2026-03-05", to: "2026-03-09" })).toEqual({ from: "2026-03-05", to: "2026-03-09" });
  });

  it("contains dates on its edges", () => {
    const p = { kind: "range", from: "2026-03-05", to: "2026-03-09" } as const;
    expect(contains(p, "2026-03-05")).toBe(true);
    expect(contains(p, "2026-03-09")).toBe(true);
    expect(contains(p, "2026-03-10")).toBe(false);
  });

  it("opens the month of a date", () => {
    expect(monthOf("2026-10-31")).toEqual({ kind: "month", month: "2026-10" });
  });

  it("shifts months across a year boundary, years, and not ranges", () => {
    expect(shift({ kind: "month", month: "2026-01" }, -1)).toEqual({ kind: "month", month: "2025-12" });
    expect(shift({ kind: "month", month: "2026-12" }, 1)).toEqual({ kind: "month", month: "2027-01" });
    expect(shift({ kind: "year", year: 2026 }, 1)).toEqual({ kind: "year", year: 2027 });
    expect(shift({ kind: "range", from: "2026-01-01", to: "2026-01-02" }, 1)).toBeNull();
  });

  it("counts and adds months", () => {
    expect(monthsBetween("2026-10", "2027-02")).toBe(4);
    expect(monthsBetween("2026-10", "2026-08")).toBe(-2);
    expect(addMonths("2026-11", 3)).toBe("2027-02");
    expect(addMonths("2026-03", -3)).toBe("2025-12");
    expect(addMonths("2026-05", monthsBetween("2026-05", "2027-09"))).toBe("2027-09");
  });
});
