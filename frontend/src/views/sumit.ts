import { el } from "../ui";
import { api, type SumitOverview } from "../api";
import { state } from "../state";
import { catPath } from "../selectors";
import { openDialog } from "../nav";
import { load } from "../sync";
import { combo } from "../widgets";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString("he-IL", { dateStyle: "short", timeStyle: "short" }) : "—");

function b64urlToBytes(s: string): Uint8Array<ArrayBuffer> {
  const raw = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

/** Ask for notification permission and register this device for "new receipt" pushes. */
export async function enablePush(key: string): Promise<string> {
  if (!("serviceWorker" in navigator) || !("PushManager" in window)) return "הדפדפן אינו תומך בהתראות";
  if ((await Notification.requestPermission()) !== "granted") return "ההרשאה להתראות לא ניתנה";
  const reg = await navigator.serviceWorker.ready;
  const sub = (await reg.pushManager.getSubscription()) ?? (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64urlToBytes(key) }));
  await api.pushSubscribe(sub.endpoint);
  return "ההתראות הופעלו במכשיר זה";
}

/** Settings card: check Sumit for new receipts now, see recent ones, and open the mapping. */
export function sumitCard(): HTMLElement {
  const status = el("div", { className: "note muted", textContent: "טוען…" });
  const held = el("div", { className: "note error" });
  const list = el("div");
  const out = el("div", { className: "note muted" });
  const check = el("button", { className: "btn primary", textContent: "בדיקת קבלות חדשות עכשיו" });
  const map = el("button", { className: "btn", textContent: "הגדרת מוצרים ועסקים" });
  const push = el("button", { className: "btn", textContent: "הפעלת התראות במכשיר" });
  push.style.display = "none";
  let data: SumitOverview | undefined;
  let painted = "";
  const paint = (o: SumitOverview) => {
    data = o;
    state.sumitData = o;
    // Nothing changed since the last paint (the cached copy was already shown): leave the card alone, no flash.
    const sig = JSON.stringify(o);
    if (sig === painted) return;
    painted = sig;
    if (!o.configured) {
      status.textContent = "החיבור ל-Sumit עדיין לא הוגדר בשרת.";
      check.disabled = true;
      return;
    }
    status.textContent = `בדיקה אחרונה: ${when(o.last_poll_at)} · הבאה: ${when(o.next_poll_at)} · קריאות החודש: ${o.calls_this_month}/${o.monthly_limit}${o.last_error ? ` · שגיאה: ${o.last_error}` : ""}`;
    push.style.display = state.sumit.offerPush ? "" : "none";
    held.textContent = o.held.length ? `${o.held.length} קבלות ממתינות: חסרה תת-קטגוריית הכנסה (${[...new Set(o.held.flatMap((h) => h.products))].join(", ") || "ללא פירוט מוצרים"}). הגדירו אותה ב"הגדרת מוצרים ועסקים" והקבלות ייובאו.` : "";
    list.replaceChildren(
      // Only receipts not seen yet: once the card has shown them they are marked read and drop off.
      ...o.notices.filter((n) => !n.read).slice(0, 5).map((n) => el("div", { className: "row" }, el("div", { className: "grow" }, el("div", { className: "name", textContent: `● ${n.title}` }), el("div", { className: "muted", textContent: `${when(n.at)} · ${n.body}` })))),
    );
    if (o.notices.some((n) => !n.read)) void api.sumitRead().then(() => (state.sumit.unread = 0)).catch(() => {});
  };
  const refresh = () => api.sumit().then(paint).catch(() => !data && (status.textContent = "טעינת הנתונים נכשלה"));
  // Draw from the cached overview at once; the fresh one replaces it only if it differs.
  if (state.sumitData) paint(state.sumitData);
  void refresh();
  check.onclick = async () => {
    check.disabled = true;
    out.className = "note muted";
    out.textContent = "בודק…";
    try {
      const r = await api.sumitPoll();
      out.textContent = r.baseline ? "נקבעה נקודת התחלה: הקבלות הקיימות לא יובאו, רק קבלות חדשות מעכשיו." : r.new_receipts ? `נמצאו ${r.new_receipts} קבלות חדשות (${r.transactions} תנועות הכנסה).` : "אין קבלות חדשות.";
      await load();
    } catch (e) {
      out.className = "note error";
      out.textContent = `שגיאה: ${e instanceof Error ? e.message : e}`;
    } finally {
      check.disabled = false;
      await refresh();
    }
  };
  map.onclick = () => data && openMapping(data, refresh);
  push.onclick = async () => {
    try {
      out.textContent = await enablePush(data!.push_public_key!);
      push.style.display = "none";
    } catch (e) {
      out.className = "note error";
      out.textContent = `שגיאה: ${e instanceof Error ? e.message : e}`;
    }
  };
  return el(
    "div",
    { className: "card" },
    el("h2", { textContent: "קבלות מ-Sumit" }),
    el("p", { className: "muted", textContent: "קבלות חדשות נמשכות אוטומטית שלוש פעמים ביום (07:00, 13:00, 19:00) ונרשמות כהכנסה. לכל בדיקה יש מכסה חודשית בצד של Sumit, לכן אין לחצן רענון אוטומטי." }),
    status,
    held,
    list,
    el("div", { className: "actions", style: "justify-content:flex-start;flex-wrap:wrap" }, check, map, push),
    out,
  );
}

