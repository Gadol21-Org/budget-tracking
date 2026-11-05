import { type Bag, type BagData, type Transaction } from "./api";

/**
 * Bags (טיוגים) group transactions across categories, nested like "Vacations › Japan 2026". They are a lens
 * over the data, like businesses: nothing moves out of the household figures. A transaction can sit in several
 * bags; a bag's figures count it once, together with everything in the bags under it.
 */
export interface BagState {
  bags: Bag[];
  /** transaction id -> the bags it is in */
  tx: Map<number, number[]>;
}

export const MAX_DEPTH = 3;

export const emptyBags = (): BagState => ({ bags: [], tx: new Map() });

export function toBagState(d: BagData): BagState {
  const tx = new Map<number, number[]>();
  for (const [t, b] of d.links) tx.set(t, [...(tx.get(t) ?? []), b]);
  return { bags: d.bags, tx };
}

export const bagById = (s: BagState, id: number | null | undefined) => s.bags.find((b) => b.id === id);

/** "Vacations › Japan 2026". */
export function bagPath(s: BagState, id: number): string {
  const names: string[] = [];
  let cur = bagById(s, id);
  while (cur && names.length < MAX_DEPTH + 2) {
    names.unshift(cur.name);
    cur = bagById(s, cur.parent_id);
  }
  return names.join(" › ");
}

/** 1 for a top-level bag. */
export function bagDepth(s: BagState, id: number): number {
  let d = 0;
  let cur = bagById(s, id);
  while (cur && d < MAX_DEPTH + 2) {
    d++;
    cur = bagById(s, cur.parent_id);
  }
  return d;
}

/** How many levels the bag and everything under it take (1 for a bag with no sub-bags). */
export function bagHeight(s: BagState, id: number): number {
  return 1 + Math.max(0, ...s.bags.filter((b) => b.parent_id === id).map((b) => bagHeight(s, b.id)));
}

export const childBags = (s: BagState, parent: number | null) => s.bags.filter((b) => b.parent_id === parent);

/** The bag and every bag under it. */
export function bagFamily(s: BagState, id: number): Set<number> {
  const out = new Set<number>([id]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const b of s.bags) {
      if (b.parent_id != null && out.has(b.parent_id) && !out.has(b.id)) {
        out.add(b.id);
        grew = true;
      }
    }
  }
  return out;
}

/** Bags a transaction can be put in when the bag `id` is being edited: not itself or anything under it, and not so deep that its own sub-bags would pass the limit. */
export function parentChoices(s: BagState, id: number | null): Bag[] {
  const own = id != null ? bagFamily(s, id) : new Set<number>();
  const height = id != null ? bagHeight(s, id) : 1;
  // Anything put under the business root would become a business, so the root and the businesses are not offered.
  return s.bags.filter((b) => !own.has(b.id) && !isBusinessBag(s, b.id) && bagDepth(s, b.id) + height <= MAX_DEPTH);
}

/** The bag all businesses sit under (named עסקים, fixed). */
export const businessRoot = (s: BagState) => s.bags.find((b) => b.role === "business_root");

/** The root itself, or a business (a bag directly under it). Businesses are managed with their own screens and chips, so the bag pickers and chips leave them out. */
export function isBusinessBag(s: BagState, id: number): boolean {
  const b = bagById(s, id);
  const root = businessRoot(s);
  return !!b && !!root && (b.id === root.id || b.parent_id === root.id);
}

/** The lines in the bag or in any bag under it, each once. */
export function bagTransactions(s: BagState, txs: Transaction[], id: number): Transaction[] {
  const fam = bagFamily(s, id);
  return txs.filter((t) => t.id > 0 && (s.tx.get(t.id) ?? []).some((b) => fam.has(b)));
}

/** What the lines cost net: expenses minus income (a refund inside a bag lowers it). */
export const netSpent = (ts: Transaction[]) => 0 - ts.reduce((sum, t) => sum + t.amount_agorot, 0);

/** The bags a line is in, as bags (a business shows by its own chip, so it is left out). */
export const bagsOf = (s: BagState, t: Transaction): number[] => (t.id > 0 ? (s.tx.get(t.id) ?? []) : []).filter((b) => !isBusinessBag(s, b));

const dayMs = 86_400_000;
const parse = (d: string) => Date.parse(`${d}T00:00:00Z`);

/** Average net spending per day across the bag's dates (up to today for a bag still going); null without a start date. */
export function dailyAverage(b: Bag, spent: number, today: string): number | null {
  if (!b.start_date) return null;
  const end = b.end_date && b.end_date < today ? b.end_date : today;
  const days = Math.floor((parse(end) - parse(b.start_date)) / dayMs) + 1;
  return days >= 1 ? Math.round(spent / days) : null;
}

/** A bag's dates for display, "2 ספט׳ – 14 ספט׳ 2026". */
export function dateRange(b: Bag): string {
  const f = (d: string, year: boolean) => new Date(`${d}T00:00:00`).toLocaleDateString("he-IL", { day: "numeric", month: "short", ...(year ? { year: "numeric" } : {}) });
  if (b.start_date && b.end_date) return `${f(b.start_date, false)} – ${f(b.end_date, true)}`;
  if (b.start_date) return `מ-${f(b.start_date, true)}`;
  if (b.end_date) return `עד ${f(b.end_date, true)}`;
  return "";
}

/** Archived itself, or inside an archived bag: it drops out of the pickers and the active list. */
export function isArchived(s: BagState, id: number): boolean {
  let cur = bagById(s, id);
  for (let i = 0; cur && i <= MAX_DEPTH + 1; i++) {
    if (cur.archived) return true;
    cur = bagById(s, cur.parent_id);
  }
  return false;
}
