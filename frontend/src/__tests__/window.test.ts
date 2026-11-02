import { describe, expect, it } from "vitest";
import { WINDOW_MONTHS, windowStart } from "../period";

describe("windowStart", () => {
  it("opens on the first day of the month WINDOW_MONTHS - 1 months back", () => {
    expect(WINDOW_MONTHS).toBe(24);
    expect(windowStart("2026-11-02")).toBe("2024-12-01");
    expect(windowStart("2026-01-31")).toBe("2024-02-01");
  });
});
