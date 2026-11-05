import { useState } from "react";
import { type BizData } from "../business";

interface Props {
  biz: BizData;
  /** undefined = nothing chosen yet (only while a choice is required); null = None. */
  value: number | null | undefined;
  onChange: (value: number | null) => void;
  /** A line must say which business it is in: the buttons show with nothing chosen. */
  required: boolean;
  /** The business the category already gives (no choice), or null for the choice. */
  givenName: string | null;
  /** Start as a "+ mark for a business" link instead of the open row. */
  collapsed?: boolean;
}

/** Pick the business a line counts in: None or one of the businesses, as chips. */
export function BusinessChips({ biz, value, onChange, required, givenName, collapsed = false }: Props) {
  const [expanded, setExpanded] = useState(!collapsed);
  const showRow = givenName == null && (expanded || required || value != null);
  const options = [{ id: null as number | null, name: "ללא" }, ...biz.businesses.map((b) => ({ id: b.id as number | null, name: b.name }))];
  return (
    <div className="bizpick">
      <button type="button" className="linkbtn" hidden={givenName != null || showRow} onClick={() => setExpanded(true)}>
        + סימון לעסק
      </button>
      <div className="muted small" hidden={!(showRow || givenName != null)}>
        {required && givenName == null ? "עסק (חובה לבחור)" : "עסק"}
      </div>
      <div className="qchips" hidden={!showRow}>
        {options.map((o) => (
          <button key={String(o.id)} type="button" className="qchip" aria-pressed={o.id === value} onClick={() => onChange(o.id)}>
            {o.name}
          </button>
        ))}
      </div>
      <div hidden={givenName == null}>
        {givenName != null && (
          <>
            <span className="bizchip big">{givenName}</span>
            <span className="muted small"> לפי הקטגוריה</span>
          </>
        )}
      </div>
    </div>
  );
}