/** Choose the income category receipts go to, and the business of each product. */
export function openMapping(o: SumitOverview, done: () => void, only?: string[]) {
  openDialog((close) => {
    const wanted = only && new Set(only.map((n) => n.trim().toLowerCase()));
    const rows = o.products.filter((p) => !wanted || wanted.has(p.name.trim().toLowerCase())).map((p) => {
      const biz = el("select");
      biz.append(el("option", { value: "", textContent: "ללא עסק" }), ...state.biz.businesses.map((b) => el("option", { value: String(b.id), textContent: b.name })));
      biz.value = p.business_id != null ? String(p.business_id) : "";
      // The same searchable category / sub-category pickers as the transaction form.
      const income = state.cats.filter((c) => c.is_income);
      const kidsOf = (top: string) => income.filter((k) => String(k.parent_id) === top);
      const top = combo("חיפוש קטגוריה…");
      const sub = combo("תת-קטגוריה");
      top.set(income.filter((c) => c.parent_id == null).map((c) => ({ value: String(c.id), label: c.name })));
      const fillSubs = (keep = "") => {
        const kids = top.value ? kidsOf(top.value) : income.filter((k) => k.parent_id != null);
        sub.set(kids.map((k) => ({ value: String(k.id), label: top.value ? k.name : catPath(k.id) })), kids.some((k) => String(k.id) === keep) ? keep : "");
        sub.setDisabled(top.value !== "" && kids.length === 0, top.value && kids.length === 0 ? "אין תתי-קטגוריות" : "חיפוש תת-קטגוריה…");
      };
      const parent = (id: number) => income.find((c) => c.id === id)?.parent_id;
      if (p.category_id != null) top.pick(String(parent(p.category_id) ?? p.category_id));
      fillSubs(p.category_id != null && parent(p.category_id) != null ? String(p.category_id) : "");
      /** The category the receipts go to: the sub-category, or the category itself when it has none. Empty until a usable one is picked. */
      const target = () => sub.value || (top.value && kidsOf(top.value).length === 0 ? top.value : "");
      const save = () => void api.sumitProduct(p.name, biz.value ? Number(biz.value) : null, target() ? Number(target()) : null);
      biz.onchange = save;
      top.input.addEventListener("change", () => (fillSubs(), save()));
      sub.input.addEventListener("change", () => {
        const par = parent(Number(sub.value));
        if (par != null && top.value !== String(par)) (top.pick(String(par)), fillSubs(sub.value));
        save();
      });
      return el("div", { className: "card" }, el("div", { className: "name", textContent: p.name }), el("label", {}, "עסק", biz), el("label", {}, "קטגוריה", top.root), el("label", {}, "תת-קטגוריה", sub.root));
    });
    // The browser focuses the first control of a dialog; keep it on the close button so no picker is highlighted or opened.
    const ok = el("button", { type: "button", className: "btn primary", textContent: "סגירה", autofocus: true });
    ok.onclick = () => (close(), done());
    return el(
      "form",
      { className: "add", method: "dialog" },
      el("h2", { textContent: "קבלות מ-Sumit: מוצרים" }),
      el("h3", { textContent: only ? "הקבלה ממתינה: בחרו קטגוריית הכנסה למוצר" : "עסק וקטגוריה לכל מוצר" }),
      ...(rows.length ? rows : [el("div", { className: "note muted", textContent: "מוצרים יופיעו כאן אחרי הבדיקה הראשונה. כל מוצר צריך קטגוריית הכנסה (תת-קטגוריה) משלו, אחרת הקבלה ממתינה. קבלה עם מוצרים מעסקים או מקטגוריות שונים תתפצל לכמה תנועות." })]),
      el("div", { className: "actions" }, ok),
    );
  });
}
