import { el } from "../ui";
import { bizName, openCategoryBusinessDialog } from "../business";
import { api, type Category } from "../api";
import { state } from "../state";
import { go, goBack, manageCtx, openDialog } from "../nav";
import { load } from "../sync";
import { catPath } from "../selectors";
import { render } from "../render";

/** Paamonim top-level categories of a kind: what a top-level category made here can stand in for. */
export const paamonimTops = (isIncome: boolean) => state.cats.filter((c) => c.parent_id == null && c.from_paamonim && c.is_income === isIncome);

/** The Paamonim top-level category a top-level category's lines go under, if any. */
export const paamonimHome = (c: Category): number | null => (c.parent_id != null ? null : c.from_paamonim ? c.id : c.linked_to);

export function paamonimSelect(isIncome: boolean): HTMLSelectElement {
  const sel = el("select");
  sel.append(el("option", { value: "", textContent: "בחירת קטגוריה בפעמונים…" }), ...paamonimTops(isIncome).map((c) => el("option", { value: String(c.id), textContent: c.name })));
  return sel;
}

export function openAddCategoryDialog(isIncome: boolean, parentId: number | null) {
  openDialog((close) => {
    const name = el("input", { type: "text", required: true, maxLength: 100, autocomplete: "off" });
    const parent = el("select");
    parent.append(
      el("option", { value: "", textContent: "קטגוריה ראשית" }),
      ...state.cats.filter((c) => c.parent_id == null && c.is_income === isIncome).map((c) => el("option", { value: String(c.id), textContent: `תת-קטגוריה של ${c.name}` })),
    );
    parent.value = parentId != null ? String(parentId) : "";
    // Paamonim has no way to add a top-level category, so a new one stands in for one there.
    const hasTops = paamonimTops(isIncome).length > 0;
    const link = paamonimSelect(isIncome);
    const linkRow = el("label", {}, "קטגוריה בפעמונים", link);
    const note = el("div", { className: "note muted" });
    const sync = () => {
      const top = !parent.value;
      linkRow.style.display = top && hasTops ? "" : "none";
      link.required = top && hasTops;
      const p = state.cats.find((c) => String(c.id) === parent.value);
      note.textContent = top
        ? hasTops
          ? "פעמונים לא מאפשר קטגוריות ראשיות חדשות. תתי-הקטגוריות של קטגוריה זו יירשמו בפעמונים תחת הקטגוריה שנבחרה."
          : "קטגוריה שנוספה כאן קיימת רק באפליקציה: תנועות עליה לא יועתקו לפעמונים."
        : p && paamonimHome(p) != null
          ? "תת-הקטגוריה תירשם בפעמונים כסעיף באותו שם (אם כבר יש סעיף בשם זה, הוא ישמש)."
          : "קטגוריה שנוספה כאן קיימת רק באפליקציה: תנועות עליה לא יועתקו לפעמונים.";
    };
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "הוספה" });
    // Greyed out until there is a name and, for a top-level category, its Paamonim category.
    const syncSave = () => (save.disabled = !name.value.trim() || (!parent.value && hasTops && !link.value));
    parent.onchange = () => (sync(), syncSave());
    link.onchange = syncSave;
    name.oninput = syncSave;
    sync();
    syncSave();
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: isIncome ? "הוספת קטגוריית הכנסה" : "הוספת קטגוריית הוצאה" }),
      el("label", {}, "שם", name),
      el("label", {}, "סוג", parent),
      linkRow,
      note,
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        const top = !parent.value;
        await api.addCategory({
          name: name.value,
          is_income: isIncome,
          parent_id: top ? null : Number(parent.value),
          linked_to: top && link.value ? Number(link.value) : null,
        });
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        syncSave();
      }
    };
    setTimeout(() => name.focus());
    return form;
  });
}

/** Choose which Paamonim category a top-level category made here stands in for. */
export function openLinkCategoryDialog(cat: Category) {
  openDialog((close) => {
    const link = paamonimSelect(cat.is_income);
    link.required = true;
    link.value = cat.linked_to != null ? String(cat.linked_to) : "";
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "שמירה" });
    const syncSave = () => (save.disabled = !link.value || link.value === String(cat.linked_to));
    link.onchange = syncSave;
    syncSave();
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `"${cat.name}" בפעמונים` }),
      el("label", {}, "קטגוריה בפעמונים", link),
      el("div", { className: "note muted", textContent: "תתי-הקטגוריות של קטגוריה זו יירשמו בפעמונים תחת הקטגוריה שנבחרה." }),
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        await api.linkCategory(cat.id, Number(link.value));
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        syncSave();
      }
    };
    return form;
  });
}

