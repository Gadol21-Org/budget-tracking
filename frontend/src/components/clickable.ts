import { type KeyboardEvent } from "react";

/** Props that make a row act as a button: click, Enter and Space. */
export function clickableProps(onActivate: () => void) {
  return {
    tabIndex: 0,
    role: "button",
    onClick: onActivate,
    onKeyDown: (e: KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
  };
}
