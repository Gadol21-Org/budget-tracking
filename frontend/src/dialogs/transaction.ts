import { dateField, el, fmtDate } from "../ui";
import { monthsBetween } from "../period";
import { buildRecurringRule, buildTxBody, ruleFxOf, startAmountText } from "./txForm";
import { enqueue, isNetworkError, requestPersistence } from "../offline";
import { businessChips, businessRequired, bizName, categoryBusiness } from "../business";
import { LRM, pickerCurrencies } from "../currencies";
import { api, currencyInfo, currencySymbol, decimals, formatRate, parseMinor, type Currency, type Transaction, type TxInput } from "../api";
import { root, state } from "../state";
import { combo, menuButton } from "../widgets";
import { catById, catPath, showPeriodFor } from "../selectors";
import { load } from "../sync";
import { trackDialog } from "../nav";

/**
 * Add or edit a transaction. An `existing` generated occurrence of a repeating transaction edits
 * the series from that occurrence's month on; earlier months are left as they were.
 */
export function openTransactionDialog(existing?: Transaction, startAsIncome = false) {
  const seriesId = existing?.upcoming ? existing.recurring_series_id! : undefined;
  const segment = state.recurring.find((s) => s.id === existing?.recurring_segment_id);
  /** The month of the occurrence being edited; the date field may move the charge out of it. */
  const month = existing?.date.slice(0, 7) ?? "";
  let isIncome = existing ? existing.amount_agorot > 0 : startAsIncome;
  const dlg = el("dialog", { className: "fullpage" });
  // The shekel is the default currency, always; a line already entered in USD or JPY opens in that one.
  const startFx = existing && !existing.upcoming && existing.id > 0 ? state.fx.get(existing.id) : undefined;
  // A repeating charge in a foreign currency opens in that currency, with its original amount.
  const ruleFx = ruleFxOf(existing, segment);
  let currency: Currency = startFx?.currency ?? ruleFx?.currency ?? "ILS";
  const amount = el("input", {
    type: "text",
    inputMode: "decimal",
    placeholder: "0.00",
    required: true,
    autocomplete: "off",
    value: startAmountText(existing, startFx, ruleFx),
  });
  const date = el("input", { type: "date", required: true, value: existing?.date ?? new Date().toISOString().slice(0, 10) });
  const topPick = combo("חיפוש קטגוריה…");
  const subPick = combo("תת-קטגוריה");
  const note = el("input", { type: "text", placeholder: "אופציונלי", maxLength: 35, value: existing?.note ?? "" });
  const err = el("div", { className: "error" });
  // The business the line counts in: its own tag, or the one its category gives (then there is nothing to choose).
  const ownTag = existing && existing.id > 0 ? (state.biz.tx.get(existing.id) ?? null) : null;
  const bizPick = businessChips(state.biz, ownTag, true);
  const bizRow = el("div", {}, bizPick.root);
  const lastKey = (cat: number) => `biz-last-${cat}`;
  const syncBiz = () => {
    let cat: number | null = null;
    try {
      cat = categoryId();
    } catch {
      const top = topPick.value;
      cat = top ? Number(top) : null;
    }
    const given = cat != null ? categoryBusiness(state.biz, state.cats, cat) : undefined;
    bizPick.lock(given != null && ownTag == null ? bizName(state.biz, given) : null);
    // A new line in a category that needs a business must pick one (None is a choice); nothing is chosen for it.
    bizPick.setRequired(!existing && cat != null && !repeat.checked && businessRequired(state.biz, state.cats, cat));
    bizRow.style.display = state.biz.businesses.length === 0 || repeat.checked ? "none" : "";
  };
  /** A new line starts with the business last used in the same category. */
  const rememberedBiz = () => {
    if (existing || bizPick.required) return;
    try {
      const sub = subPick.value || topPick.value;
      const v = sub ? localStorage.getItem(lastKey(Number(sub))) : null;
      bizPick.set(v && state.biz.businesses.some((b) => String(b.id) === v) ? Number(v) : null);
    } catch {
      /* storage unavailable */
    }
  };

  // Repeating monthly: for a new transaction it is opt-in; for an occurrence it is always on.
  const repeat = el("input", { type: "checkbox", checked: seriesId !== undefined, disabled: seriesId !== undefined });
  const forever = el("select", {}, el("option", { value: "forever", textContent: "ללא הגבלה" }), el("option", { value: "months", textContent: "למשך מספר חודשים" }));
  const months = el("input", { type: "number", min: "1", step: "1", inputMode: "numeric", value: "12" });
  if (segment?.end_month) {
    forever.value = "months";
    months.value = String(monthsBetween(existing!.date.slice(0, 7), segment.end_month) + 1);
  }
  const repeatBox = el("div", { style: "display:flex;flex-direction:column;gap:12px" }, el("div", { className: "field" }, el("span", { textContent: "משך" }), forever), el("div", { className: "field" }, el("span", { textContent: "מספר חודשים (כולל החודש הזה)" }), months));
  const showRepeat = () => {
    repeatBox.style.display = repeat.checked ? "flex" : "none";
    months.parentElement!.style.display = forever.value === "months" ? "" : "none";
  };
  repeat.onchange = forever.onchange = showRepeat;
  showRepeat();

  // Foreign currency (USD, JPY): the shekel amount follows the rate for the transaction's day.
  /** The currency in force: the same picker serves single and repeating charges. */
  const cur = (): Currency => currency;
  const amountLbl = el("span", { textContent: "סכום (₪)" });
  const curPick = combo("חיפוש מטבע…");
  // Only the currencies added in settings (in their order); a line already in a removed one keeps its own.
  curPick.set(pickerCurrencies(currency).map((code) => { const c = currencyInfo(code); return { value: c.code, label: c.symbol === c.code ? `${c.name} ${c.code}` : `${LRM}${c.symbol}${LRM} ${c.name} ${c.code}` }; }), currency);
  curPick.input.addEventListener("change", () => {
    currency = (curPick.value || "ILS") as Currency;
    syncFx();
    void refreshQuote();
    syncSave();
  });
  const curSeg = el("div", { className: "field curfield" }, el("span", { textContent: "מטבע" }), curPick.root);
  const ils = el("input", { type: "text", inputMode: "decimal", placeholder: "0.00", autocomplete: "off" });
  const fxNote = el("div", { className: "fxnote muted" });
  const refreshBtn = el("button", { type: "button", className: "btn small", textContent: "עדכון שער" });
  const ilsField = el("div", { className: "field" }, el("span", { textContent: "סכום בשקלים (₪)" }), ils);
  const fxBox = el("div", { className: "fxbox" }, ilsField, fxNote, startFx ? refreshBtn : null);
  /** The rate for the chosen day (null: unknown, e.g. offline: the server converts when the line is sent). */
  let quote: { rate: number; date: string; estimated: boolean } | null = startFx ? { rate: startFx.rate, date: startFx.rate_date, estimated: startFx.estimated } : null;
  let quoteSeq = 0;
  /** The shekel amount was typed in (what the card really charged): it is used as it is. */
  let ilsTouched = false;
  /** "Update rate" was pressed: look the rate up again instead of keeping the saved one. */
  let forceLookup = false;
  if (startFx) ils.value = String(Math.abs(existing!.amount_agorot) / 100);
  const origMinor = () => {
    try {
      return Math.abs(parseMinor(amount.value, cur()));
    } catch {
      return 0;
    }
  };
  /** A saved foreign line whose currency, amount and day are as they were keeps its saved rate. */
  const fxUnchanged = () => !!startFx && cur() === startFx.currency && origMinor() === Math.abs(startFx.orig_minor) && date.value === existing!.date;
  const recalc = () => {
    if (cur() === "ILS") return;
    const sym = currencySymbol(cur());
    if (!quote) {
      fxNote.textContent = "אין שער זמין כרגע: השער ייקבע כשהתנועה תישלח";
      return;
    }
    // A repeating charge is always priced by the rate of each month's day, so no shekel amount is typed for it.
    const manual = ilsTouched && !repeat.checked;
    if (!manual) {
      const minor = origMinor();
      ils.value = minor ? ((minor / 10 ** decimals(cur())) * quote.rate).toFixed(2) : "";
    }
    const rateText = `שער: 1 ${sym} = ₪${formatRate(quote.rate)} · ${fmtDate(quote.date)}${quote.estimated ? " · זמני, יתעדכן אוטומטית כשהיום יסתיים" : ""}`;
    fxNote.textContent = manual ? `סכום החיוב הוזן ידנית · השער יחושב ממנו` : repeat.checked ? `כל חודש יחושב לפי השער של יום החיוב · ${rateText}` : rateText;
  };
  const refreshQuote = async () => {
    if (cur() === "ILS") return;
    const seq = ++quoteSeq;
    fxNote.textContent = "מביא שער…";
    try {
      const q = await api.fxRate(cur(), date.value);
      if (seq === quoteSeq) quote = q;
    } catch {
      if (seq !== quoteSeq) return;
      quote = null;
    }
    if (seq === quoteSeq) recalc();
  };
  const syncFx = () => {
    const c = cur();
    if (curPick.value !== c) curPick.pick(c);
    fxBox.style.display = c === "ILS" ? "none" : "";
    ilsField.style.display = repeat.checked ? "none" : "";
    amountLbl.textContent = `סכום (${currencySymbol(c)})`;
    if (c !== "ILS") recalc();
  };
  amount.addEventListener("input", recalc);
  ils.addEventListener("input", () => {
    ilsTouched = ils.value.trim() !== "";
    recalc();
  });
  date.addEventListener("change", () => void refreshQuote());
  refreshBtn.onclick = () => {
    ilsTouched = false;
    forceLookup = true;
    void refreshQuote();
  };
  repeat.addEventListener("change", () => (syncFx(), syncSave()));
  const expBtn = el("button", { type: "button", textContent: "הוצאה" });
  const incBtn = el("button", { type: "button", textContent: "הכנסה" });
  const kindCats = () => state.cats.filter((c) => c.is_income === isIncome);
  const kidsOf = (topId: string) => kindCats().filter((k) => String(k.parent_id) === topId);
  /**
   * The sub-category list follows the category: just its sub-categories once one is picked, otherwise every
   * sub-category (named with its parent). It is only locked for a category that has none.
   */
  const fillSubs = (keepSub = "") => {
    const top = topPick.value;
    const kids = top ? kidsOf(top) : kindCats().filter((k) => k.parent_id != null);
    subPick.set(
      kids.map((k) => ({ value: String(k.id), label: top ? k.name : catPath(k.id) })),
      kids.some((k) => String(k.id) === keepSub) ? keepSub : "",
    );
    subPick.setDisabled(top !== "" && kids.length === 0, top && kids.length === 0 ? "אין תתי-קטגוריות" : "חיפוש תת-קטגוריה…");
  };
  topPick.input.addEventListener("change", () => ((err.textContent = ""), fillSubs()));
  // Picking a sub-category first fills in its category.
  subPick.input.addEventListener("change", () => {
    err.textContent = "";
    const sub = subPick.value;
    const parent = catById(Number(sub))?.parent_id;
    if (sub && parent != null && topPick.value !== String(parent)) {
      topPick.pick(String(parent));
      fillSubs(sub);
    }
  });
  /** The category the transaction goes on: always a sub-category (top-level entries are not mirrored to Paamonim). */
  const categoryId = (): number => {
    const top = topPick.value;
    if (!top) throw new Error("יש לבחור קטגוריה");
    if (subPick.value) return Number(subPick.value);
    // Only a transaction already on a top-level category (from before sub-categories were required) keeps it.
    if (existing?.category_id === Number(top)) return Number(top);
    throw new Error("יש לבחור תת-קטגוריה");
  };
  /** Saving stays greyed out until the category (and its sub-category, if it has any) is chosen. */
  const categoryReady = () => {
    try {
      categoryId();
      return true;
    } catch {
      return false;
    }
  };
  /** A usable amount: a number that is not zero (to the agora, cent or yen). */
  const amountReady = () => {
    try {
      return parseMinor(amount.value, cur()) !== 0 && amount.value.trim() !== "";
    } catch {
      return false;
    }
  };
  const syncSave = () => {
    const ready = categoryReady() && amountReady() && bizPick.chosen;
    save.disabled = !ready;
    for (const b of dlg.querySelectorAll<HTMLButtonElement>(".needs-category")) b.disabled = !ready;
  };
  amount.addEventListener("input", syncSave);
  topPick.input.addEventListener("change", syncSave);
  subPick.input.addEventListener("change", syncSave);
  let firstFill = true;
  const fillCats = (sync = true) => {
    expBtn.setAttribute("aria-pressed", String(!isIncome));
    incBtn.setAttribute("aria-pressed", String(isIncome));
    // First time: the transaction's own category; after switching kind it no longer applies unless it fits.
    let top = topPick.value;
    let sub = subPick.value;
    if (firstFill && existing?.category_id != null) {
      const own = catById(existing.category_id);
      top = String(own?.parent_id ?? existing.category_id);
      sub = own?.parent_id != null ? String(own.id) : "";
    }
    firstFill = false;
    // A category without sub-categories cannot hold transactions, so it is not offered (unless one already sits on it).
    const tops = kindCats().filter((c) => c.parent_id == null && (kidsOf(String(c.id)).length > 0 || c.id === existing?.category_id));
    if (!tops.some((c) => String(c.id) === top)) (top = ""), (sub = "");
    topPick.set(tops.map((c) => ({ value: String(c.id), label: c.name })), top);
    fillSubs(sub);
    if (sync) syncSave();
  };
  expBtn.onclick = () => ((isIncome = false), fillCats());
  incBtn.onclick = () => ((isIncome = true), fillCats());
  fillCats(false);
  for (const c of [topPick.input, subPick.input]) c.addEventListener("change", () => (bizPick.clear(), syncBiz(), rememberedBiz(), syncSave()));
  repeat.addEventListener("change", () => (syncBiz(), syncSave()));
  bizPick.onchange = syncSave;
  syncBiz();

  const cancel = el("button", { type: "button", className: "btn", textContent: seriesId !== undefined ? "סגירה" : "ביטול" });
  cancel.onclick = () => dlg.close();
  const save = el("button", { type: "submit", className: "btn primary", textContent: existing ? "עדכון" : "שמירה" });
  /** The line as the server takes it, for a single transaction and for one month of a series. */
  const buildBody = (): TxInput =>
    buildTxBody({
      date: date.value,
      amountText: amount.value,
      isIncome,
      categoryId,
      paymentMethodId: existing?.payment_method_id ?? null,
      note: note.value,
      currency: cur(),
      origMinor: origMinor(),
      ilsText: ils.value,
      ilsTouched,
      repeating: repeat.checked,
      forceLookup,
      fxUnchanged: fxUnchanged(),
      savedAgorot: existing?.amount_agorot,
    });
  // Only this month: becomes a transaction with these values; later months are unchanged.
  const updateOnce = async () => {
    try {
      await api.settleRecurringMonth(seriesId!, month, buildBody());
      showPeriodFor(date.value);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = e2 instanceof Error ? e2.message : String(e2);
    }
  };
  // Stop the series: this month and later disappear, earlier months stay.
  const stopFromHere = async () => {
    if (!confirm("לבטל את ההוצאה החוזרת מחודש זה והלאה? חודשים קודמים לא ישתנו.")) return;
    try {
      await api.cancelRecurring(seriesId!, month);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = e2 instanceof Error ? e2.message : String(e2);
    }
  };
  // Skip only this month; the series carries on around it.
  const skipOnce = async () => {
    if (!confirm("לדלג על החיוב של חודש זה בלבד?")) return;
    try {
      await api.skipRecurringMonth(seriesId!, month);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = e2 instanceof Error ? e2.message : String(e2);
    }
  };
  const form = el(
    "form",
    { className: "add", method: "dialog" },
    el("div", { className: "seg" }, expBtn, incBtn),
    el("div", { className: "amountrow" }, el("div", { className: "field" }, amountLbl, amount), curSeg),
    fxBox,
    el("div", { className: "field" }, el("span", { textContent: "קטגוריה" }), topPick.root),
    el("div", { className: "field" }, el("span", { textContent: "תת-קטגוריה" }), subPick.root),
    el("div", { className: "field" }, el("span", { textContent: "תאריך" }), dateField(date)),
    el("div", { className: "field" }, el("span", { textContent: "הערה" }), note),
    bizRow,
    // Only the checkbox (inside a finger-sized hit box) toggles; the row and its text are inert.
    !existing ? el("div", { className: "repeat-row" }, el("label", { className: "repeat-hit" }, repeat), el("span", { textContent: "חוזר מדי חודש" })) : null,
    repeatBox,
    err,
    seriesId !== undefined
      ? el(
          "div",
          { className: "actions" },
          cancel,
          menuButton("ביטול חיוב", [["חודש זה בלבד", skipOnce], ["מכאן והלאה", stopFromHere]]),
          menuButton("עדכון", [["חודש זה בלבד", updateOnce], ["מכאן והלאה", () => dlg.querySelector("form")!.requestSubmit()]], "btn primary"),
        )
      : el("div", { className: "actions" }, cancel, save),
  );
  // The update menu of a repeating charge is greyed out the same way.
  form.querySelectorAll(".menuwrap > button.primary").forEach((b) => b.classList.add("needs-category"));
  dlg.append(form);
  syncFx();
  if (!startFx) void refreshQuote();
  syncSave();
  form.onsubmit = async (e) => {
    e.preventDefault();
    try {
      const body = buildBody();
      const catId = categoryId();
      save.disabled = true;
      if (repeat.checked) {
        const rule = buildRecurringRule({ body, categoryId: catId, date: date.value, duration: forever.value, months: Number(months.value), seriesMonth: seriesId !== undefined ? month : undefined, segmentDay: segment?.day });
        if (seriesId !== undefined) await api.changeRecurring(seriesId, rule);
        else await api.addRecurring(rule);
      } else if (existing) {
        await api.updateTransaction(existing.id, body);
        if (bizPick.value !== ownTag) await api.tagTransactions([existing.id], bizPick.value);
      } else {
        const chosen = bizPick.value;
        try {
          const id = await api.addTransaction(body);
          if (chosen != null) await api.tagTransactions([id], chosen);
        } catch (e3) {
          // No connection: keep the line on this device and send it when the connection is back.
          if (!isNetworkError(e3)) throw e3;
          await enqueue(body, chosen);
          requestPersistence();
        }
      }
      try {
        localStorage.setItem(lastKey(catId), bizPick.value == null ? "" : String(bizPick.value));
      } catch {
        /* storage unavailable */
      }
      showPeriodFor(date.value);
      dlg.close();
      await load();
    } catch (e2) {
      err.textContent = isNetworkError(e2) ? "אין חיבור: בלי חיבור אפשר רק להוסיף תנועה חדשה (לא חוזרת)" : e2 instanceof Error ? e2.message : String(e2);
      save.disabled = false;
    }
  };
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  trackDialog(dlg);
  // Do not put the cursor (and the keyboard) on a field when the dialog opens.
  dlg.tabIndex = -1;
  dlg.setAttribute("autofocus", "");
  dlg.showModal();
  if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
}
