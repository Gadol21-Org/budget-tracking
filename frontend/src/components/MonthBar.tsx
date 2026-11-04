import { title as periodTitle, type Period } from "../period";

interface Props {
  period: Period;
  /** Move to the previous (-1) or next (1) period. */
  onShift: (delta: number) => void;
  /** Open the period picker. */
  onPick: () => void;
}

/** The period switcher at the top of the summary, list and dashboards. */
export function MonthBar({ period, onShift, onPick }: Props) {
  // A custom range has no neighbours; keep the layout steady.
  const hidden = period.kind === "range" ? { visibility: "hidden" as const } : undefined;
  // RTL: the first element is on the right and goes back in time, the last (left) goes forward.
  return (
    <div className="monthbar">
      <button className="iconbtn" title="הקודם" style={hidden} onClick={() => onShift(-1)}>
        →
      </button>
      <button className="title periodbtn" title="בחירת תקופה" onClick={onPick}>
        {`${periodTitle(period)} ▾`}
      </button>
      <button className="iconbtn" title="הבא" style={hidden} onClick={() => onShift(1)}>
        ←
      </button>
    </div>
  );
}
