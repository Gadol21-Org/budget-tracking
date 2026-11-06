/**
 * When a pick list (category, sub-category, currency) opens with the keyboard up, make the page as tall as the visible area
 * above the keyboard (as the browser reports it) and scroll the field's label to the top, so the list under the field is
 * not hidden behind the keyboard. Nothing is added to the page; plain fields (amount, note, date) are left to the browser.
 */
export let restorePage = () => {};

export function revealField(input: HTMLElement) {
  const dlg = input.closest("dialog");
  const vv = window.visualViewport;
  if (!dlg) return;
  const top = input.closest("label") ?? input;
  const go = () => {
    // Where the browser does not shrink the page for the keyboard, the visible area is smaller than the window.
    if (vv && vv.height < window.innerHeight - 100) {
      dlg.style.height = `${vv.height}px`;
      dlg.style.top = `${vv.offsetTop}px`;
    } else {
      dlg.style.removeProperty("height");
      dlg.style.removeProperty("top");
    }
    dlg.scrollTo({ top: dlg.scrollTop + top.getBoundingClientRect().top - dlg.getBoundingClientRect().top - dlg.clientTop, behavior: "smooth" });
  };
  restorePage();
  go();
  const timer = setTimeout(go, 350);
  vv?.addEventListener("resize", go);
  restorePage = () => {
    clearTimeout(timer);
    vv?.removeEventListener("resize", go);
    dlg.style.removeProperty("height");
    dlg.style.removeProperty("top");
    restorePage = () => {};
  };
}

export const isPick = (t: EventTarget | null): t is HTMLElement => t instanceof HTMLElement && t.getAttribute("role") === "combobox" && !!t.closest("dialog");

document.addEventListener("focusin", (e) => isPick(e.target) && revealField(e.target));

document.addEventListener("focusout", () => setTimeout(() => !isPick(document.activeElement) && restorePage(), 50));

// iOS can leave the fixed bottom bar where the keyboard was after it closes. A one-pixel scroll makes the browser lay the page out again.
document.addEventListener("focusout", () =>
  setTimeout(() => {
    const a = document.activeElement;
    if (a instanceof HTMLInputElement || a instanceof HTMLTextAreaElement || a instanceof HTMLSelectElement) return;
    window.scrollBy(0, 1);
    window.scrollBy(0, -1);
  }, 150),
);
