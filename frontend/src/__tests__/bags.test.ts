import { describe, expect, it } from "vitest";
import { type Bag, type Transaction } from "../api";
import { bagDepth, bagFamily, bagPath, bagTransactions, dailyAverage, isArchived, netSpent, parentChoices, toBagState } from "../bags";

const bag = (id: number, name: string, parent: number | null = null, extra: Partial<Bag> = {}): Bag => ({ id, name, parent_id: parent, archived: false, budget_agorot: null, start_date: null, end_date: null, ...extra });
const tx = (id: number, amount: number): Transaction => ({ id, date: "2026-09-03", amount_agorot: amount, category_id: null, payment_method_id: null, note: "", sync_state: "synced" });

// Vacations (1) › Japan 2026 (2) › Kyoto (3); Dinners (4)
const state = toBagState({
  bags: [bag(1, "חופשות"), bag(2, "יפן 2026", 1), bag(3, "קיוטו", 2), bag(4, "ארוחות זוגיות")],
  links: [
    [10, 2],
    [11, 3],
    [12, 2],
    [12, 4],
    [13, 4],
  ],
});

describe("bags", () => {
  it("names a bag by its path and knows how deep it is", () => {
    expect(bagPath(state, 3)).toBe("חופשות › יפן 2026 › קיוטו");
    expect(bagDepth(state, 1)).toBe(1);
    expect(bagDepth(state, 3)).toBe(3);
  });

  it("collects a bag with everything under it", () => {
    expect([...bagFamily(state, 1)].sort()).toEqual([1, 2, 3]);
    expect([...bagFamily(state, 4)]).toEqual([4]);
  });

  it("counts a line once in a parent even when it is in two of its bags", () => {
    const s = toBagState({ bags: state.bags, links: [[10, 2], [10, 3]] });
    expect(bagTransactions(s, [tx(10, -500)], 1)).toHaveLength(1);
  });

  it("lets a line sit in two unrelated bags, counted in each", () => {
    const txs = [tx(10, -1000), tx(11, -2000), tx(12, -300), tx(13, -50)];
    expect(netSpent(bagTransactions(state, txs, 1))).toBe(3300); // 10, 11, 12
    expect(netSpent(bagTransactions(state, txs, 4))).toBe(350); // 12, 13
    // Overlap: 12 is in both, so the two bags add up to more than the lines cost.
    expect(netSpent(txs)).toBe(3350);
  });

  it("ignores queued and upcoming lines, and lets a refund lower the total", () => {
    expect(bagTransactions(state, [tx(0, -1), tx(-3, -1)], 1)).toHaveLength(0);
    expect(netSpent([tx(10, -1000), tx(11, 400)])).toBe(600);
  });

  it("offers as parents only bags that keep the nesting within three levels", () => {
    // A new bag fits under Vacations (level 1) and Japan (level 2), not under Kyoto (level 3).
    expect(parentChoices(state, null).map((b) => b.id).sort()).toEqual([1, 2, 4]);
    // Japan (with Kyoto below it, 2 levels) can go under Dinners only, not under itself or its own sub-bag.
    expect(parentChoices(state, 2).map((b) => b.id).sort()).toEqual([1, 4]);
  });

  it("treats a bag inside an archived one as archived", () => {
    const s = toBagState({ bags: [bag(1, "a", null, { archived: true }), bag(2, "b", 1)], links: [] });
    expect(isArchived(s, 2)).toBe(true);
    expect(isArchived(state, 2)).toBe(false);
  });

  it("averages per day over the bag's dates, to today while it is still going", () => {
    const trip = bag(1, "t", null, { start_date: "2026-09-02", end_date: "2026-09-14" });
    expect(dailyAverage(trip, 1_300_000, "2026-12-01")).toBe(100_000); // 13 days
    expect(dailyAverage(trip, 300_000, "2026-09-04")).toBe(100_000); // 3 days so far
    expect(dailyAverage(bag(2, "u"), 100, "2026-09-04")).toBeNull();
  });
});
