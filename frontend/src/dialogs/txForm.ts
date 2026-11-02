/** The pure rules behind the add/edit transaction dialog: what it starts with and what it sends. */
import { decimals, parseAgorot, type Currency, type Fx, type NewRecurring, type RecurringSegment, type Transaction, type TxInput } from "../api";
import { addMonths } from "../period";

/** A foreign-currency repeating charge's original amount, when the occurrence being edited has one. */
export function ruleFxOf(existing: Transaction | undefined, segment: RecurringSegment | undefined): { currency: Currency; orig_minor: number } | undefined {
  return existing?.upcoming && segment?.currency && segment.orig_minor != null ? { currency: segment.currency, orig_minor: segment.orig_minor } : undefined;
}

/** What the amount field starts with: the original foreign amount if there is one, else the shekels. */
export function startAmountText(existing: Transaction | undefined, startFx: Pick<Fx, "currency" | "orig_minor"> | undefined, ruleFx: { currency: Currency; orig_minor: number } | undefined): string {
  const foreign = startFx ?? ruleFx;
  if (foreign) return String(Math.abs(foreign.orig_minor) / 10 ** decimals(foreign.currency));
  return existing ? String(Math.abs(existing.amount_agorot) / 100) : "";
}

export interface BodyInput {
  date: string;
  amountText: string;
  isIncome: boolean;
  /** Read after the amount is checked, so a missing amount is reported before a missing category. */
  categoryId: () => number;
  paymentMethodId: number | null;
  note: string;
  currency: Currency;
  /** The typed amount in the currency's minor unit (0 when it cannot be read). */
  origMinor: number;
  /** The shekel amount, as typed, and whether it was typed (what the card really charged). */
  ilsText: string;
  ilsTouched: boolean;
  repeating: boolean;
  /** "Update rate" was pressed, so the saved rate is not kept. */
  forceLookup: boolean;
  /** A saved foreign line whose currency, amount and day are as they were. */
  fxUnchanged: boolean;
  /** The shekel amount the line already has, kept when its foreign amount is unchanged. */
  savedAgorot: number | undefined;
}

/** The line as the server takes it, for a single transaction and for one month of a series. */
export function buildTxBody(i: BodyInput): TxInput {
  const abs = Math.abs(parseAgorot(i.amountText));
  if (abs === 0) throw new Error("יש להזין סכום");
  const sign = i.isIncome ? 1 : -1;
  const body: TxInput = { date: i.date, amount_agorot: sign * abs, category_id: i.categoryId(), payment_method_id: i.paymentMethodId, note: i.note };
  if (i.currency !== "ILS") {
    // Foreign: send the original amount; the server converts it at the rate for the day unless
    // the shekel amount was typed in, or the saved line is unchanged (then its rate is kept).
    delete body.amount_agorot;
    body.currency = i.currency;
    body.orig_minor = sign * i.origMinor;
    if (i.ilsTouched && !i.repeating) {
      const a = Math.abs(parseAgorot(i.ilsText));
      if (a === 0) throw new Error("יש להזין סכום בשקלים");
      body.amount_agorot = sign * a;
    } else if (!i.forceLookup && i.fxUnchanged) body.amount_agorot = i.savedAgorot;
  }
  return body;
}

export interface RuleInput {
  body: TxInput;
  categoryId: number;
  /** The date field (YYYY-MM-DD). */
  date: string;
  /** "forever" or "months", and the typed number of months. */
  duration: string;
  months: number;
  /** The month of the occurrence being edited, when the line is one of a series. */
  seriesMonth: string | undefined;
  /** The day of the month of the series' current rule. */
  segmentDay: number | undefined;
}

/** The repeating rule to save from a dialog that has "repeat monthly" on. */
export function buildRecurringRule(i: RuleInput): NewRecurring {
  const { body } = i;
  if (i.duration === "months" && !(Number.isInteger(i.months) && i.months >= 1)) throw new Error("מספר החודשים אינו תקין");
  // A series change starts from the occurrence's own month, whatever date was typed.
  const start = i.seriesMonth !== undefined ? i.seriesMonth : i.date.slice(0, 7);
  return {
    start_month: start,
    end_month: i.duration === "months" ? addMonths(start, i.months - 1) : null,
    day: i.seriesMonth !== undefined && i.date.slice(0, 7) !== i.seriesMonth ? (i.segmentDay ?? 1) : Number(i.date.slice(8)),
    // A foreign rule keeps its original amount; the server estimates the shekels and prices each month at its day's rate.
    amount_agorot: body.amount_agorot ?? ((body.orig_minor ?? 0) < 0 ? -1 : 1),
    currency: body.currency ?? null,
    orig_minor: body.orig_minor ?? null,
    category_id: i.categoryId,
    payment_method_id: body.payment_method_id,
    note: body.note,
  };
}
