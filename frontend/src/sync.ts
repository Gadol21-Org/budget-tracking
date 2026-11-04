import { dequeue, isNetworkError, loadSnapshot, queuedList, requestPersistence, saveSnapshot, type Queued, type Snapshot } from "./offline";
import { toBizData } from "./business";
import { loadEnabledCurrencies } from "./currencies";
import { api, type Fx, type LogEntry, type Transaction } from "./api";
import { state } from "./state";
import { LOG_PAGE } from "./views/log";
import { addMonths, bounds, windowStart } from "./period";
import { render } from "./render";

/** States of a line whose change is on its way to Paamonim. */
export const IN_FLIGHT = new Set(["pending", "modified", "verifying"]);

export const SYNCED_SHOWN_MS = 2500;

/** Send lines saved while offline, oldest first. Stops at the first one the server can't be reached for. */
export async function flushQueue() {
  for (const q of await queuedList()) {
    try {
      const id = await api.addTransaction(q.tx);
      if (q.business != null) await api.tagTransactions([id], q.business).catch(() => {});
    } catch (e) {
      if (isNetworkError(e)) return;
      // The server refused this line (it would be refused again): drop it rather than block the rest.
    }
    await dequeue(q.qid);
  }
}

export type ServerData = Pick<Snapshot, "cats" | "txs" | "recurring" | "occurrences" | "mirror" | "fx" | "biz">;

const todayIso = () => new Date().toISOString().slice(0, 10);

export async function fetchAll(from?: string): Promise<ServerData & { log?: LogEntry[] }> {
  const [cats, txs, recurring, occurrences, log, mirror, fx, biz] = await Promise.all([
    api.categories(),
    api.transactions(from || undefined),
    api.recurring(),
    api.occurrences(),
    state.view === "log" ? api.log(undefined, Math.max(LOG_PAGE, state.log.length)) : undefined,
    api.mirrorStatus().catch(() => ({ enabled: false })),
    api.foreign().catch((e) => (isNetworkError(e) ? Promise.reject(e) : undefined)),
    // Businesses are optional: a failure leaves the last ones in place.
    api.businesses().catch(() => undefined),
  ]);
  return { cats, txs, recurring, occurrences, mirror: mirror.enabled, log, fx, biz };
}

export async function load() {
  await flushQueue();
  let data: ServerData & { log?: LogEntry[] };
  // The window opens on the last 3 months; older months already fetched for the screen shown stay loaded.
  const winFrom = windowStart(todayIso());
  const from = state.loadedFrom !== undefined && state.loadedFrom < winFrom ? state.loadedFrom : winFrom;
  let loadedFrom = from;
  try {
    data = await fetchAll(from);
    state.offline = false;
    historyFailed = false;
    // The offline copy is only the window, replaced whole each time, so it can never drift from the server.
    void saveSnapshot({ ...data, txs: data.txs.filter((t) => t.date >= winFrom), from: winFrom, savedAt: Date.now() });
    requestPersistence();
  } catch (e) {
    // Offline: show what was last seen (if anything) instead of an error.
    const snap = isNetworkError(e) ? await loadSnapshot() : undefined;
    if (!snap) throw e;
    data = snap;
    loadedFrom = snap.from ?? "";
    state.offline = true;
  }
  state.loadedFrom = loadedFrom;
  await loadEnabledCurrencies();
  await apply(data);
}

/** First paint: show the offline copy at once, before anything is fetched. False if there is none yet. */
export async function showCached(): Promise<boolean> {
  const snap = await loadSnapshot();
  if (!snap) return false;
  state.loadedFrom = snap.from ?? "";
  await apply(snap);
  return true;
}

