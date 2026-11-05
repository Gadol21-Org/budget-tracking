import { useState } from "react";

export type MenuItem = [label: string, run: () => void];

/** A button that opens a small menu of choices above it. */
export function MenuButton({ label, items, className = "btn", disabled = false }: { label: string; items: MenuItem[]; className?: string; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="menuwrap">
      <button type="button" className={className} disabled={disabled} onClick={() => setOpen(!open)}>
        {label} ▾
      </button>
      <div className="menu" hidden={!open}>
        {items.map(([text, run]) => (
          <button
            key={text}
            type="button"
            onClick={() => {
              setOpen(false);
              run();
            }}
          >
            {text}
          </button>
        ))}
      </div>
    </div>
  );
}
