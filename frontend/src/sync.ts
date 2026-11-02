import { dequeue, isNetworkError, loadSnapshot, queuedList, requestPersistence, saveSnapshot, type Queued, type Snapshot } from "./offline";
import { toBizData } from "./business";
import { loadEnabledCurrencies } from "./currencies";
import { api, type Fx, type LogEntry, type Transaction } from "./api";
import { state } from "./state";
import { LOG_PAGE } from "./views/log";
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

export type ServerData = Pick<Snapshot, "cats" | "txs" | "recurring" | "occurrences" | "mirror" | "fx">;

export async function fetchAll(): Promise<ServerData & { log?: LogEntry[] }> {
  const [cats, txs, recurring, occurrences, log, mirror, fx] = await Promise.all([
    api.categories(),
    api.transactions(),
    api.recurring(),
    api.occurrences(),
    state.view === "log" ? api.log(undefined, Math.max(LOG_PAGE, state.log.length)) : undefined,
    api.mirrorStatus().catch(() => ({ enabled: false })),
    api.foreign().catch((e) => (isNetworkError(e) ? Promise.reject(e) : undefined)),
  ]);
  return { cats, txs, recurring, occurrences, mirror: mirror.enabled, log, fx };
}

export async function load() {
  await flushQueue();
  let data: ServerData & { log?: LogEntry[] };
  try {
    data = await fetchAll();
    state.offline = false;
    void saveSnapshot({ ...data, savedAt: Date.now() });
    requestPersistence();
  } catch (e) {
    // Offline: show what was last seen (if anything) instead of an error.
    const snap = isNetworkError(e) ? await loadSnapshot() : undefined;
    if (!snap) throw e;
    data = snap;
    state.offline = true;
  }
  const { cats, recurring, occurrences, log } = data;
  // Businesses are optional: a failure (or being offline) leaves the last ones in place.
  try {
    state.biz = toBizData(await api.businesses());
  } catch {
    /* keep what we had */
  }
  await loadEnabledCurrencies();
  // Foreign-currency details (fetched with the rest; the server may just have re-priced stand-in rates).
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
