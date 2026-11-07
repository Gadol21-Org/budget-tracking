import { type MouseEvent, useState } from "react";
import { Money } from "./Money";
import { lineChart } from "../insights/charts";

/**
 * A running total through a month against another series (usually the previous month). Tap a day to read both values for it;
 * until then the readout shows the latest day that has data (today, for the current month).
 */
export function CumulativeChart({
  days,
  cur,
  prev,
  color,
  curLabel,
  prevLabel,
  prevColor = "var(--muted)",
  signed = false,
}: {
  /** x-axis labels, one per day (or month). */
  days: string[];
  /** Running total up to the latest day with data (may be shorter than `days`). */
  cur: number[];
  /** The comparison, one value per day. */
  prev?: number[];
  color: string;
  curLabel: string;
  prevLabel: string;
  prevColor?: string;
  /** Colour the figures green/red by sign (profit), instead of a single colour. */
  signed?: boolean;
}) {
  const last = Math.max(0, cur.length - 1);
  const [picked, setPicked] = useState<number | null>(null);
  const sel = Math.min(picked ?? last, days.length - 1);
  const cls = (v: number) => (signed ? (v < 0 ? "expense" : "income") : "");
  const figure = (v: number | undefined) => (v == null ? <span className="muted">—</span> : <Money agorot={Math.round(v)} whole cls={cls(v)} />);
  return (
    <>
      <div
        className="chart"
        dangerouslySetInnerHTML={{ __html: lineChart(cur, days, color, prev, 5, sel) }}
        onClick={(e: MouseEvent) => {
          const hit = (e.target as Element).closest("[data-i]");
          if (hit) {
            const i = Number((hit as HTMLElement).dataset.i);
            setPicked(i === picked ? null : i);
          }
        }}
      />
      <div className="cumread" role="status">
        <b>{`היום ה-${days[sel]}`}</b>
        <span>
          <i style={{ background: color }} />
          {curLabel} {figure(cur[sel])}
        </span>
        {prev && (
          <span>
            <i style={{ background: prevColor }} />
            {prevLabel} {figure(prev[sel])}
          </span>
        )}
      </div>
    </>
  );
}
