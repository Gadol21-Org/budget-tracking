import { fmtDate } from "./ui";
/** The span of time the summary and list show. */
export type Period =
  | { kind: "month"; month: string } // YYYY-MM
  | { kind: "year"; year: number }
  | { kind: "range"; from: string; to: string }; // YYYY-MM-DD, inclusive

export const monthOf = (date: string): Period => ({ kind: "month", month: date.slice(0, 7) });

/** Inclusive date bounds; ISO dates compare correctly as strings. */
export function bounds(p: Period): { from: string; to: string } {
  switch (p.kind) {
    case "month":
      return { from: `${p.month}-01`, to: `${p.month}-31` };
    case "year":
      return { from: `${p.year}-01-01`, to: `${p.year}-12-31` };
    case "range":
      return { from: p.from, to: p.to };
  }
}

export function contains(p: Period, date: string): boolean {
  const { from, to } = bounds(p);
  return date >= from && date <= to;
}

/** Previous (-1) or next (+1) month or year. A custom range has no neighbours. */
export function shift(p: Period, delta: number): Period | null {
  switch (p.kind) {
    case "month": {
      const [y, m] = p.month.split("-").map(Number);
      return { kind: "month", month: new Date(Date.UTC(y, m - 1 + delta, 1)).toISOString().slice(0, 7) };
    }
    case "year":
      return { kind: "year", year: p.year + delta };
    case "range":
      return null;
  }
}

const day = fmtDate;

export function title(p: Period): string {
  switch (p.kind) {
    case "month":
      return new Date(`${p.month}-01T00:00:00`).toLocaleDateString("he-IL", { month: "long", year: "numeric" });
    case "year":
      return String(p.year);
    case "range":
      return `${day(p.from)} – ${day(p.to)}`;
  }
}

export const MONTHS = Array.from({ length: 12 }, (_, i) =>
  new Date(2000, i, 1).toLocaleDateString("he-IL", { month: "long" }),
);
