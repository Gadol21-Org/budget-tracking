export type Child = Node | string | null | false;

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Record<string, unknown> = {},
  ...kids: Child[]
): HTMLElementTagNameMap[K] {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...kids.filter((k): k is Node | string => !!k));
  return node;
}

/** Make a row act as a button: click, Enter and Space. */
export function clickable<T extends HTMLElement>(node: T, onActivate: () => void): T {
  node.classList.add("clickable");
  node.tabIndex = 0;
  node.setAttribute("role", "button");
  node.onclick = onActivate;
  node.onkeydown = (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onActivate();
    }
  };
  return node;
}

/** An ISO date (yyyy-mm-dd) as dd/mm/yyyy, whatever the device locale. */
export function fmtDate(iso: string): string {
  const [y, m, d] = iso.split("-");
  return y && m && d ? `${d}/${m}/${y}` : "";
}

/** Wrap a native date input so it shows dd/mm/yyyy; the input keeps the ISO value and the native picker. */
export function dateField(input: HTMLInputElement): HTMLElement {
  const shown = el("span", { className: "dateshown" });
  const sync = () => (shown.textContent = fmtDate(input.value) || "dd/mm/yyyy");
  input.classList.add("datenative");
  input.addEventListener("input", sync);
  input.addEventListener("change", sync);
  input.addEventListener("click", () => {
    try {
      input.showPicker();
    } catch {
      /* the browser opens its own picker */
    }
  });
  // Callers set .value programmatically too; keep the text in step.
  const desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!;
  Object.defineProperty(input, "value", {
    get: () => desc.get!.call(input),
    set: (v: string) => {
      desc.set!.call(input, v);
      sync();
    },
  });
  sync();
  return el("div", { className: "datefield" }, shown, input);
}
