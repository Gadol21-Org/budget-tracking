// @vitest-environment jsdom
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

vi.mock("../sync", () => ({
  ensureHistory: () => {},
  neededFrom: () => "0000-00-00",
  load: vi.fn(async () => {
    const { render } = await import("../render");
    render();
  }),
  loadHistory: async () => {},
  IN_FLIGHT: new Set<string>(),
}));
vi.mock("../api", async (orig) => ({
  ...(await orig<typeof import("../api")>()),
  api: {
    importPaamonim: vi.fn(async () => ({ transactions_added: 12, transactions_skipped: 30, categories_added: 1 })),
    exportSheets: vi.fn(async () => ({ transactions: 5, tabs: 1 })),
    setLogUserName: vi.fn(async () => {}),
    sumit: vi.fn(async () => ({ configured: false, last_poll_at: null, last_error: null, next_poll_at: null, calls_this_month: 0, monthly_limit: 0, products: [], notices: [], held: [], push_public_key: null })),
  },
}));

let app: typeof import("../render");
let state: typeof import("../state").state;

beforeAll(async () => {
  document.body.innerHTML = '<div id="app"></div>';
  app = await import("../render");
  state = (await import("../state")).state;
});

afterEach(() => {
  cleanup();
});

describe("a data refresh on a screen", () => {
  it("keeps the result of a Settings action (it used to vanish when the data reloaded)", async () => {
    state.view = "settings";
    app.render();
    fireEvent.click(await screen.findByRole("button", { name: "ייבוא מפעמונים" }));
    await waitFor(() => expect(screen.getByText(/יובאו 12 תנועות חדשות/)).toBeTruthy());
    // The reload after the action renders again; the message must still be there.
    app.render();
    expect(screen.getByText(/יובאו 12 תנועות חדשות/)).toBeTruthy();
  });

  it("keeps a half-typed name on the user names screen", async () => {
    state.logUsers = [{ email: "dana@example.com", name: "דנה" }];
    state.view = "people";
    app.render();
    const input = (await screen.findByLabelText("שם תצוגה עבור dana@example.com")) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "דנה כהן" } });
    app.render();
    expect((screen.getByLabelText("שם תצוגה עבור dana@example.com") as HTMLInputElement).value).toBe("דנה כהן");
  });

  it("starts the screen afresh when what it shows changes", async () => {
    state.logUsers = [{ email: "dana@example.com", name: "דנה" }];
    state.view = "people";
    app.render();
    fireEvent.change(await screen.findByLabelText("שם תצוגה עבור dana@example.com"), { target: { value: "אחר" } });
    state.view = "settings";
    app.render();
    state.view = "people";
    app.render();
    expect((screen.getByLabelText("שם תצוגה עבור dana@example.com") as HTMLInputElement).value).toBe("דנה");
  });
});

describe("the app shell", () => {
  it("marks the open tab, and Settings stays marked on its sub-screens", () => {
    state.view = "currencies";
    app.render();
    const nav = within(document.querySelector("nav")!);
    expect(nav.getByRole("button", { name: /הגדרות/ }).getAttribute("aria-current")).toBe("page");
    expect(nav.getByRole("button", { name: /סיכום/ }).getAttribute("aria-current")).toBeNull();
  });

  it("shows the offline banner only while offline", () => {
    state.view = "settings";
    state.offline = true;
    app.render();
    expect(screen.getByText(/אין חיבור או שהשרת אינו זמין/)).toBeTruthy();
    state.offline = false;
    app.render();
    expect(screen.queryByText(/אין חיבור או שהשרת אינו זמין/)).toBeNull();
  });
});
