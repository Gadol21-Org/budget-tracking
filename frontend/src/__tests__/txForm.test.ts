import { describe, expect, it } from "vitest";
import type { Transaction } from "../api";
import { buildRecurringRule, buildTxBody, startAmountText, type BodyInput } from "../dialogs/txForm";

const base: BodyInput = {
  date: "2026-10-05",
  amountText: "12.5",
  isIncome: false,
  categoryId: () => 7,
  paymentMethodId: null,
  note: "n",
  currency: "ILS",
  origMinor: 0,
  ilsText: "",
  ilsTouched: false,
  repeating: false,
  forceLookup: false,
  fxUnchanged: false,
  savedAgorot: undefined,
};

describe("buildTxBody", () => {
  it("signs shekel amounts by kind", () => {
    expect(buildTxBody(base)).toMatchObject({ amount_agorot: -1250, category_id: 7 });
    expect(buildTxBody({ ...base, isIncome: true }).amount_agorot).toBe(1250);
  });

  it("reports a missing amount before a missing category", () => {
    const noCat = () => {
      throw new Error("יש לבחור קטגוריה");
    };
    expect(() => buildTxBody({ ...base, amountText: "0", categoryId: noCat })).toThrow("יש להזין סכום");
    expect(() => buildTxBody({ ...base, categoryId: noCat })).toThrow("יש לבחור קטגוריה");
  });

  it("sends the original foreign amount and lets the server price it", () => {
    const b = buildTxBody({ ...base, currency: "USD", origMinor: 2500 });
    expect(b).toMatchObject({ currency: "USD", orig_minor: -2500 });
    expect(b.amount_agorot).toBeUndefined();
  });

  it("uses a typed shekel amount, but not for a repeating charge", () => {
    const typed = { ...base, currency: "USD" as const, origMinor: 2500, ilsTouched: true, ilsText: "90" };
    expect(buildTxBody(typed).amount_agorot).toBe(-9000);
    expect(buildTxBody({ ...typed, repeating: true }).amount_agorot).toBeUndefined();
    expect(() => buildTxBody({ ...typed, ilsText: "0" })).toThrow("יש להזין סכום בשקלים");
  });

  it("keeps a saved line's shekel amount when nothing about it changed, unless the rate is refreshed", () => {
    const same = { ...base, currency: "USD" as const, origMinor: 2500, fxUnchanged: true, savedAgorot: -9000 };
    expect(buildTxBody(same).amount_agorot).toBe(-9000);
    expect(buildTxBody({ ...same, forceLookup: true }).amount_agorot).toBeUndefined();
  });
});

describe("buildRecurringRule", () => {
  const body = { date: "2026-10-15", amount_agorot: -5000, category_id: 7, payment_method_id: null, note: "rent" };
  const input = { body, categoryId: 7, date: "2026-10-15", duration: "forever", months: 12, seriesMonth: undefined, segmentDay: undefined };

  it("starts in the typed month on the typed day", () => {
    expect(buildRecurringRule(input)).toMatchObject({ start_month: "2026-10", end_month: null, day: 15, amount_agorot: -5000, currency: null, orig_minor: null });
  });

  it("ends after the chosen number of months, counting the first", () => {
    expect(buildRecurringRule({ ...input, duration: "months", months: 3 }).end_month).toBe("2026-12");
    expect(() => buildRecurringRule({ ...input, duration: "months", months: 0 })).toThrow("מספר החודשים אינו תקין");
  });

  it("changes a series from the occurrence's month, keeping the rule's day if the date moved out of it", () => {
    const r = buildRecurringRule({ ...input, date: "2026-11-20", seriesMonth: "2026-10", segmentDay: 3 });
    expect(r).toMatchObject({ start_month: "2026-10", day: 3 });
  });

  it("stands in a sign for a foreign rule's shekel amount", () => {
    const fx = { ...body, amount_agorot: undefined, currency: "USD" as const, orig_minor: -2500 };
    expect(buildRecurringRule({ ...input, body: fx })).toMatchObject({ amount_agorot: -1, currency: "USD", orig_minor: -2500 });
  });
});

describe("startAmountText", () => {
  const tx = { amount_agorot: -4550 } as Transaction;
  it("shows shekels, the original foreign amount, or nothing", () => {
    expect(startAmountText(tx, undefined, undefined)).toBe("45.5");
    expect(startAmountText(tx, { currency: "USD", orig_minor: -2500 }, undefined)).toBe("25");
    expect(startAmountText(tx, undefined, { currency: "JPY", orig_minor: -3000 })).toBe("3000");
    expect(startAmountText(undefined, undefined, undefined)).toBe("");
  });
});
