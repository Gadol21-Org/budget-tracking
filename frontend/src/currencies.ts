import { api, currencyInfo, type Currency } from "./api";
import { el } from "./ui";

/**
 * The currencies added in settings (the shekel is always available and not part of the list).
 * Their order is the order of the currency picker. The server keeps the setting so it follows the
 * user across devices; a copy in localStorage lets the pickers work offline.
 */
const KEY = "budget.currencies";
/** Keeps a Latin symbol like C$ in reading order inside Hebrew text. */
export const LRM = "\u200E";
const ALL_KEY = "budget.currencies.all";
const DEFAULT: Currency[] = ["USD", "EUR", "JPY"];

let enabled: Currency[] = readCache();
/** Every currency the server can price (for the add list). */
let supported: Currency[] = readCache(ALL_KEY, []);

function readCache(key = KEY, fallback: Currency[] = DEFAULT): Currency[] {
  try {
    const v = JSON.parse(localStorage.getItem(key) ?? "null");
    if (Array.isArray(v)) return v.filter((c): c is string => typeof c === "string");
  } catch {
    /* storage unavailable or damaged */
  }
  return [...fallback];
}

function remember(list: Currency[]) {
  enabled = list;
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    /* private mode, quota: the pickers just use the server's copy next time */
  }
}

/** The added currencies, in picker order. */
export const enabledCurrencies = (): Currency[] => enabled;

/** Fetch the saved list; offline, the last known one stays. */
export async function loadEnabledCurrencies(): Promise<void> {
  try {
    const r = await api.currencySettings();
    remember(r.enabled);
    supported = r.supported;
    try {
      localStorage.setItem(ALL_KEY, JSON.stringify(supported));
    } catch {
      /* fine */
    }
  } catch {
    /* keep what we had */
  }
}

/** What the add/edit pickers offer: the shekel first, then the added currencies in order. A line already in a removed currency keeps its own so it can still be opened and saved. */
export function pickerCurrencies(keep?: Currency): Currency[] {
  const list = ["ILS", ...enabled];
  if (keep && !list.includes(keep)) list.push(keep);
  return list;
}

export interface CurrenciesCtx {
  /** Back to the settings screen. */
  back: () => void;
}

