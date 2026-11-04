import { type ReactNode } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";

/**
 * Bridge for the incremental move to React: render a React element into a plain DOM node that the
 * existing imperative code can place like any other element. Screens are rebuilt by replacing their
 * DOM wholesale, so a mounted tree is released once its host node leaves the document. The host
 * takes no space of its own (`display: contents`), so it does not change the layout around it.
 */
export function mountReact(element: ReactNode, className = "reacthost"): HTMLElement {
  const host = document.createElement("div");
  host.className = className;
  const reactRoot = createRoot(host);
  // Rendered at once, so the caller can find and scroll to the content right after placing it.
  flushSync(() => reactRoot.render(element));
  const watcher = new MutationObserver(() => {
    if (!host.isConnected) return;
    watcher.disconnect();
    new MutationObserver((_, self) => host.isConnected || (self.disconnect(), reactRoot.unmount())).observe(document.body, { childList: true, subtree: true });
  });
  watcher.observe(document.body, { childList: true, subtree: true });
  return host;
}
