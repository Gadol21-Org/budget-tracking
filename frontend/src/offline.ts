import type { Category, Occurrence, RecurringSegment, Transaction } from "./api";

/** Offline support: the last data fetched from the server, and new transactions waiting to be sent.
 * Both live in IndexedDB (with persistent storage requested, so the browser does not evict them). */

export interface Snapshot {
  cats: Category[];
  txs: Transaction[];
  recurring: RecurringSegment[];
  occurrences: Occurrence[];
  mirror: boolean;
  savedAt: number;
}

export type NewTransaction = Omit<Transaction, "id" | "sync_state">;
export interface Queued {
  qid: number;
  tx: NewTransaction;
}

let dbp: Promise<IDBDatabase> | undefined;
function db(): Promise<IDBDatabase> {
  dbp ??= new Promise((resolve, reject) => {
    const open = indexedDB.open("budget", 1);
    open.onupgradeneeded = () => {
      open.result.createObjectStore("kv");
      open.result.createObjectStore("queue", { keyPath: "qid", autoIncrement: true });
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
  return dbp;
}

async function run<T>(store: string, mode: IDBTransactionMode, f: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const d = await db();
  return new Promise((resolve, reject) => {
    const req = f(d.transaction(store, mode).objectStore(store));
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/** Storage problems (private mode, quota) must never break the app: every helper swallows them. */
export const saveSnapshot = (s: Snapshot) => run("kv", "readwrite", (o) => o.put(s, "snapshot")).catch(() => {});
export const loadSnapshot = () => run<Snapshot | undefined>("kv", "readonly", (o) => o.get("snapshot")).catch(() => undefined);
export const queuedList = () => run<Queued[]>("queue", "readonly", (o) => o.getAll()).catch(() => [] as Queued[]);
export const enqueue = (tx: NewTransaction) => run("queue", "readwrite", (o) => o.add({ tx }));
export const dequeue = (qid: number) => run("queue", "readwrite", (o) => o.delete(qid)).catch(() => {});

/** Ask the browser not to evict our data under storage pressure (a no-op where unsupported). */
export function requestPersistence() {
  void navigator.storage?.persist?.().catch(() => {});
}

/** A fetch that never reached the server (offline, DNS, server down) rejects with a TypeError. */
export const isNetworkError = (e: unknown) => e instanceof TypeError;
