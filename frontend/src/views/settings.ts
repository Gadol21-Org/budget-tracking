import { el } from "../ui";
import { api } from "../api";
import { load } from "../sync";
import { businessesLinkCard, categoriesLinkCard, currenciesLinkCard } from "./categories";
import { openLog } from "./log";

export function settingsView(): HTMLElement {
  const out = el("div", { className: "note muted" });
  const run = (btn: HTMLButtonElement, work: () => Promise<string>) => async () => {
    btn.disabled = true;
    out.className = "note muted";
    out.textContent = "מבצע…";
    try {
      out.textContent = await work();
      await load();
    } catch (e) {
      out.className = "note error";
      out.textContent = `שגיאה: ${e instanceof Error ? e.message : e}`;
    } finally {
      btn.disabled = false;
    }
  };
  const imp = el("button", { className: "btn", textContent: "ייבוא מפעמונים" });
  imp.onclick = run(imp, async () => {
    const s = await api.importPaamonim("2000-01-01", new Date().toISOString().slice(0, 10));
    return `יובאו ${s.transactions_added} תנועות חדשות (${s.transactions_skipped} כבר קיימות), ${s.categories_added} קטגוריות חדשות.`;
  });
  const exp = el("button", { className: "btn", textContent: "ייצוא ל-Google Sheets" });
  exp.onclick = run(exp, async () => {
    const s = await api.exportSheets();
    return `יוצאו ${s.transactions} תנועות.`;
  });
  return el(
    "div",
    {},
    el("h1", { textContent: "הגדרות" }),
    el("div", { className: "card" }, el("p", { className: "muted", textContent: "ייבוא מושך את כל ההיסטוריה מפעמונים ואינו יוצר כפילויות. ייצוא כותב עותק לגיליון Google." }), el("div", { className: "actions", style: "justify-content:flex-start" }, imp, exp), out),
    categoriesLinkCard(),
    businessesLinkCard(),
    currenciesLinkCard(),
    historyCard(),
  );
}

export function historyCard(): HTMLElement {
  const open = el("button", { className: "btn", textContent: "צפייה בהיסטוריית שינויים" });
  open.onclick = () => void openLog().catch(() => alert("טעינת ההיסטוריה נכשלה"));
  return el(
    "div",
    { className: "card" },
    el("h2", { textContent: "היסטוריית שינויים" }),
    el("p", { className: "muted", textContent: "כל הוספה, עריכה ומחיקה של תנועות, קטגוריות והוצאות חוזרות, עם הערכים לפני ואחרי. נשמרת שנה אחת." }),
    el("div", { className: "actions", style: "justify-content:flex-start" }, open),
  );
}
