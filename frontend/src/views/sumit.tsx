import { useEffect, useRef, useState } from "react";
import { api, type SumitOverview } from "../api";
import { CategoryPicker, type CategoryChoice } from "../components/CategoryPicker";
import { BackButton } from "../components/BackButton";
import { clickableProps } from "../components/clickable";
import { openReactDialog } from "../components/openReactDialog";
import { SettingsRow } from "../components/SettingsRow";
import { go, goBack } from "../nav";
import { render } from "../render";
import { state } from "../state";
import { load } from "../sync";

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

/** On the summary: receipts waiting for a category (tap to classify their products), or new ones not yet looked at. */
export function SumitBanner() {
  const { held, unread } = state.sumit;
  if (!held && !unread) return null;
  const text = held ? `${held} קבלות מ-Sumit ממתינות לסיווג: הקישו כדי להגדיר קטגוריה` : `${unread} קבלות חדשות מ-Sumit`;
  const open = () => {
    const o = state.sumitData;
    // Held receipts: open the classification of just their products; new ones: the settings card that lists them.
    if (held && o) openMapping(o, () => void load(), [...new Set(o.held.flatMap((h) => h.products))]);
    else go("settings");
  };
  return (
    <div className={held ? "sumit-banner warn" : "sumit-banner"} {...clickableProps(open)}>
      {text}
    </div>
  );
}

const DISMISSED_KEY = "budget.sumit.pushOfferDismissed";

/** The offer on the summary was closed on this device (it stays available in the Sumit settings page). */
function offerDismissed(): boolean {
  try {
    return localStorage.getItem(DISMISSED_KEY) === "1";
  } catch {
    return false;
  }
}

/** Offers to turn on notifications for new Sumit receipts, only while the server does not hold this device yet. The X hides it on this device; the same option is in the Sumit settings page. */
export function PushOffer() {
  if (!state.sumit.offerPush || offerDismissed()) return null;
  const dismiss = () => {
    try {
      localStorage.setItem(DISMISSED_KEY, "1");
    } catch {
      /* private mode: it just comes back next time */
    }
    render();
  };
  const ask = async () => {
    try {
      await enablePush(state.sumit.pushKey!);
      state.sumit.offerPush = false;
    } catch {
      /* the permission stays undecided: the offer comes back */
    }
    render();
  };
  return (
    <div className="sumit-banner offer" role="status">
      <button className="dismiss" aria-label="סגירה" title="סגירה (אפשר להפעיל בהגדרות)" onClick={dismiss}>
        ×
      </button>
      <div>לקבל התראה כשמגיעה קבלה חדשה מ-Sumit?</div>
      <div className="actions" style={{ justifyContent: "flex-start", marginTop: 8 }}>
        <button className="btn primary" onClick={() => void ask()}>
          הפעלה
        </button>
      </div>
    </div>
  );
}

