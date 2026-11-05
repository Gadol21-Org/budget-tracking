import { api, type Currency } from "./api";

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

export function rememberCurrencies(list: Currency[]) {
  enabled = list;
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    /* private mode, quota: the pickers just use the server's copy next time */
  }
}

/** Every currency the server can price, for the add list (empty until it has been fetched once). */
export const supportedCurrencies = (): Currency[] => supported;

/** The added currencies, in picker order. */
export const enabledCurrencies = (): Currency[] => enabled;

/** Fetch the saved list; offline, the last known one stays. */
export async function loadEnabledCurrencies(): Promise<void> {
  try {
    const r = await api.currencySettings();
    rememberCurrencies(r.enabled);
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