/** Move a sub-category, with its transactions and repeating rules, under another top-level category. */
export function openMoveCategoryDialog(cat: Category) {
  openDialog((close) => {
    const from = state.cats.find((c) => c.id === cat.parent_id);
    const home = from ? paamonimHome(from) : null;
    // An item in Paamonim stays where it is there, so it can only move within its Paamonim category.
    const targets = state.cats.filter(
      (c) => c.parent_id == null && c.is_income === cat.is_income && c.id !== cat.parent_id && (!cat.from_paamonim || paamonimHome(c) === home),
    );
    const lines = state.txs.filter((t) => t.category_id === cat.id && !t.upcoming).length;
    const target = el("select");
    target.required = true;
    target.append(el("option", { value: "", textContent: "בחירת קטגוריה…" }), ...targets.map((c) => el("option", { value: String(c.id), textContent: c.name })));
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const save = el("button", { type: "submit", className: "btn primary", textContent: "העברה" });
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `העברת "${cat.name}"` }),
      el("p", { className: "muted", textContent: `כרגע תחת "${from?.name ?? ""}". ${lines} התנועות שלה והתנועות החוזרות עוברות איתה.` }),
      targets.length
        ? el("label", {}, "העברה אל", target)
        : el("p", { className: "muted", textContent: "אין קטגוריה ראשית אחרת שאפשר להעביר אליה." }),
      cat.from_paamonim && el("div", { className: "note muted", textContent: "זהו סעיף בפעמונים, ולכן אפשר להעביר אותו רק לקטגוריות המקושרות לאותה קטגוריה בפעמונים. בפעמונים דבר לא משתנה." }),
      err,
      el("div", { className: "actions" }, cancel, save),
    );
    const syncSave = () => (save.disabled = !target.value);
    target.onchange = syncSave;
    syncSave();
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        save.disabled = true;
        await api.moveCategory(cat.id, Number(target.value));
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        syncSave();
      }
    };
    return form;
  });
}

export function openDeleteCategoryDialog(cat: Category) {
  openDialog((close) => {
    const subs = state.cats.filter((c) => c.parent_id === cat.id);
    const doomed = new Set([cat.id, ...subs.map((c) => c.id)]);
    // Occurrences not yet settled are not in txs, so repeating rules are counted separately.
    const lines = state.txs.filter((t) => t.category_id != null && doomed.has(t.category_id) && !t.upcoming).length;
    const rules = new Set(state.recurring.filter((r) => r.category_id != null && doomed.has(r.category_id)).map((r) => r.series_id)).size;
    const used = lines + rules;
    const target = el("select");
    const opt = (c: Category) => el("option", { value: String(c.id), textContent: catPath(c.id) });
    target.append(
      el("option", { value: "", textContent: "בחירת קטגוריה…" }),
      ...state.cats.filter((c) => c.is_income === cat.is_income && !doomed.has(c.id)).map(opt),
    );
    const err = el("div", { className: "error" });
    const cancel = el("button", { type: "button", className: "btn", textContent: "ביטול" });
    cancel.onclick = close;
    const del = el("button", { type: "submit", className: "btn primary", textContent: "מחיקה" });
    const form = el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: `מחיקת "${cat.name}"` }),
      subs.length > 0 && el("p", { className: "muted", textContent: `יימחקו גם ${subs.length} תתי-קטגוריות.` }),
      used > 0
        ? el("label", {}, `${lines} תנועות ו-${rules} הוצאות חוזרות משויכות לקטגוריה. להעביר אותן אל:`, target)
        : el("p", { className: "muted", textContent: "אין תנועות משויכות." }),
      err,
      el("div", { className: "actions" }, cancel, del),
    );
    if (used > 0) target.required = true;
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        del.disabled = true;
        await api.deleteCategory(cat.id, target.value ? Number(target.value) : undefined);
        close();
        await load();
      } catch (e2) {
        err.textContent = e2 instanceof Error ? e2.message : String(e2);
        del.disabled = false;
      }
    };
    return form;
  });
}