async function apply(data: ServerData & { log?: LogEntry[] }) {
  const { cats, recurring, occurrences, log } = data;
  // Foreign-currency details (fetched with the rest; the server may just have re-priced stand-in rates).
  // Businesses come with the offline copy too, so the business card shows at once; a missing one keeps what we had.
  if (data.biz) state.biz = toBizData(data.biz);
  if (data.fx) state.fx = new Map(data.fx.map((f) => [f.transaction_id, f]));
  const queued = await queuedList();
  // A queued foreign line has no shekel amount yet: the server converts it when it is sent.
  const queuedFx = new Map<number, Fx>();
  const txs: Transaction[] = [
    ...data.txs,
    ...queued.map((q: Queued) => {
      if (q.tx.currency && q.tx.orig_minor != null) queuedFx.set(-q.qid, { transaction_id: -q.qid, currency: q.tx.currency, orig_minor: q.tx.orig_minor, rate: 0, rate_date: q.tx.date, estimated: true });
      return { id: -q.qid, ...q.tx, amount_agorot: q.tx.amount_agorot ?? 0, sync_state: "queued" };
    }),
  ];
  for (const [id, f] of queuedFx) state.fx.set(id, f);
  state.mirror = data.mirror;
  trackSync(txs);
  if (log) {
    state.log = log;
    state.logMore = log.length >= LOG_PAGE;
  }
  state.cats = cats;
  state.recurring = recurring;
  const generated: Transaction[] = occurrences.map((o) => ({
    id: 0,
    date: o.date,
    amount_agorot: o.amount_agorot,
    category_id: o.category_id,
    payment_method_id: o.payment_method_id,
    note: o.note,
    sync_state: "synced",
    recurring_series_id: o.series_id,
    recurring_segment_id: o.segment_id,
    upcoming_fx: o.currency && o.orig_minor != null ? { currency: o.currency, orig_minor: o.orig_minor } : undefined,
    upcoming: true,
  }));
  state.txs = [...txs, ...generated];
  render();
  void refreshSumit();
}

/** Look at the Sumit receipts' status (a read of our own server, no Sumit call) for the banner on the summary. */
async function refreshSumit() {
  try {
    const o = await api.sumit();
    const next = o.configured ? { held: o.held.length, unread: o.notices.filter((n) => !n.read).length } : { held: 0, unread: 0 };
    if (next.held === state.sumit.held && next.unread === state.sumit.unread) return;
    state.sumit = next;
    render();
  } catch {
    /* offline or not set up: no banner */
  }
}

/** Months fetched beyond what the screen needs when older months are loaded. */
export const HISTORY_PADDING_MONTHS = 3;

let historyFailed = false;
let extending: Promise<void> | undefined;

/** Make sure stored transactions are loaded back to `from` (YYYY-MM-DD), fetching the older months if not. */
export async function loadHistory(from: string): Promise<void> {
  while (extending) await extending;
  if (state.loadedFrom === undefined || from >= state.loadedFrom) return;
  extending = (async () => {
    state.loadingOlder = true;
    try {
      const older = await api.transactions(from || undefined);
      // Everything stored we hold is dated after `from`, so the fetched list replaces it.
      state.txs = [...older, ...state.txs.filter((t) => t.id <= 0 || t.upcoming)];
      state.loadedFrom = from;
    } catch {
      historyFailed = true; // offline or server down: show what we have, no retry loop
    } finally {
      state.loadingOlder = false;
      extending = undefined;
      render();
    }
  })();
  await extending;
}

/** The earliest day the screen shown needs: its period, and for the dashboards the months they compare with. */
export function neededFrom(): string {
  let from = bounds(state.period).from;
  const cmp = state.insights.cmp;
  if (state.view === "insights" || state.view === "business") {
    // Trends show 12 months; a year earlier is the "same period last year"; an average looks back n periods.
    const avgBack = state.view === "insights" && cmp.kind === "avg" ? cmp.n * (state.period.kind === "year" ? 12 : 1) : 0;
    from = `${addMonths(from.slice(0, 7), -12 - avgBack)}-01`;
    if (state.view === "insights" && cmp.kind === "custom") from = [from, bounds(cmp.period).from].sort()[0];
  } else if (state.view === "month") {
    // The summary's category breakdown compares with other periods, and an open category shows 12 months.
    const unit = state.period.kind === "year" ? 12 : 1;
    const back = cmp.kind === "avg" ? cmp.n * unit : unit;
    from = `${addMonths(from.slice(0, 7), -(back + (state.insights.cat !== undefined ? 12 : 0)))}-01`;
    if (cmp.kind === "custom") from = [from, bounds(cmp.period).from].sort()[0];
  }
  return from;
}