/** Settings page: check Sumit for new receipts now, see receipts not yet seen, and open the mapping. */
export function SumitPage() {
  const [data, setData] = useState<SumitOverview | undefined>(state.sumitData);
  const [failed, setFailed] = useState(false);
  const [out, setOut] = useState<{ text: string; error: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const marked = useRef(false);
  const refresh = () =>
    api
      .sumit()
      .then((o) => {
        state.sumitData = o;
        setData(o);
        setFailed(false);
      })
      .catch(() => setFailed(true));
  useEffect(() => {
    void refresh();
  }, []);
  // Receipts the card has shown are marked read, so they drop off the next time.
  useEffect(() => {
    if (!data || marked.current || !data.notices.some((n) => !n.read)) return;
    marked.current = true;
    void api.sumitRead().then(() => (state.sumit.unread = 0)).catch(() => {});
  }, [data]);
  const check = async () => {
    setBusy(true);
    setOut({ text: "בודק…", error: false });
    try {
      const r = await api.sumitPoll();
      setOut({ error: false, text: r.baseline ? "נקבעה נקודת התחלה: הקבלות הקיימות לא יובאו, רק קבלות חדשות מעכשיו." : r.new_receipts ? `נמצאו ${r.new_receipts} קבלות חדשות (${r.transactions} תנועות הכנסה).` : "אין קבלות חדשות." });
      await load();
    } catch (e) {
      setOut({ error: true, text: `שגיאה: ${e instanceof Error ? e.message : e}` });
    } finally {
      setBusy(false);
      await refresh();
    }
  };
  const push = async () => {
    try {
      setOut({ error: false, text: await enablePush(data!.push_public_key!) });
      state.sumit.offerPush = false;
      render();
    } catch (e) {
      setOut({ error: true, text: `שגיאה: ${e instanceof Error ? e.message : e}` });
    }
  };
  const unseen = data?.notices.filter((n) => !n.read).slice(0, 5) ?? [];
  const products = data ? [...new Set(data.held.flatMap((h) => h.products))].join(", ") : "";
  const status = !data
    ? failed
      ? "טעינת הנתונים נכשלה"
      : "טוען…"
    : !data.configured
      ? "החיבור ל-Sumit עדיין לא הוגדר בשרת."
      : `בדיקה אחרונה: ${when(data.last_poll_at)} · הבאה: ${when(data.next_poll_at)} · קריאות החודש: ${data.calls_this_month}/${data.monthly_limit}${data.last_error ? ` · שגיאה: ${data.last_error}` : ""}`;
  return (
    <div>
      <div className="card filterbar">
        <BackButton onClick={() => goBack("settings")} />
        <div className="grow">
          <div className="name">קבלות מ-Sumit</div>
        </div>
      </div>
      <div className="sfoot top">קבלות חדשות נמשכות אוטומטית שלוש פעמים ביום (07:00, 13:00, 19:00) ונרשמות כהכנסה. לכל בדיקה יש מכסה חודשית בצד של Sumit, לכן אין רענון אוטומטי.</div>
      {data && data.held.length > 0 && (
        <div className="sgroup">
          <div className="snote error">{`${data.held.length} קבלות ממתינות: חסרה תת-קטגוריית הכנסה (${products || "ללא פירוט מוצרים"}). הגדירו ב"מוצרי Sumit" והקבלות ייובאו.`}</div>
        </div>
      )}
      <div className="sgroup">
        <SettingsRow icon="⚙" color={["#b4b4ba", "#7c7c82"]} label="מוצרי Sumit" value={data?.products.length ? String(data.products.length) : ""} onClick={() => data && openMapping(data, () => void refresh())} />
        <button className="srow sbtn" disabled={busy || !data?.configured} onClick={() => void check()}>
          בדיקת קבלות חדשות עכשיו
        </button>
        {state.sumit.offerPush && (
          <button className="srow sbtn" onClick={() => void push()}>
            הפעלת התראות במכשיר
          </button>
        )}
      </div>
      <div className="sfoot">{status}</div>
      {out && <div className={out.error ? "note error" : "note muted"}>{out.text}</div>}
      {unseen.length > 0 && (
        <div className="sgroup">
          {unseen.map((n, i) => (
            <div className="srow notice" key={i}>
              <div className="slabel">
                <div className="name">{`● ${n.title}`}</div>
                <div className="muted">{`${when(n.at)} · ${n.body}`}</div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Business and income category of one product; every change is saved at once. */
function ProductRow({ p }: { p: SumitOverview["products"][number] }) {
  const income = state.cats.filter((c) => c.is_income);
  const own = income.find((c) => c.id === p.category_id);
  const [biz, setBiz] = useState(p.business_id != null ? String(p.business_id) : "");
  const [pick, setPick] = useState<CategoryChoice>(own ? (own.parent_id != null ? { top: String(own.parent_id), sub: String(own.id) } : { top: String(own.id), sub: "" }) : { top: "", sub: "" });
  /** The category the receipts go to: the sub-category, or the category itself when it has none. Empty until a usable one is picked. */
  const target = (c: CategoryChoice) => c.sub || (c.top && !income.some((k) => String(k.parent_id) === c.top) ? c.top : "");
  const save = (b: string, c: CategoryChoice) => void api.sumitProduct(p.name, b ? Number(b) : null, target(c) ? Number(target(c)) : null);
  return (
    <div className="card">
      <div className="name">{p.name}</div>
      <div className="field">
        <span>מוצר באפליקציה</span>
        <select
          value={biz}
          onChange={(e) => {
            setBiz(e.target.value);
            save(e.target.value, pick);
          }}
        >
          <option value="">ללא מוצר באפליקציה</option>
          {state.products.products.map((b) => (
            <option key={b.id} value={String(b.id)}>
              {b.name}
            </option>
          ))}
        </select>
      </div>
      <CategoryPicker
        cats={income}
        value={pick}
        onChange={(c) => {
          setPick(c);
          save(biz, c);
        }}
      />
    </div>
  );
}

/** The business and income category of each product (or only of `only`, the products of receipts waiting for one). */
export function openMapping(o: SumitOverview, done: () => void, only?: string[]) {
  const wanted = only && new Set(only.map((n) => n.trim().toLowerCase()));
  const rows = o.products.filter((p) => !wanted || wanted.has(p.name.trim().toLowerCase()));
  openReactDialog(
    (close) => (
      <form
        className="add"
        method="dialog"
        style={{ justifyContent: "flex-start" }}
        onSubmit={(e) => {
          e.preventDefault();
          close();
          done();
        }}
      >
        <h2>קבלות מ-Sumit: מוצרים</h2>
        <h3>{only ? "הקבלה ממתינה: בחרו קטגוריית הכנסה למוצר" : "מוצר באפליקציה וקטגוריה לכל מוצר Sumit"}</h3>
        {rows.length ? (
          rows.map((p) => <ProductRow key={p.name} p={p} />)
        ) : (
          <div className="note muted">מוצרים יופיעו כאן אחרי הבדיקה הראשונה. כל מוצר צריך קטגוריית הכנסה (תת-קטגוריה) משלו, אחרת הקבלה ממתינה. קבלה עם מוצרי Sumit שמקושרים למוצרים או לקטגוריות שונים תתפצל לכמה תנועות.</div>
        )}
        <div className="actions">
          <button type="submit" className="btn primary">
            סגירה
          </button>
        </div>
      </form>
    ),
    "fullpage",
  );
}

export function sumitView() {
  return <SumitPage />;
}