export function categoriesView(): HTMLElement {
  const back = el("button", { className: "btn", textContent: "→ הגדרות", title: "חזרה להגדרות" });
  back.onclick = () => {
    goBack("settings");
  };
  return el("div", {}, el("div", { className: "actions", style: "justify-content:flex-start" }, back), el("h1", { textContent: "קטגוריות" }), categoriesCard());
}

export function categoriesLinkCard(): HTMLElement {
  const open = el("button", { className: "btn", textContent: "עריכת קטגוריות" });
  open.onclick = () => {
    go("categories");
  };
  return el("div", { className: "card" }, el("h2", { textContent: "קטגוריות" }), el("p", { className: "muted", textContent: "הוספה ומחיקה של קטגוריות ותתי-קטגוריות." }), el("div", { className: "actions", style: "justify-content:flex-start" }, open));
}

export function businessesLinkCard(): HTMLElement {
  const open = el("button", { className: "btn", textContent: "ניהול עסקים" });
  open.onclick = () => go("businesses");
  return el("div", { className: "card" }, el("h2", { textContent: "עסקים" }), el("p", { className: "muted", textContent: "עסקים ותתי-עסקים לצפייה ברווח והפסד, בלי להוציא את התנועות מסיכום משק הבית." }), el("div", { className: "actions", style: "justify-content:flex-start" }, open));
}

export function currenciesLinkCard(): HTMLElement {
  const open = el("button", { className: "btn", textContent: "ניהול מטבעות" });
  open.onclick = () => go("currencies");
  return el("div", { className: "card" }, el("h2", { textContent: "מטבעות" }), el("p", { className: "muted", textContent: "בחירת המטבעות שמוצגים בהוספת תנועה, וסדר הופעתם." }), el("div", { className: "actions", style: "justify-content:flex-start" }, open));
}

export function categoriesCard(): HTMLElement {
  const kinds = [
    [false, "הוצאות"],
    [true, "הכנסות"],
  ] as const;
  const tabs = kinds.map(([inc, label]) => {
    const b = el("button", { type: "button", textContent: label });
    b.onclick = () => ((state.catKind = inc), render());
    b.setAttribute("aria-pressed", String(state.catKind === inc));
    return b;
  });
  const ofKind = state.cats.filter((c) => c.is_income === state.catKind);
  const iconBtn = (text: string, title: string, on: () => void) => {
    const b = el("button", { type: "button", className: "btn", textContent: text, title });
    b.onclick = on;
    return b;
  };
  const linkedName = (c: Category) => state.cats.find((x) => x.id === c.linked_to)?.name;
  const row = (c: Category, sub: boolean) =>
    el(
      "div",
      { className: sub ? "row sub" : "row" },
      el(
        "div",
        { className: "grow" },
        el("div", { className: "name", textContent: c.name }),
        !sub && c.linked_to != null && el("div", { className: "muted", textContent: `בפעמונים: ${linkedName(c) ?? ""}` }),
        !sub && !c.from_paamonim && c.linked_to == null && el("div", { className: "muted", textContent: "לא מקושרת לפעמונים" }),
        state.biz.cat.has(c.id) ? el("div", {}, el("span", { className: "bizchip", textContent: `עסק: ${bizName(state.biz, state.biz.cat.get(c.id))}` })) : null,
      ),
      state.biz.businesses.length > 0 && iconBtn("💼", "עסק של הקטגוריה", () => openCategoryBusinessDialog(manageCtx(), c)),
      !sub && !c.from_paamonim && iconBtn("🔗", "קטגוריה בפעמונים", () => openLinkCategoryDialog(c)),
      !sub && iconBtn("+", "הוספת תת-קטגוריה", () => openAddCategoryDialog(c.is_income, c.id)),
      sub && iconBtn("⇄", "העברה לקטגוריה אחרת", () => openMoveCategoryDialog(c)),
      iconBtn("🗑", "מחיקה", () => openDeleteCategoryDialog(c)),
    );
  const rows = ofKind
    .filter((c) => c.parent_id == null)
    .flatMap((c) => [row(c, false), ...ofKind.filter((k) => k.parent_id === c.id).map((k) => row(k, true))]);
  return el(
    "div",
    { className: "card" },
    el("div", { className: "seg" }, ...tabs),
    el("div", { className: "rows" }, ...(rows.length ? rows : [el("div", { className: "empty", textContent: "אין קטגוריות" })])),
    el("div", { className: "actions", style: "justify-content:flex-start" }, iconBtn("הוספת קטגוריה", "", () => openAddCategoryDialog(state.catKind, null))),
  );
}
