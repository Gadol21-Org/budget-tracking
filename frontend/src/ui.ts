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
