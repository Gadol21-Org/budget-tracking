import { type InsightsNav, type InsightsState } from "./insights/model";
import { monthOf, type Period } from "./period";
import { emptyProducts, type ProductsData, type BusinessView } from "./business";
import { type Category, type Fx, type LogEntry, type LogUser, type RecurringSegment, type Transaction } from "./api";

export type View = "month" | "list" | "insights" | "settings" | "log" | "categories" | "actions" | "businessOverview" | "products" | "currencies" | "people" | "dataTransfer";

export const root = document.getElementById("app")!;

export const state = {
  view: "month" as View,
  period: monthOf(new Date().toISOString().slice(0, 10)) as Period,
  /** Stored transactions plus the generated occurrences of repeating ones. */
  txs: [] as Transaction[],
  /** Stored transactions are loaded from this day (YYYY-MM-DD) on; "" means all of them; undefined before the first load. */
  loadedFrom: undefined as string | undefined,
  /** Older months are being fetched because the screen shown reaches before the loaded ones. */
  loadingOlder: false,
  recurring: [] as RecurringSegment[],
  cats: [] as Category[],
  /** Which side of the summary's category breakdown is shown. */
  kind: "expense" as "expense" | "income",
  /** Which side of the category manager in settings is shown (true = income). */
  catKind: false,
  /** Set when a summary row was opened: the list shows only that category (and its sub-categories). */
  filter: undefined as { id: number | null } | undefined,
  /** The insights tab: open dashboard, drill-down position and what the period is compared with. */
  insights: { dash: "overview", cmp: { kind: "prev" } } as InsightsState,
  /** Change log entries loaded so far (newest first) and whether older ones remain. */
  log: [] as LogEntry[],
  logMore: false,
  /** The period the change history shows. */
  logPeriod: monthOf(new Date().toISOString().slice(0, 10)) as Period,
  /** Emails seen on recorded changes, with display names. */
  logUsers: [] as LogUser[],
  /** Changes are written to Paamonim (PAAMONIM_MIRROR=1). */
  mirror: false,
  /** sync_state of each transaction at the last load, to notice lines that just got saved. */
  syncSeen: new Map<number, string>(),
  /** Lines just saved in Paamonim, shown with ✅ until this time (ms). */
  justSynced: new Map<number, number>(),
  /** The products, and which transactions and categories count in them. */
  products: emptyProducts() as ProductsData,
  /** The business dashboards: open tab and which product they show. */
  businessView: { tab: "overview", scope: "all" } as BusinessView,
  /** The transaction list: all, only product lines or only the rest. */
  productFilter: "all" as "all" | "product" | "none",
  /** Choosing several lines to tag them as a product's. */
  /** Transactions entered in USD or JPY, by transaction id (queued lines by -queue id): original amount and rate. */
  fx: new Map<number, Fx>(),
  /** The server could not be reached: the last saved data is shown, and new lines wait in the queue. */
  offline: false,
};

/** Browser history: every screen is a history entry, so the phone's back button/gesture goes to the previous screen (and closes a dialog first) instead of leaving the app. */
export type Screen = { view: View; filter?: { id: number | null }; ins?: InsightsNav; biz?: BusinessView; dlg?: boolean; n: number };
