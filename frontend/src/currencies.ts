import { api, CURRENCIES, currencyInfo, type Currency } from "./api";
import { el } from "./ui";

/**
 * The currencies added in settings (the shekel is always available and not part of the list).
 * Their order is the order of the currency picker. The server keeps the setting so it follows the
 * user across devices; a copy in localStorage lets the pickers work offline.
 */
const KEY = "budget.currencies";
/** Keeps a Latin symbol like C$ in reading order inside Hebrew text. */
export const LRM = "\u200E";
const DEFAULT: Currency[] = ["USD", "EUR", "JPY"];

let enabled: Currency[] = readCache();

function readCache(): Currency[] {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (Array.isArray(v)) return v.filter((c): c is string => typeof c === "string");
  } catch {
    /* storage unavailable or damaged */
  }
  return [...DEFAULT];
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
    remember((await api.currencySettings()).enabled);
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
  const moreBox = el("div", { className: "rows" });
  const search = el("input", { type: "search", placeholder: "חיפוש מטבע…", autocomplete: "off", spellcheck: false, className: "cursearch" });

  /** Save a new list; on failure show the error and go back to what the server has. */
  const save = async (list: Currency[]) => {
    const before = enabled;
    remember(list);
    paint();
    msg.textContent = "";
    try {
      remember((await api.saveCurrencySettings(list)).enabled);
    } catch (e) {
      remember(before);
      msg.textContent = `השמירה נכשלה: ${e instanceof Error ? e.message : e}`;
    }
    paint();
  };

  const icon = (text: string, title: string, on: () => void, disabled = false) => {
    const b = el("button", { type: "button", className: "btn iconbtn", textContent: text, title, disabled });
    b.setAttribute("aria-label", title);
    b.onclick = on;
    return b;
  };
  const addBtn = (c: { code: Currency; name: string }) => {
    const b = el("button", { type: "button", className: "btn", textContent: "הוספה", title: `הוספת ${c.name}` });
    b.setAttribute("aria-label", `הוספת ${c.name}`);
    b.style.whiteSpace = "nowrap";
    b.onclick = () => void save([...enabled, c.code]);
    return b;
  };
  const label = (c: Currency) => {
    const i = currencyInfo(c);
    return el("div", { className: "grow" }, el("div", { className: "name", textContent: `${LRM}${i.symbol}${LRM} ${i.name}` }), el("div", { className: "muted", textContent: i.code }));
  };
  const move = (i: number, d: number) => {
    const list = [...enabled];
    [list[i], list[i + d]] = [list[i + d], list[i]];
    void save(list);
  };

  function paint() {
    const ils = currencyInfo("ILS");
    addedBox.replaceChildren(
      el("div", { className: "row fixed" }, el("div", { className: "grow" }, el("div", { className: "name", textContent: `${ils.symbol} ${ils.name}` }), el("div", { className: "muted", textContent: "ILS · ברירת מחדל, תמיד זמין" }))),
      ...enabled.map((c, i) =>
        el(
          "div",
          { className: "row" },
          label(c),
          icon("▲", "הזזה למעלה", () => move(i, -1), i === 0),
          icon("▼", "הזזה למטה", () => move(i, 1), i === enabled.length - 1),
          icon("✕", "הסרה", () => void save(enabled.filter((x) => x !== c))),
        ),
      ),
    );
    const q = search.value.trim().toLowerCase();
    const rest = CURRENCIES.filter((c) => c.code !== "ILS" && !enabled.includes(c.code)).filter((c) => !q || `${c.symbol} ${c.name} ${c.code}`.toLowerCase().includes(q));
    moreBox.replaceChildren(
      ...(rest.length
        ? rest.map((c) => el("div", { className: "row" }, label(c.code), addBtn(c)))
        : [el("div", { className: "empty", textContent: q ? "אין תוצאות" : "כל המטבעות נוספו" })]),
    );
  }
  search.oninput = paint;
  paint();
  // Another device may have changed the list: show the server's copy.
  void loadEnabledCurrencies().then(paint);

  return el(
    "div",
    {},
    el("div", { className: "actions", style: "justify-content:flex-start" }, back),
    el("h1", { textContent: "מטבעות" }),
    el(
      "div",
      { className: "card" },
      el("h2", { textContent: "מטבעות פעילים" }),
      el("p", { className: "muted", textContent: "רק המטבעות כאן מוצגים בהוספה ועריכה של תנועות, לפי הסדר הזה. תנועות שכבר נשמרו במטבע שהוסר ממשיכות להופיע כרגיל." }),
      addedBox,
      msg,
    ),
    el("div", { className: "card" }, el("h2", { textContent: "הוספת מטבע" }), search, moreBox),
  );
}
