import { el } from "./ui";
import { contains } from "./period";
import { go } from "./nav";
import { root } from "./state";

/** A button that opens a small menu of choices above it. */
export function menuButton(label: string, items: [string, () => void][], className = "btn"): HTMLElement {
  const menu = el("div", { className: "menu", hidden: true }, ...items.map(([text, run]) => {
    const item = el("button", { type: "button", textContent: text });
    item.onclick = () => ((menu.hidden = true), run());
    return item;
  }));
  const button = el("button", { type: "button", className, textContent: `${label} ▾` });
  button.onclick = () => (menu.hidden = !menu.hidden);
  return el("div", { className: "menuwrap" }, button, menu);
}

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

export interface ComboItem {
  value: string;
  label: string;
}

/**
 * A text field with a drop-down list that narrows as you type. Picking an item sets `value`;
 * leaving the field with text that matches nothing puts the picked label back.
 */
export function combo(placeholder: string) {
  let items: ComboItem[] = [];
  let value = "";
  const input = el("input", { type: "text", placeholder, autocomplete: "off", spellcheck: false });
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-expanded", "false");
  const list = el("div", { className: "combo-list", hidden: true });
  list.setAttribute("role", "listbox");
  const root = el("div", { className: "combo" }, input, list);
  const labelOf = (v: string) => items.find((i) => i.value === v)?.label ?? "";
  const close = () => {
    list.hidden = true;
    input.setAttribute("aria-expanded", "false");
    input.value = labelOf(value);
    document.removeEventListener("click", outside, true);
  };
  /** Only a tap elsewhere closes the list: dismissing the phone keyboard also blurs the field and must not, and neither must scrolling (a scroll gesture fires pointerdown but no click). */
  const outside = (e: Event) => (root.isConnected ? root.contains(e.target as Node) || close() : document.removeEventListener("click", outside, true));
  /** When the last item was picked: the tap's own click may land on the field once the layout shifts, and must not reopen the list. */
  let pickedAt = 0;
  const pick = (v: string) => {
    value = v;
    input.value = labelOf(v);
    close();
  };
  const show = (filter: string) => {
    const q = filter.trim().toLowerCase();
    const shown = items.filter((i) => !q || i.label.toLowerCase().includes(q));
    list.replaceChildren(
      ...(shown.length
        ? shown.map((i) => {
            const b = el("button", { type: "button", textContent: i.label });
            b.setAttribute("role", "option");
            b.setAttribute("aria-selected", String(i.value === value));
            // mousedown, so the pick lands before the input's blur closes the list.
            b.onmousedown = (e) => (e.preventDefault(), (pickedAt = Date.now()), pick(i.value), input.blur(), input.dispatchEvent(new Event("change")));
            return b;
          })
        : [el("div", { className: "combo-empty", textContent: "אין תוצאות" })]),
    );
    list.hidden = false;
    input.setAttribute("aria-expanded", "true");
    document.addEventListener("click", outside, true);
  };
  const justPicked = () => Date.now() - pickedAt <= 500;
  input.onfocus = () => {
    // The layout can shift after a pick (the shekel box hides) so the tap's click lands on the field: stay closed.
    if (justPicked()) return input.blur();
    input.select();
    show("");
  };
  input.onclick = () => {
    if (justPicked()) return input.blur();
    if (list.hidden) show("");
  };
  input.oninput = () => show(input.value);
  input.onkeydown = (e) => {
    if (e.key === "Tab" && !list.hidden) close();
    if (e.key === "Escape" && !list.hidden) (e.stopPropagation(), e.preventDefault(), close());
    if (e.key === "Enter" && !list.hidden) {
      // Enter takes the first match instead of submitting the form.
      e.preventDefault();
      const first = list.querySelector<HTMLButtonElement>("button");
      if (first) first.dispatchEvent(new MouseEvent("mousedown", { cancelable: true }));
    }
  };
  return {
    root,
    input,
    get value() {
      return value;
    },
    set(newItems: ComboItem[], v = "") {
      items = newItems;
      pick(v);
    },
    pick,
    setDisabled(d: boolean, ph = placeholder) {
      input.disabled = d;
      input.placeholder = ph;
      if (d) close();
    },
  };
}
