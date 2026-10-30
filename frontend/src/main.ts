import { api, formatShekels, parseAgorot, type Category, type Transaction } from "./api";

const app = document.getElementById("app")!;
let categories: Category[] = [];

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Record<string, unknown> = {},
  ...kids: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...kids);
  return node;
}

function renderRow(t: Transaction): HTMLElement {
  const cat = categories.find((c) => c.id === t.category_id)?.name ?? "—";
  const del = el("button", { textContent: "✕", title: "Delete" });
  del.onclick = async () => {
    await api.deleteTransaction(t.id);
    await refresh();
  };
  return el("li", {}, `${t.date} · ${cat} · ${t.note} · `, el("strong", {}, formatShekels(t.amount_agorot)), " ", del);
}

function renderForm(): HTMLFormElement {
  const date = el("input", { type: "date", required: true, valueAsDate: new Date() });
  const amount = el("input", { type: "text", inputMode: "decimal", placeholder: "Amount (e.g. 12.50)", required: true });
  const category = el("select", {}, el("option", { value: "", textContent: "No category" }));
  for (const c of categories) category.append(el("option", { value: String(c.id), textContent: c.name }));
  const note = el("input", { type: "text", placeholder: "Note" });
  const form = el("form", {}, date, amount, category, note, el("button", { type: "submit", textContent: "Add" }));
  form.onsubmit = async (e) => {
    e.preventDefault();
    await api.addTransaction({
      date: date.value,
      amount_agorot: parseAgorot(amount.value),
      category_id: category.value ? Number(category.value) : null,
      payment_method_id: null,
      note: note.value,
    });
    await refresh();
  };
  return form;
}

async function refresh() {
  categories = await api.categories();
  const txs = await api.transactions();
  app.replaceChildren(el("h1", {}, "Budget Tracker"), renderForm(), el("ul", {}, ...txs.map(renderRow)));
}

refresh().catch((e) => app.replaceChildren(el("p", {}, `Failed to load: ${e}`)));

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});
