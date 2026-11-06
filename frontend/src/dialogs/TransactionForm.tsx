import { useEffect, useMemo, useRef, useState } from "react";
import { api, currencyInfo, currencySymbol, decimals, formatRate, parseMinor, type Currency, type Transaction } from "../api";
import { bizName, businessRequired, categoryBusiness } from "../business";
import { BusinessChips } from "../components/BusinessChips";
import { type CategoryChoice, CategoryPicker } from "../components/CategoryPicker";
import { Combo } from "../components/Combo";
import { DateField } from "../components/DateField";
import { Field } from "../components/Field";
import { MenuButton } from "../components/MenuButton";
import { LRM, pickerCurrencies } from "../currencies";
import { enqueue, isNetworkError, requestPersistence } from "../offline";
import { monthsBetween } from "../period";
import { catById, showPeriodFor } from "../selectors";
import { state } from "../state";
import { load } from "../sync";
import { fmtDate } from "../ui";
import { buildRecurringRule, buildTxBody, ruleFxOf, startAmountText } from "./txForm";

interface Quote {
  rate: number;
  date: string;
  estimated: boolean;
}

const lastKey = (cat: number) => `biz-last-${cat}`;
const today = () => new Date().toISOString().slice(0, 10);

/** The business last used in a category, if it still exists. */
function rememberedBiz(cat: number | null): number | null {
  try {
    const v = cat != null ? localStorage.getItem(lastKey(cat)) : null;
    return v && state.biz.businesses.some((b) => String(b.id) === v) ? Number(v) : null;
  } catch {
    return null; // storage unavailable
  }
}

/** The category a choice stands for, for the business rules: the sub-category, else the category. */
const choiceCategory = (c: CategoryChoice) => (c.sub ? Number(c.sub) : c.top ? Number(c.top) : null);

/** What an emailed suggestion starts a new transaction with. */
export interface Prefill {
  suggestionId: number;
  date: string;
  /** Signed, in the currency's minor unit. */
  amount_minor: number;
  currency: Currency;
  category_id: number | null;
  note: string;
}

/**
 * Add or edit a transaction. An `existing` generated occurrence of a repeating transaction edits
 * the series from that occurrence's month on; earlier months are left as they were.
 */
