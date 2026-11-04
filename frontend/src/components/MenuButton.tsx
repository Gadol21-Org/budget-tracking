import { useState } from "react";
import { mountReact } from "./mount";

export type MenuItem = [label: string, run: () => void];

/** A button that opens a small menu of choices above it. */
export function MenuButton({ label, items, className = "btn" }: { label: string; items: MenuItem[]; className?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="menuwrap">
      <button type="button" className={className} onClick={() => setOpen(!open)}>
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

/** For the screens not yet written in React: the same menu as a DOM node. */
export const menuButton = (label: string, items: MenuItem[], className = "btn") => mountReact(<MenuButton label={label} items={items} className={className} />, "menuhost");
