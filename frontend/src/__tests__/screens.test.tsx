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
  fetchInbox: vi.fn(async () => []),
  IN_FLIGHT: new Set<string>(),
}));
vi.mock("../api", async (orig) => ({
  ...(await orig<typeof import("../api")>()),
  api: {
    importPaamonim: vi.fn(async () => ({ transactions_added: 12, transactions_skipped: 30, categories_added: 1 })),
    exportSheets: vi.fn(async () => ({ transactions: 5, tabs: 1 })),
    setLogUserName: vi.fn(async () => {}),
    rejectSuggestion: vi.fn(async () => {}),
    discardSuggestion: vi.fn(async () => {}),
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

describe("the email inbox", () => {
  const suggestion = {
    id: 7,
    message_id: "g7",
    status: "pending" as const,
    received: "2026-03-02",
    sender: "Shop <shop@example.com>",
    subject: "Your receipt",
    excerpt: "Total 12.50",
    date: "2026-03-01",
    amount_minor: -1250,
    currency: "ILS",
    category_id: null,
    note: "Corner shop",
    confidence: "high" as const,
    transaction_id: null,
    error: "",
    has_html: true,
  };

  it("shows what was found, and nothing is added until it is accepted", async () => {
    state.suggestions = [suggestion];
    state.view = "emailInbox";
    app.render();
    expect(await screen.findByText("Corner shop")).toBeTruthy();
    expect(screen.getByText(/בלי קטגוריה/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "בדיקה והוספה" })).toBeTruthy();
  });

  it("rejects a suggestion through the API", async () => {
    const { api } = await import("../api");
    state.suggestions = [suggestion];
    state.view = "emailInbox";
    app.render();
    fireEvent.click(await screen.findByRole("button", { name: "דחייה" }));
    await waitFor(() => expect(api.rejectSuggestion).toHaveBeenCalledWith(7));
  });

  it("lets a message the AI has not read yet be retried or removed", async () => {
    state.suggestions = [{ ...suggestion, id: 9, status: "queued", error: "the AI service answered 503" }];
    state.view = "emailInbox";
    app.render();
    expect(await screen.findByText("ההודעה עדיין לא נקראה")).toBeTruthy();
    expect(screen.getByRole("button", { name: "קריאה עכשיו" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "הסרה" })).toBeTruthy();
  });

  it("offers manual entry for a message that could not be read", async () => {
    state.suggestions = [{ ...suggestion, id: 8, status: "error", error: "no answer" }];
    state.view = "emailInbox";
    app.render();
    expect(await screen.findByText("לא הצלחתי לקרוא את ההודעה")).toBeTruthy();
    expect(screen.getByRole("button", { name: "הוספה ידנית" })).toBeTruthy();
  });

  it("shows each AI call with its price and the total", async () => {
    state.aiCost = {
      total_micro_usd: 2400,
      items: [
        { at: new Date().toISOString(), sender: "a@b", subject: "Your receipt", input_tokens: 1000, output_tokens: 150, cost_micro_usd: 160, result: "1 suggestions" },
        { at: new Date().toISOString(), sender: "a@b", subject: "Newsletter", input_tokens: 5000, output_tokens: 20, cost_micro_usd: 2240, result: "ignored" },
      ],
    };
    state.view = "aiCost";
    app.render();
    expect(await screen.findByText("$0.002400")).toBeTruthy();
    expect(screen.getByText("Your receipt")).toBeTruthy();
    expect(screen.getByText("$0.000160")).toBeTruthy();
    expect(screen.getByText(/לא נמצאה תנועה/)).toBeTruthy();
  });
});