/** Called by every render: fetch older months in the background when the screen reaches before the loaded ones. */
export function ensureHistory() {
  const needed = neededFrom();
  if (state.loadedFrom === undefined || needed >= state.loadedFrom || extending || historyFailed) return;
  // Fetch 3 months more than needed, so paging back a month or two does not fetch again.
  const from = `${addMonths(needed.slice(0, 7), -HISTORY_PADDING_MONTHS)}-01`;
  state.loadingOlder = true;
  void loadHistory(from);
}

/** Note lines that went from in flight to saved (✅ for a moment), and keep checking while any
 * line is still on its way to Paamonim, so the marks update without a manual refresh. */
export let syncTimer: ReturnType<typeof setTimeout> | undefined;

export let syncPolls = 0;

export function trackSync(txs: Transaction[]) {
  const now = Date.now();
  for (const t of txs) {
    const before = state.syncSeen.get(t.id);
    if (before && IN_FLIGHT.has(before) && t.sync_state === "synced") state.justSynced.set(t.id, now + SYNCED_SHOWN_MS);
  }
  state.syncSeen = new Map(txs.map((t) => [t.id, t.sync_state]));
  clearTimeout(syncTimer);
  const waiting = state.mirror && txs.some((t) => IN_FLIGHT.has(t.sync_state));
  syncPolls = waiting ? syncPolls + 1 : 0;
  if (waiting) {
    // Quick at first, then slower if Paamonim takes a while (e.g. waiting for the next sweep).
    const poll = async () => {
      // A refresh is skipped while a dialog is open: try again shortly.
      if (!(await refresh())) syncTimer = setTimeout(poll, 3000);
    };
    syncTimer = setTimeout(poll, syncPolls < 40 ? 1500 : 15000);
  } else if ([...state.justSynced.values()].some((until) => until > now)) {
    // Take the ✅ away once its moment is over.
    const next = Math.min(...[...state.justSynced.values()].filter((u) => u > now));
    syncTimer = setTimeout(() => {
      for (const [id, until] of state.justSynced) if (until <= Date.now()) state.justSynced.delete(id);
      render();
    }, next - now + 50);
  }
}

/** Reload everything without disturbing an open dialog; a failure keeps the data already shown. */
export let refreshing: Promise<boolean> | undefined;

export function refresh(): Promise<boolean> {
  if (document.querySelector("dialog[open]")) return Promise.resolve(false);
  refreshing ??= load()
    .then(() => true)
    .catch(() => false)
    .finally(() => (refreshing = undefined));
  return refreshing;
}

// An installed PWA is usually resumed from the background, not reloaded, so fetch again whenever it
// becomes visible (also after a bfcache restore or regained connectivity). Throttled against bursts.
let lastLoad = Date.now();
/** Note that data was just fetched, so a resume right after does not fetch again. */
export function markLoaded() {
  lastLoad = Date.now();
}

export function refreshIfStale() {
  if (document.visibilityState !== "visible" || Date.now() - lastLoad < 5000) return;
  lastLoad = Date.now();
  void refresh();
}

document.addEventListener("visibilitychange", refreshIfStale);

window.addEventListener("pageshow", (e) => e.persisted && refreshIfStale());

window.addEventListener("online", () => void refresh());

window.addEventListener("offline", () => void refresh());
