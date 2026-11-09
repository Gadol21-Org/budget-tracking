import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

export interface ComboItem {
  value: string;
  label: string;
}

interface Props {
  items: ComboItem[];
  /** The picked item's value; "" for nothing picked. */
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  disabled?: boolean;
  /** Show an X that empties the pick (for optional or re-pickable fields). */
  clearable?: boolean;
}

/**
 * A text field with a drop-down list that narrows as you type. Picking an item calls `onChange`;
 * leaving the field with text that matches nothing puts the picked label back.
 */
export function Combo({ items, value, onChange, placeholder, disabled = false, clearable = false }: Props) {
  const [openState, setOpen] = useState(false);
  // A disabled field never shows its list.
  const open = openState && !disabled;
  /** What was typed since the list opened; null shows the picked label. */
  const [typed, setTyped] = useState<string | null>(null);
  /** The list row highlighted with the arrow keys; -1 for none (Enter then takes the first match). */
  const [active, setActive] = useState(-1);
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  /** When the last item was picked: the tap's own click may land on the field once the layout shifts, and must not reopen the list. */
  const pickedAt = useRef(0);

  const label = items.find((i) => i.value === value)?.label ?? "";
  const q = (typed ?? "").trim().toLowerCase();
  const shown = items.filter((i) => !q || i.label.toLowerCase().includes(q));

  const close = useCallback(() => {
    setOpen(false);
    setTyped(null);
    setActive(-1);
  }, []);
  const justPicked = () => Date.now() - pickedAt.current <= 500;

  const pick = (v: string) => {
    pickedAt.current = Date.now();
    onChange(v);
    close();
    inputRef.current?.blur();
  };

  // Keep the highlighted row in view while arrowing through a long list.
  useEffect(() => {
    if (active >= 0) listRef.current?.querySelectorAll("button")[active]?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  // Only a tap elsewhere closes the list: dismissing the phone keyboard also blurs the field and must not,
  // and neither must scrolling (a scroll gesture fires pointerdown but no click).
  useEffect(() => {
    if (!open) return;
    // (A target that has left the page, like the X that just emptied the field, is not a tap elsewhere.)
    const outside = (e: Event) => rootRef.current?.contains(e.target as Node) || !(e.target as Node).isConnected || close();
    document.addEventListener("click", outside, true);
    return () => document.removeEventListener("click", outside, true);
  }, [open, close]);

  // Make the list as tall as the room under the field allows (above the keyboard when it is up), within sensible bounds.
  useLayoutEffect(() => {
    if (!open) return;
    const fit = () => {
      const list = listRef.current;
      const input = inputRef.current;
      if (!list || !input) return;
      const vv = window.visualViewport;
      const bottom = Math.min(vv ? vv.offsetTop + vv.height : innerHeight, innerHeight);
      const room = bottom - input.getBoundingClientRect().bottom - 16;
      list.style.maxHeight = `${Math.round(Math.min(Math.max(room, 160), 560))}px`;
    };
    fit();
    window.visualViewport?.addEventListener("resize", fit);
    // In a dialog the page scrolls the field to the top and the keyboard slides in: keep the list invisible (it still takes
    // its place) until that has settled, so it appears once at its final size instead of jumping.
    const list = listRef.current;
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (list && inputRef.current?.closest("dialog")) {
      list.classList.add("settling");
      timer = setTimeout(() => {
        fit();
        void list.offsetHeight; // apply the final size while the transition is still off
        list.classList.remove("settling");
      }, 450);
    }
    return () => {
      clearTimeout(timer);
      list?.classList.remove("settling");
      window.visualViewport?.removeEventListener("resize", fit);
    };
  }, [open]);

  return (
    <div className="combo" ref={rootRef}>
      <input
        ref={inputRef}
        type="text"
        role="combobox"
        aria-expanded={open}
        placeholder={placeholder}
        autoComplete="off"
        spellCheck={false}
        disabled={disabled}
        value={disabled ? label : (typed ?? label)}
        onFocus={(e) => {
          // The layout can shift after a pick (the shekel box hides) so the tap's click lands on the field: stay closed.
          if (justPicked()) return e.currentTarget.blur();
          e.currentTarget.select();
          setOpen(true);
        }}
        onClick={(e) => (justPicked() ? e.currentTarget.blur() : setOpen(true))}
        onChange={(e) => (setTyped(e.target.value), setActive(-1), setOpen(true))}
        onKeyDown={(e) => {
          if (e.key === "Tab" && open) close();
          if (e.key === "Escape" && open) {
            e.stopPropagation();
            e.preventDefault();
            close();
          }
          if ((e.key === "ArrowDown" || e.key === "ArrowUp") && !disabled) {
            e.preventDefault();
            if (!open) return setOpen(true);
            if (!shown.length) return;
            const step = e.key === "ArrowDown" ? 1 : -1;
            setActive(active < 0 ? (step > 0 ? 0 : shown.length - 1) : (active + step + shown.length) % shown.length);
          }
          if (e.key === "Enter" && open) {
            // Enter takes the highlighted match (else the first) instead of submitting the form.
            e.preventDefault();
            const item = shown[active] ?? shown[0];
            if (item) pick(item.value);
          }
        }}
      />
      {clearable && value && !disabled && (
        <button
          type="button"
          className="combo-clear"
          aria-label="נקה בחירה"
          // mousedown, so the field keeps focus; then reopen the list so the user can search again.
          onMouseDown={(e) => {
            e.preventDefault();
            onChange("");
            setTyped(null);
            inputRef.current?.focus();
            setOpen(true);
          }}
        >
          ×
        </button>
      )}
      <div className="combo-list" role="listbox" hidden={!open} ref={listRef}>
        {shown.length ? (
          shown.map((i, n) => (
            // mousedown, so the pick lands before the input's blur closes the list.
            <button key={i.value} type="button" role="option" aria-selected={i.value === value} data-active={n === active || undefined} onMouseDown={(e) => (e.preventDefault(), pick(i.value))}>
              {i.label}
            </button>
          ))
        ) : (
          <div className="combo-empty">אין תוצאות</div>
        )}
      </div>
    </div>
  );
}
