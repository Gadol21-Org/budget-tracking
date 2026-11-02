import { describe, expect, it } from "vitest";
import { WINDOW_MONTHS, windowStart } from "../period";

describe("windowStart", () => {
  it("opens on the first day of the month WINDOW_MONTHS - 1 months back (the current month and the two before it)", () => {
    expect(WINDOW_MONTHS).toBe(3);
    expect(windowStart("2026-11-02")).toBe("2026-09-01");
    expect(windowStart("2026-01-31")).toBe("2025-11-01");
  });
});
