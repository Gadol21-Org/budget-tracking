/** Number and date formatting shared by the screens (Hebrew locale, shekel amounts in agorot). */

/** A shekel amount with agorot, e.g. ‏12.50 ₪. */
export function formatShekels(agorot: number): string {
  return (agorot / 100).toLocaleString("he-IL", { style: "currency", currency: "ILS" });
}

/** Millionths of a US dollar as dollars, with the decimals a single AI call needs (it costs a fraction of a cent), e.g. $0.000160. */
export const formatUsd = (micro: number): string => (micro / 1e6).toLocaleString("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 6, maximumFractionDigits: 6 });

/** What an AI call found, from the result the server logged (`ignored`, `unreadable` or `N suggestions`). */
export function aiResultText(r: string): string {
  if (r === "ignored") return "לא נמצאה תנועה";
  if (r === "unreadable") return "תשובה לא קריאה";
  const n = Number.parseInt(r, 10);
  return n === 1 ? "נמצאה תנועה אחת" : Number.isFinite(n) ? `נמצאו ${n} תנועות` : r;
}

/** A shekel amount rounded to whole shekels, for charts and dashboards. */
export const formatShekelsWhole = (agorot: number): string => (agorot / 100).toLocaleString("he-IL", { style: "currency", currency: "ILS", maximumFractionDigits: 0 });

/** "אוק׳" for a YYYY-MM month. */
export const shortMonth = (m: string) => new Date(`${m}-01T00:00:00`).toLocaleDateString("he-IL", { month: "short" });

/** "אוקטובר 2026" for a YYYY-MM month. */
export const longMonth = (m: string) => new Date(`${m}-01T00:00:00`).toLocaleDateString("he-IL", { month: "long", year: "numeric" });
