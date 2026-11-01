import { categoryById, categoryName, categoryPath } from "./categories";
import { formatShekels } from "./format";
import { el } from "./ui";
import { contains, monthOf } from "./period";
import { decimals, type Fx, type Transaction } from "./api";
import { state } from "./state";

/** The foreign-currency details of a line, if it was entered in USD or JPY. */
export const fxOf = (t: Transaction): Fx | undefined => {
  if (!t.upcoming) return state.fx.get(t.id);
  // A foreign repeating charge not yet made: priced at the newest rate until its day comes.
  const u = t.upcoming_fx;
  if (!u) return undefined;
  const units = Math.abs(u.orig_minor) / 10 ** decimals(u.currency);
  return { transaction_id: 0, currency: u.currency, orig_minor: u.orig_minor, rate: Math.abs(t.amount_agorot) / 100 / units, rate_date: t.date, estimated: true };
};

export const money = (agorot: number, cls = "") => el("span", { className: `num ${cls}`.trim(), textContent: formatShekels(agorot) });

export const catById = (id: number | null) => categoryById(state.cats, id);

export const catName = (id: number | null) => categoryName(state.cats, id);

/** "parent › child" for sub-categories. */
export const catPath = (id: number | null) => categoryPath(state.cats, id);

export const dayTitle = (d: string) =>
  new Date(`${d}T00:00:00`).toLocaleDateString("he-IL", { weekday: "long", day: "numeric", month: "long" });

/** After saving, keep the period the user is viewing; switch to the item's month only if it would otherwise be out of view. */
export function showPeriodFor(date: string) {
  if (!contains(state.period, date)) state.period = monthOf(date);
}

export const inPeriod = () => state.txs.filter((t) => contains(state.period, t.date));

export function inFilter(t: Transaction): boolean {
  if (!state.filter) return true;
  const id = state.filter.id;
  return t.category_id === id || (id !== null && catById(t.category_id)?.parent_id === id);
}
