import { useState } from "react";
import { bagPath } from "../bags";
import { BagChoice } from "../dialogs/BagPicker";
import { state } from "../state";

/**
 * The bags a line is in, as chips; "+" opens the list of bags right here in the form (a second dialog on top
 * of the form would fight with the back button, which closes dialogs one by one).
 */
export function BagField({ value, onChange }: { value: number[]; onChange: (ids: number[]) => void }) {
  const [open, setOpen] = useState(false);
  const toggle = (id: number) => onChange(value.includes(id) ? value.filter((v) => v !== id) : [...value, id]);
  return (
    <div className="bagfield">
      <div className="muted small">טיוגים</div>
      <div className="qchips">
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
            {value.length ? "＋ עוד" : "＋ הוספה לטיוג"}
          </button>
        )}
      </div>
      {open && (
        <div className="bagpanel">
          <BagChoice picked={new Set(value)} onToggle={toggle} canCreate />
          <button type="button" className="btn" onClick={() => setOpen(false)}>
            סיום
          </button>
        </div>
      )}
    </div>
  );
}
