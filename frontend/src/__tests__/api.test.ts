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

describe("gateway failures", () => {
  const res = (status: number, type = "application/json") => new Response("x", { status, headers: { "content-type": type } });
  it("treats proxy errors and HTML pages as server unavailable", async () => {
    const { isGatewayFailure } = await import("../api");
    expect([502, 503, 504, 521, 530].every((s) => isGatewayFailure(res(s)))).toBe(true);
    expect(isGatewayFailure(res(200, "text/html"))).toBe(true);
  });
  it("leaves real app errors and good responses alone", async () => {
    const { isGatewayFailure } = await import("../api");
    expect(isGatewayFailure(res(500))).toBe(false);
    expect(isGatewayFailure(res(400))).toBe(false);
    expect(isGatewayFailure(res(200))).toBe(false);
  });
});