export function TransactionForm({ existing, startAsIncome, prefill, close }: { existing?: Transaction; startAsIncome: boolean; prefill?: Prefill; close: () => void }) {
  const seriesId = existing?.upcoming ? existing.recurring_series_id! : undefined;
  const segment = state.recurring.find((s) => s.id === existing?.recurring_segment_id);
  /** The month of the occurrence being edited; the date field may move the charge out of it. */
  const month = existing?.date.slice(0, 7) ?? "";
  // The shekel is the default currency, always; a line already entered in USD or JPY opens in that one.
  const startFx = existing && !existing.upcoming && existing.id > 0 ? state.fx.get(existing.id) : undefined;
  // A repeating charge in a foreign currency opens in that currency, with its original amount.
  const ruleFx = ruleFxOf(existing, segment);
  // The business the line counts in: its own tag, or the one its category gives (then there is nothing to choose).
  const ownTag = existing && existing.id > 0 ? (state.biz.tx.get(existing.id) ?? null) : null;

  const kindCats = (income: boolean) => state.cats.filter((c) => c.is_income === income);
  /** A category without sub-categories cannot hold transactions, so it is not offered (unless one already sits on it). */
  function offeredTops(income: boolean) {
    const cats = kindCats(income);
    return cats.filter((c) => c.parent_id == null && (cats.some((k) => k.parent_id === c.id) || c.id === existing?.category_id));
  }
  /** What stays picked after the kind changes: only what still fits. */
  function reconcile(income: boolean, c: CategoryChoice): CategoryChoice {
    if (!offeredTops(income).some((t) => String(t.id) === c.top)) return { top: "", sub: "" };
    const kids = kindCats(income).filter((k) => String(k.parent_id) === c.top);
    return { top: c.top, sub: kids.some((k) => String(k.id) === c.sub) ? c.sub : "" };
  }
  const startIncome = existing ? existing.amount_agorot > 0 : prefill ? prefill.amount_minor > 0 : startAsIncome;
  const [isIncome, setIsIncome] = useState(startIncome);
  const [currency, setCurrency] = useState<Currency>(startFx?.currency ?? ruleFx?.currency ?? prefill?.currency ?? "ILS");
  const [amount, setAmount] = useState(prefill ? String(Math.abs(prefill.amount_minor) / 10 ** decimals(prefill.currency)) : startAmountText(existing, startFx, ruleFx));
  const [date, setDate] = useState(existing?.date ?? prefill?.date ?? today());
  const [note, setNote] = useState(existing?.note ?? prefill?.note ?? "");
  const [choice, setChoice] = useState<CategoryChoice>(() => {
    const seed = existing?.category_id ?? prefill?.category_id ?? null;
    const own = seed != null ? catById(seed) : undefined;
    return reconcile(startIncome, own ? { top: String(own.parent_id ?? own.id), sub: own.parent_id != null ? String(own.id) : "" } : { top: "", sub: "" });
  });
  // Repeating monthly: for a new transaction it is opt-in; for an occurrence it is always on.
  const [repeat, setRepeat] = useState(seriesId !== undefined);
  const [duration, setDuration] = useState(segment?.end_month ? "months" : "forever");
  const [months, setMonths] = useState(segment?.end_month ? String(monthsBetween(existing!.date.slice(0, 7), segment.end_month) + 1) : "12");
  /** undefined = nothing chosen yet (only while a choice is required). */
  const [bizValue, setBizValue] = useState<number | null | undefined>(() => {
    if (ownTag != null || existing || prefill?.category_id == null) return ownTag;
    return businessRequired(state.biz, state.cats, prefill.category_id) ? undefined : rememberedBiz(prefill.category_id);
  });
  // Foreign currency: the shekel amount follows the rate for the transaction's day.
  const [quote, setQuote] = useState<Quote | null>(startFx ? { rate: startFx.rate, date: startFx.rate_date, estimated: startFx.estimated } : null);
  const [loadingQuote, setLoadingQuote] = useState(!startFx && (ruleFx !== undefined || currency !== "ILS"));
  const [ilsText, setIlsText] = useState(startFx ? String(Math.abs(existing!.amount_agorot) / 100) : "");
  /** The shekel amount was typed in (what the card really charged): it is used as it is. */
  const [ilsTouched, setIlsTouched] = useState(false);
  /** "Update rate" was pressed: look the rate up again instead of keeping the saved one. */
  const [forceLookup, setForceLookup] = useState(false);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const quoteSeq = useRef(0);
  const formRef = useRef<HTMLFormElement>(null);

  // Only the currencies added in settings (in their order); a line already in a removed one keeps its own.
  const currencyItems = useMemo(
    () =>
      pickerCurrencies(currency).map((code) => {
        const c = currencyInfo(code);
        return { value: c.code, label: c.symbol === c.code ? `${c.name} ${c.code}` : `${LRM}${c.symbol}${LRM} ${c.name} ${c.code}` };
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- fixed when the dialog opens
    [],
  );

  // ---- Categories -------------------------------------------------------------------------------------
  /** The category the transaction goes on: always a sub-category (top-level entries are not mirrored to Paamonim). */
  const categoryId = (): number => {
    if (!choice.top) throw new Error("יש לבחור קטגוריה");
    if (choice.sub) return Number(choice.sub);
    // Only a transaction already on a top-level category (from before sub-categories were required) keeps it.
    if (existing?.category_id === Number(choice.top)) return Number(choice.top);
    throw new Error("יש לבחור תת-קטגוריה");
  };
  const succeeds = (f: () => unknown) => {
    try {
      f();
      return true;
    } catch {
      return false;
    }
  };

  // ---- Business ---------------------------------------------------------------------------------------
  const cat = choiceCategory(choice);
  const given = cat != null ? categoryBusiness(state.biz, state.cats, cat) : undefined;
  const givenName = given != null && ownTag == null ? bizName(state.biz, given) : null;
  /** A new line in a category that needs a business must pick one (None is a choice); nothing is chosen for it. */
  const needsBiz = (c: number | null, repeating: boolean) => !existing && c != null && !repeating && businessRequired(state.biz, state.cats, c);
  const bizRequired = needsBiz(cat, repeat);
  const bizHidden = state.biz.businesses.length === 0 || repeat;

  const changeChoice = (next: CategoryChoice) => {
    setChoice(next);
    setErr("");
    const c = choiceCategory(next);
    // A new line starts with the business last used in the same category.
    if (needsBiz(c, repeat)) setBizValue(undefined);
    else if (!existing) setBizValue(rememberedBiz(c));
  };
  const changeRepeat = (on: boolean) => {
    setRepeat(on);
    if (needsBiz(cat, on) !== bizRequired) setBizValue(needsBiz(cat, on) ? undefined : null);
  };

  // ---- Amount and currency ----------------------------------------------------------------------------
  const foreign = currency !== "ILS";
  const origMinor = () => {
    try {
      return Math.abs(parseMinor(amount, currency));
    } catch {
      return 0;
    }
  };
  const minor = origMinor();
  // A repeating charge is always priced by the rate of each month's day, so no shekel amount is typed for it.
  const manual = ilsTouched && !repeat;
  const ilsShown = manual || !quote ? ilsText : minor ? ((minor / 10 ** decimals(currency)) * quote.rate).toFixed(2) : "";
  /** Look the rate up; only the newest lookup's answer is kept (null: none available, e.g. offline). */
  const lookUp = (cur: Currency, day: string) => {
    const seq = ++quoteSeq.current;
    const settle = (q: Quote | null) => {
      if (seq !== quoteSeq.current) return;
      setQuote(q);
      setLoadingQuote(false);
    };
    api.fxRate(cur, day).then(settle, () => settle(null));
  };
  const refreshQuote = (cur: Currency, day: string) => {
    if (cur === "ILS") return;
    setLoadingQuote(true);
    lookUp(cur, day);
  };
  // A new foreign line (or a repeating one) looks its rate up as the dialog opens; a saved line starts from its saved rate.
  useEffect(() => {
    if (!startFx && currency !== "ILS") lookUp(currency, date);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once, when the dialog opens
  }, []);
  const rateText = quote ? `שער: 1 ${currencySymbol(currency)} = ₪${formatRate(quote.rate)} · ${fmtDate(quote.date)}${quote.estimated ? " · זמני, יתעדכן אוטומטית כשהיום יסתיים" : ""}` : "";
  const fxNote = !foreign
    ? ""
    : loadingQuote
      ? "מביא שער…"
      : !quote
        ? "אין שער זמין כרגע: השער ייקבע כשהתנועה תישלח"
        : manual
          ? "סכום החיוב הוזן ידנית · השער יחושב ממנו"
          : repeat
            ? `כל חודש יחושב לפי השער של יום החיוב · ${rateText}`
            : rateText;
  /** A saved foreign line whose currency, amount and day are as they were keeps its saved rate. */
  const fxUnchanged = () => !!startFx && currency === startFx.currency && origMinor() === Math.abs(startFx.orig_minor) && date === existing!.date;

  /** A usable amount: a number that is not zero (to the agora, cent or yen). */
  const amountReady = succeeds(() => {
    if (parseMinor(amount, currency) === 0 || amount.trim() === "") throw new Error();
  });
  /** Saving stays greyed out until the category (and its sub-category, if it has any) and the amount are chosen. */
  const ready = succeeds(categoryId) && amountReady && bizValue !== undefined;

  // ---- Saving -----------------------------------------------------------------------------------------
  const buildBody = () =>
    buildTxBody({
      date,
      amountText: amount,
      isIncome,
      categoryId,
      paymentMethodId: existing?.payment_method_id ?? null,
      note,
      currency,
      origMinor: origMinor(),
      ilsText: ilsShown,
      ilsTouched,
      repeating: repeat,
      forceLookup,
      fxUnchanged: fxUnchanged(),
      savedAgorot: existing?.amount_agorot,
    });
  const attempt = (work: () => Promise<void>) => async () => {
    try {
      await work();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };
  // Only this month: becomes a transaction with these values; later months are unchanged.
  const updateOnce = attempt(async () => {
    await api.settleRecurringMonth(seriesId!, month, buildBody());
    showPeriodFor(date);
    close();
    await load();
  });
  // Stop the series: this month and later disappear, earlier months stay.
  const stopFromHere = attempt(async () => {
    if (!confirm("לבטל את ההוצאה החוזרת מחודש זה והלאה? חודשים קודמים לא ישתנו.")) return;
    await api.cancelRecurring(seriesId!, month);
    close();
    await load();
  });
  // Skip only this month; the series carries on around it.
  const skipOnce = attempt(async () => {
    if (!confirm("לדלג על החיוב של חודש זה בלבד?")) return;
    await api.skipRecurringMonth(seriesId!, month);
    close();
    await load();
  });
  const submit = async () => {
    try {
      const body = buildBody();
      const catId = categoryId();
      setBusy(true);
      if (repeat) {
        const rule = buildRecurringRule({ body, categoryId: catId, date, duration, months: Number(months), seriesMonth: seriesId !== undefined ? month : undefined, segmentDay: segment?.day });
        if (seriesId !== undefined) await api.changeRecurring(seriesId, rule);
        else await api.addRecurring(rule);
      } else if (existing) {
        await api.updateTransaction(existing.id, body);
        if ((bizValue ?? null) !== ownTag) await api.tagTransactions([existing.id], bizValue ?? null);
      } else {
        const chosen = bizValue ?? null;
        try {
          const id = await api.addTransaction(body);
          if (chosen != null) await api.tagTransactions([id], chosen);
          // The transaction exists; a failure to record the decision must not look like a failed save.
          if (prefill) await api.acceptSuggestion(prefill.suggestionId, id).catch(() => {});
        } catch (e3) {
          // No connection: keep the line on this device and send it when the connection is back.
          if (!isNetworkError(e3)) throw e3;
          await enqueue(body, chosen);
          requestPersistence();
        }
      }
      try {
        localStorage.setItem(lastKey(catId), bizValue == null ? "" : String(bizValue));
      } catch {
        /* storage unavailable */
      }
      showPeriodFor(date);
      close();
      await load();
    } catch (e2) {
      setErr(isNetworkError(e2) ? "אין חיבור לשרת: בלי חיבור אפשר רק להוסיף תנועה חדשה (לא חוזרת)" : e2 instanceof Error ? e2.message : String(e2));
      setBusy(false);
    }
  };

  const switchKind = (income: boolean) => {
    setIsIncome(income);
    setChoice(reconcile(income, choice));
  };

  return (
    <form
      ref={formRef}
      className="add"
      method="dialog"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div className="seg">
        <button type="button" aria-pressed={!isIncome} onClick={() => switchKind(false)}>
          הוצאה
        </button>
        <button type="button" aria-pressed={isIncome} onClick={() => switchKind(true)}>
          הכנסה
        </button>
      </div>
      <div className="amountrow">
        <Field label={`סכום (${currencySymbol(currency)})`}>
          <input type="text" inputMode="decimal" placeholder="0.00" required autoComplete="off" value={amount} onChange={(e) => setAmount(e.target.value)} />
        </Field>
        <Field label="מטבע" className="curfield">
          <Combo
            items={currencyItems}
            value={currency}
            placeholder="חיפוש מטבע…"
            onChange={(c) => {
              const next = c || "ILS";
              setCurrency(next);
              refreshQuote(next, date);
            }}
          />
        </Field>
      </div>
      {foreign && (
        <div className="fxbox">
          {!repeat && (
            <Field label="סכום בשקלים (₪)">
              <input
                type="text"
                inputMode="decimal"
                placeholder="0.00"
                autoComplete="off"
                value={ilsShown}
                onChange={(e) => (setIlsText(e.target.value), setIlsTouched(e.target.value.trim() !== ""))}
              />
            </Field>
          )}
          <div className="fxnote muted">{fxNote}</div>
          {startFx && (
            <button
              type="button"
              className="btn small"
              onClick={() => {
                setIlsTouched(false);
                setForceLookup(true);
                refreshQuote(currency, date);
              }}
            >
              עדכון שער
            </button>
          )}
        </div>
      )}
      <CategoryPicker cats={kindCats(isIncome)} tops={offeredTops(isIncome)} value={choice} onChange={changeChoice} />
      <Field label="תאריך">
        <DateField
          value={date}
          required
          onChange={(d) => {
            setDate(d);
            refreshQuote(currency, d);
          }}
        />
      </Field>
      <Field label="הערה">
        <input type="text" placeholder="אופציונלי" maxLength={35} value={note} onChange={(e) => setNote(e.target.value)} />
      </Field>
      <div style={{ display: bizHidden ? "none" : "" }}>
        <BusinessChips biz={state.biz} value={bizValue} onChange={setBizValue} required={bizRequired} givenName={givenName} collapsed />
      </div>
      {/* Only the checkbox (inside a finger-sized hit box) toggles; the row and its text are inert. */}
      {!existing && (
        <div className="repeat-row">
          <label className="repeat-hit">
            <input type="checkbox" checked={repeat} onChange={(e) => changeRepeat(e.target.checked)} />
          </label>
          <span>חוזר מדי חודש</span>
        </div>
      )}
      <div style={{ display: repeat ? "flex" : "none", flexDirection: "column", gap: 12 }}>
        <Field label="משך">
          <select value={duration} onChange={(e) => setDuration(e.target.value)}>
            <option value="forever">ללא הגבלה</option>
            <option value="months">למשך מספר חודשים</option>
          </select>
        </Field>
        <div className="field" style={{ display: duration === "months" ? "" : "none" }}>
          <span>מספר חודשים (כולל החודש הזה)</span>
          <input type="number" min="1" step="1" inputMode="numeric" value={months} onChange={(e) => setMonths(e.target.value)} />
        </div>
      </div>
      <div className="error">{err}</div>
      {seriesId !== undefined ? (
        <div className="actions">
          <button type="button" className="btn" onClick={close}>
            סגירה
          </button>
          <MenuButton
            label="ביטול חיוב"
            items={[
              ["חודש זה בלבד", skipOnce],
              ["מכאן והלאה", stopFromHere],
            ]}
          />
          <MenuButton
            label="עדכון"
            className="btn primary"
            disabled={!ready}
            items={[
              ["חודש זה בלבד", updateOnce],
              ["מכאן והלאה", () => formRef.current?.requestSubmit()],
            ]}
          />
        </div>
      ) : (
        <div className="actions">
          <button type="button" className="btn" onClick={close}>
            ביטול
          </button>
          <button type="submit" className="btn primary" disabled={!ready || busy}>
            {existing ? "עדכון" : "שמירה"}
          </button>
        </div>
      )}
    </form>
  );
}
