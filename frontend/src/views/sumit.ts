import { el } from "../ui";
import { api, type SumitOverview } from "../api";
import { state } from "../state";
import { catPath } from "../selectors";
import { openDialog } from "../nav";
import { load } from "../sync";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString("he-IL", { dateStyle: "short", timeStyle: "short" }) : "—");

function b64urlToBytes(s: string): Uint8Array<ArrayBuffer> {
  const raw = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

/** Ask for notification permission and register this device for "new receipt" pushes. */
async function enablePush(key: string): Promise<string> {
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
  const list = el("div");
  const out = el("div", { className: "note muted" });
  const check = el("button", { className: "btn primary", textContent: "בדיקת קבלות חדשות עכשיו" });
  const map = el("button", { className: "btn", textContent: "הגדרת מוצרים ועסקים" });
  const push = el("button", { className: "btn", textContent: "הפעלת התראות במכשיר" });
  push.style.display = "none";
  let data: SumitOverview | undefined;
  const paint = (o: SumitOverview) => {
    data = o;
    if (!o.configured) {
      status.textContent = "החיבור ל-Sumit עדיין לא הוגדר בשרת.";
      check.disabled = true;
      return;
    }
    status.textContent = `בדיקה אחרונה: ${when(o.last_poll_at)} · הבאה: ${when(o.next_poll_at)} · קריאות החודש: ${o.calls_this_month}/${o.monthly_limit}${o.last_error ? ` · שגיאה: ${o.last_error}` : ""}`;
    push.style.display = o.push_public_key ? "" : "none";
    list.replaceChildren(
      ...o.notices.slice(0, 5).map((n) => el("div", { className: "row" }, el("div", { className: "grow" }, el("div", { className: "name", textContent: `${n.read ? "" : "● "}${n.title}` }), el("div", { className: "muted", textContent: `${when(n.at)} · ${n.body}` })))),
    );
    if (o.notices.some((n) => !n.read)) void api.sumitRead().catch(() => {});
  };
  const refresh = () => api.sumit().then(paint).catch(() => (status.textContent = "טעינת הנתונים נכשלה"));
  void refresh();
  check.onclick = async () => {
    check.disabled = true;
    out.className = "note muted";
    out.textContent = "בודק…";
    try {
      const r = await api.sumitPoll();
      out.textContent = r.new_receipts ? `נמצאו ${r.new_receipts} קבלות חדשות (${r.transactions} תנועות הכנסה).` : "אין קבלות חדשות.";
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
    list,
    el("div", { className: "actions", style: "justify-content:flex-start;flex-wrap:wrap" }, check, map, push),
    out,
  );
}

/** Choose the income category receipts go to, and the business of each product. */
function openMapping(o: SumitOverview, done: () => void) {
  openDialog((close) => {
    const cat = el("select");
    cat.append(el("option", { value: "", textContent: "ללא קטגוריה" }), ...state.cats.filter((c) => c.is_income).map((c) => el("option", { value: String(c.id), textContent: c.name })));
    cat.value = o.category_id != null ? String(o.category_id) : "";
    cat.onchange = () => void api.sumitCategory(cat.value ? Number(cat.value) : null);
    // Every income category, sub-categories with their full path, so each product can go to its own.
    const incomeCats = state.cats.filter((c) => c.is_income && !state.cats.some((k) => k.parent_id === c.id));
    const rows = o.products.map((p) => {
      const biz = el("select");
      biz.append(el("option", { value: "", textContent: "ללא עסק" }), ...state.biz.businesses.map((b) => el("option", { value: String(b.id), textContent: b.name })));
      biz.value = p.business_id != null ? String(p.business_id) : "";
      const pc = el("select");
      pc.append(el("option", { value: "", textContent: "קטגוריית ברירת המחדל" }), ...incomeCats.map((c) => el("option", { value: String(c.id), textContent: catPath(c.id) })));
      pc.value = p.category_id != null ? String(p.category_id) : "";
      const save = () => void api.sumitProduct(p.name, biz.value ? Number(biz.value) : null, pc.value ? Number(pc.value) : null);
      biz.onchange = save;
      pc.onchange = save;
      return el("div", { className: "card" }, el("div", { className: "name", textContent: p.name }), el("label", {}, "עסק", biz), el("label", {}, "קטגוריה ותת-קטגוריה", pc));
    });
    const ok = el("button", { type: "button", className: "btn primary", textContent: "סגירה" });
    ok.onclick = () => (close(), done());
    return el(
      "div",
      { className: "add" },
      el("h2", { textContent: "קבלות מ-Sumit: קטגוריה ומוצרים" }),
      el("label", {}, "קטגוריית ברירת מחדל לקבלות", cat),
      el("h3", { textContent: "עסק וקטגוריה לכל מוצר" }),
      ...(rows.length ? rows : [el("div", { className: "note muted", textContent: "מוצרים יופיעו כאן אחרי הקבלה הראשונה. קבלה עם מוצרים מעסקים או מקטגוריות שונים תתפצל לכמה תנועות." })]),
      el("div", { className: "actions" }, ok),
    );
  });
}
