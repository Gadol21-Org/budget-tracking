import { type ReactNode } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { trackDialog } from "../nav";

/**
 * Open a modal whose content is a React tree; `render` gets a way to close it. The modal takes a history
 * entry while open (back closes it), and the tree is released when it closes.
 */
export function openReactDialog(render: (close: () => void) => ReactNode, className?: string) {
  const dlg = document.createElement("dialog");
  if (className) dlg.className = className;
  const root = createRoot(dlg);
  // Rendered before it opens, so the dialog is complete when it appears.
  flushSync(() => root.render(render(() => dlg.close())));
  dlg.addEventListener("close", () => {
    dlg.remove();
    setTimeout(() => root.unmount());
  });
  document.body.append(dlg);
  trackDialog(dlg);
  // Do not put the cursor (and the keyboard) on a field when the dialog opens.
  dlg.tabIndex = -1;
  dlg.setAttribute("autofocus", "");
  dlg.showModal();
  if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
}
