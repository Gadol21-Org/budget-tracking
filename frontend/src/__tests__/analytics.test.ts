// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => {
  vi.unstubAllEnvs();
  vi.resetModules();
  document.head.innerHTML = "";
  delete (window as { dataLayer?: unknown }).dataLayer;
  delete (window as { gtag?: unknown }).gtag;
});

describe("analytics", () => {
  it("does nothing unless the production flag is set", async () => {
    const { startAnalytics, trackScreen } = await import("../analytics");
    startAnalytics();
    trackScreen("month");
    expect(document.querySelector("script")).toBeNull();
    expect((window as { dataLayer?: unknown }).dataLayer).toBeUndefined();
  });

  it("loads gtag.js and sends only a fixed screen name, once per screen", async () => {
    vi.stubEnv("VITE_ENABLE_ANALYTICS", "true");
    const { startAnalytics, trackScreen } = await import("../analytics");
    startAnalytics();
    expect(document.querySelector<HTMLScriptElement>("script")!.src).toContain("googletagmanager.com/gtag/js?id=G-F459M13TGD");
    trackScreen("insights");
    trackScreen("insights");
    trackScreen("settings");
    const layer = (window as unknown as { dataLayer: ArrayLike<unknown>[] }).dataLayer.map((a) => Array.from(a));
    const views = layer.filter((a) => a[0] === "event");
    expect(views).toHaveLength(2);
    expect(views[0][2]).toEqual({ page_title: "insights", page_location: `${location.origin}/insights`, page_path: "/insights" });
  });
});
