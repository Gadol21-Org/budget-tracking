/** Number and date formatting shared by the screens (Hebrew locale, shekel amounts in agorot). */

/** A shekel amount with agorot, e.g. ‏12.50 ₪. */
export function formatShekels(agorot: number): string {
  return (agorot / 100).toLocaleString("he-IL", { style: "currency", currency: "ILS" });
}

/** A shekel amount rounded to whole shekels, for charts and dashboards. */
export const formatShekelsWhole = (agorot: number): string => (agorot / 100).toLocaleString("he-IL", { style: "currency", currency: "ILS", maximumFractionDigits: 0 });

/** "אוק׳" for a YYYY-MM month. */
export const shortMonth = (m: string) => new Date(`${m}-01T00:00:00`).toLocaleDateString("he-IL", { month: "short" });

/** "אוקטובר 2026" for a YYYY-MM month. */
export const longMonth = (m: string) => new Date(`${m}-01T00:00:00`).toLocaleDateString("he-IL", { month: "long", year: "numeric" });
