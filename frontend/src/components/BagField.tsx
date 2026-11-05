import { useState } from "react";
import { bagPath } from "../bags";
import { BagChoice } from "../dialogs/BagPicker";
import { bizName } from "../business";
import { state } from "../state";

/**
 * The bags a line is in, as chips; "+" opens the list of bags right here in the form (a second dialog on top
 * of the form would fight with the back button, which closes dialogs one by one).
 */
export interface BizPick {
  /** undefined = nothing chosen yet (only while a choice is required); null = no business. */
  value: number | null | undefined;
  onChange: (id: number | null | undefined) => void;
  /** A line in this category must say which business it is in (or "no business"). */
  required: boolean;
  /** The business the category already gives (no choice), or null. */
  givenName: string | null;
}

export function BagField({ value, onChange, biz }: { value: number[]; onChange: (ids: number[]) => void; biz?: BizPick }) {
  const [open, setOpen] = useState(false);
  const toggle = (id: number) => onChange(value.includes(id) ? value.filter((v) => v !== id) : [...value, id]);
  return (
    <div className="bagfield">
      <div className="muted small">{biz?.required && biz.givenName == null ? "טיוגים ועסק (חובה לבחור עסק, או ״ללא עסק״)" : biz ? "טיוגים ועסק" : "טיוגים"}</div>
      <div className="qchips">
        {biz?.givenName != null && (
          <span className="bizchip big">
            {`💼 ${biz.givenName}`}
            <span className="muted small"> · לפי הקטגוריה</span>
          </span>
        )}
        {biz && biz.givenName == null && biz.value != null && (
          <span className="bizchip big">
            {`💼 ${bizName(state.biz, biz.value)}`}
            <button type="button" className="x" title="הסרה" aria-label="הסרת העסק" onClick={() => biz.onChange(null)}>
              ✕
            </button>
          </span>
        )}
        {biz?.required && biz.givenName == null && biz.value === null && (
          <span className="bizchip big">
            ללא עסק
            <button type="button" className="x" title="ביטול" aria-label="ביטול ״ללא עסק״" onClick={() => biz.onChange(undefined)}>
              ✕
            </button>
          </span>
        )}
        {biz?.required && biz.givenName == null && biz.value === undefined && (
          <button type="button" className="qchip" onClick={() => biz.onChange(null)}>
            ללא עסק
          </button>
        )}
        {value
          .filter((id) => state.bags.bags.some((b) => b.id === id))
          .map((id) => (
            <span key={id} className="bagchip big">
              {`🎒 ${bagPath(state.bags, id)}`}
              <button type="button" className="x" title="הסרה" aria-label={`הסרה מ-${bagPath(state.bags, id)}`} onClick={() => toggle(id)}>
                ✕
              </button>
            </span>
          ))}
        {!open && (
          <button type="button" className="qchip" onClick={() => setOpen(true)}>
            {value.length || (biz && biz.value != null) ? "＋ עוד" : biz ? "＋ טיוג או עסק" : "＋ הוספה לטיוג"}
          </button>
        )}
      </div>
      {open && (
        <div className="bagpanel">
          <BagChoice picked={new Set(value)} onToggle={toggle} canCreate biz={biz && biz.givenName == null ? { value: biz.value, onPick: (id) => biz.onChange(id) } : undefined} />
          <button type="button" className="btn" onClick={() => setOpen(false)}>
            סיום
          </button>
        </div>
      )}
    </div>
  );
}