/** The settings page: add currencies from the full list and put the added ones in order. */
export function currenciesView(ctx: CurrenciesCtx): HTMLElement {
  const back = el("button", { className: "btn", textContent: "→ הגדרות", title: "חזרה להגדרות" });
  back.onclick = ctx.back;
  const msg = el("div", { className: "note error" });
  const addedBox = el("div", { className: "rows" });
  const moreBox = el("div", { className: "rows curlist" });
  const search = el("input", { type: "search", placeholder: "חיפוש מטבע לפי שם או קוד…", autocomplete: "off", spellcheck: false, className: "cursearch" });

  /** Save a new list; on failure show the error and go back to what the server has. */
  const save = async (list: Currency[], focus?: number) => {
    const before = enabled;
    remember(list);
    paint(focus);
    msg.textContent = "";
    try {
      remember((await api.saveCurrencySettings(list)).enabled);
    } catch (e) {
      remember(before);
      msg.textContent = `השמירה נכשלה: ${e instanceof Error ? e.message : e}`;
    }
    paint(focus);
  };

  const icon = (text: string, title: string, on: () => void) => {
    const b = el("button", { type: "button", className: "btn iconbtn", textContent: text, title });
    b.setAttribute("aria-label", title);
    b.onclick = on;
    return b;
  };
  const addBtn = (c: Currency) => {
    const b = el("button", { type: "button", className: "btn", textContent: "הוספה", title: `הוספת ${currencyInfo(c).name}` });
    b.setAttribute("aria-label", `הוספת ${currencyInfo(c).name}`);
    b.style.whiteSpace = "nowrap";
    b.onclick = () => void save([...enabled, c]);
    return b;
  };
  const label = (c: Currency) => {
    const i = currencyInfo(c);
    return el("div", { className: "grow" }, el("div", { className: "name", textContent: i.symbol === i.code ? i.name : `${LRM}${i.symbol}${LRM} ${i.name}` }), el("div", { className: "muted", textContent: i.code }));
  };
  const reorder = (from: number, to: number) => {
    const list = [...enabled];
    list.splice(to, 0, ...list.splice(from, 1));
    return list;
  };

  /** A handle that drags its row (mouse, touch and pen alike: pointer events, the handle opts out of scrolling); arrow keys on the focused handle move it too. */
  function handle(i: number, row: HTMLElement): HTMLElement {
    const h = el("button", { type: "button", className: "btn iconbtn draghandle", textContent: "⠿", title: "גרירה לשינוי סדר" });
    h.setAttribute("aria-label", `שינוי סדר: ${currencyInfo(enabled[i]).name}. חצים למעלה ולמטה מזיזים`);
    h.onkeydown = (e) => {
      const to = e.key === "ArrowUp" ? i - 1 : e.key === "ArrowDown" ? i + 1 : -1;
      if (to < 0 || to >= enabled.length) return;
      e.preventDefault();
      void save(reorder(i, to), to);
    };
    h.onpointerdown = (e) => {
      if (e.button > 0) return;
      e.preventDefault();
      h.setPointerCapture(e.pointerId);
      const rows = [...addedBox.querySelectorAll<HTMLElement>(".row[data-i]")];
      const rects = rows.map((r) => r.getBoundingClientRect());
      const startY = e.clientY;
      const step = rects.length > 1 ? rects[1].top - rects[0].top : rects[i].height;
      let to = i;
      row.classList.add("dragging");
      const move = (ev: PointerEvent) => {
        const dy = Math.max(rects[0].top - rects[i].top, Math.min(rects[rects.length - 1].top - rects[i].top, ev.clientY - startY));
        row.style.transform = `translateY(${dy}px)`;
        // Rows are equally tall, so the new place is the number of whole rows the handle has travelled (the clamp above lets it reach either end).
        to = Math.max(0, Math.min(rows.length - 1, i + Math.round(dy / step)));
        rows.forEach((r, k) => {
          if (k !== i) r.style.transform = i < k && k <= to ? `translateY(${-step}px)` : to <= k && k < i ? `translateY(${step}px)` : "";
        });
      };
      const end = (ev: PointerEvent) => {
        h.removeEventListener("pointermove", move);
        h.removeEventListener("pointerup", end);
        h.removeEventListener("pointercancel", end);
        rows.forEach((r) => ((r.style.transform = ""), r.classList.remove("dragging")));
        if (ev.type === "pointerup" && to !== i) void save(reorder(i, to));
      };
      h.addEventListener("pointermove", move);
      h.addEventListener("pointerup", end);
      h.addEventListener("pointercancel", end);
    };
    return h;
  }

  function paint(focus?: number) {
    const ils = currencyInfo("ILS");
    addedBox.replaceChildren(
      el("div", { className: "row fixed" }, el("div", { className: "grow" }, el("div", { className: "name", textContent: `${ils.symbol} ${ils.name}` }), el("div", { className: "muted", textContent: "ILS · ברירת מחדל, תמיד זמין" }))),
      ...(enabled.length ? [] : [el("div", { className: "empty", textContent: "לא נוספו מטבעות נוספים" })]),
      ...enabled.map((c, i) => {
        const row = el("div", { className: "row", draggable: false });
        row.dataset.i = String(i);
        row.append(handle(i, row), label(c), icon("✕", "הסרה", () => void save(enabled.filter((x) => x !== c))));
        return row;
      }),
    );
    if (focus != null) addedBox.querySelector<HTMLElement>(`.row[data-i="${focus}"] .draghandle`)?.focus();
    const q = search.value.trim().toLowerCase();
    const all = supported.length ? supported : enabled;
    const rest = all.filter((c) => c !== "ILS" && !enabled.includes(c)).filter((c) => !q || `${currencyInfo(c).symbol} ${currencyInfo(c).name} ${c}`.toLowerCase().includes(q));
    moreBox.replaceChildren(...(rest.length ? rest.map((c) => el("div", { className: "row" }, label(c), addBtn(c))) : [el("div", { className: "empty", textContent: q ? "אין תוצאות" : "כל המטבעות נוספו" })]));
    count.textContent = `${rest.length} מטבעות זמינים`;
  }
  const count = el("div", { className: "muted" });
  search.oninput = () => paint();
  paint();
  // Another device may have changed the list: show the server's copy.
  void loadEnabledCurrencies().then(() => paint());

  return el(
    "div",
    {},
    el("div", { className: "actions", style: "justify-content:flex-start" }, back),
    el("h1", { textContent: "מטבעות" }),
    el(
      "div",
      { className: "card" },
      el("h2", { textContent: "מטבעות פעילים" }),
      el("p", { className: "muted", textContent: "רק המטבעות כאן מוצגים בהוספה ועריכה של תנועות, לפי הסדר הזה. גוררים בידית ⠿ כדי לשנות את הסדר. תנועות שכבר נשמרו במטבע שהוסר ממשיכות להופיע כרגיל." }),
      addedBox,
      msg,
    ),
    el("div", { className: "card" }, el("h2", { textContent: "הוספת מטבע" }), search, count, moreBox),
  );
}
