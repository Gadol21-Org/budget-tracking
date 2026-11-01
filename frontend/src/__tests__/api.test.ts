import { describe, expect, it } from "vitest";
import { currencySymbol, formatMinor, parseAgorot, parseMinor, type Currency } from "../api";

describe("amounts", () => {
  it("parses shekels into exact agorot, accepting a comma", () => {
    expect(parseAgorot("12.5")).toBe(1250);
    expect(parseAgorot("0,07")).toBe(7);
    expect(parseAgorot("19.99")).toBe(1999);
    expect(() => parseAgorot("abc")).toThrow();
  });

  it("parses into each currency's minor unit (the yen has none)", () => {
    expect(parseMinor("12.50", "USD")).toBe(1250);
    expect(parseMinor("3000", "JPY")).toBe(3000);
  });

  it("formats minor units with the symbol", () => {
    expect(formatMinor(1250, "USD")).toBe("$12.50");
    expect(formatMinor(300000, "JPY")).toBe("¥300,000");
  });

  it("falls back to the code for an unknown currency", () => {
    expect(currencySymbol("ILS")).toBe("₪");
    expect(currencySymbol("XYZ" as Currency)).toBe("XYZ");
  });
});
