import { useEffect, useState } from "react";
import { api, type EmailStatus } from "../api";
import { load } from "../sync";
import { openEmailInbox } from "./emailInbox";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString("he-IL", { dateStyle: "short", timeStyle: "short" }) : "עדיין לא");

function b64urlToBytes(s: string): Uint8Array<ArrayBuffer> {
  const raw = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

const pushSupported = () => "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;

/** Ask for notification permission and register this device for "new suggestion" pushes. */
async function enablePush(key: string): Promise<string> {
  if (!pushSupported()) return "הדפדפן אינו תומך בהתראות";
  if ((await Notification.requestPermission()) !== "granted") return "ההרשאה להתראות לא ניתנה";
  const reg = await navigator.serviceWorker.ready;
  const sub = (await reg.pushManager.getSubscription()) ?? (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64urlToBytes(key) }));
  await api.pushSubscribe(sub.endpoint);
  return "ההתראות הופעלו במכשיר זה";
}

/** Settings card: how mail reaches the app, when the last message came, and the way into the suggestions. Hidden offline. */
export function EmailCard({ status }: { status: EmailStatus | null }) {
  const [busy, setBusy] = useState(false);
  const [out, setOut] = useState<{ text: string; error: boolean } | null>(null);
  const [offerPush, setOfferPush] = useState(false);
  const key = status?.push_public_key;
  // Offer notifications only while the server does not hold this device yet.
  useEffect(() => {
    if (!key || !pushSupported()) return;
    let live = true;
    void navigator.serviceWorker.ready
      .then((reg) => reg.pushManager.getSubscription())
      .then((sub) => (sub ? api.pushRegistered(sub.endpoint) : false))
      .catch(() => false)
      .then((registered) => live && setOfferPush(!registered));
    return () => {
      live = false;
    };
  }, [key]);
  if (!status) return null;
  const notify = async () => {
    try {
      setOut({ text: await enablePush(key!), error: false });
      setOfferPush(false);
    } catch (e) {
      setOut({ text: `שגיאה: ${e instanceof Error ? e.message : e}`, error: true });
    }
  };
  const retry = async () => {
    setBusy(true);
    setOut({ text: "מבצע…", error: false });
    try {
      const r = await api.emailRetry();
      setOut({ text: r.read ? `נקראו ${r.read} הודעות.` : "אין הודעות שנקראו (השירות עדיין לא זמין, או שאין הודעות ממתינות).", error: false });
      await load();
    } catch (e) {
      setOut({ text: `שגיאה: ${e instanceof Error ? e.message : e}`, error: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="card">
      <h2>חיובים מדוא״ל</h2>
      {!status.configured ? (
        <p className="muted">קבלת מיילים עוד לא הוגדרה בשרת (EMAIL_INBOUND_SECRET). ההוראות ב-deploy/EMAIL.md.</p>
      ) : (
        <>
          <p className="muted">
            מיילים שמועברים מ-Gmail נקראים על ידי AI. מה שבטוח או עם כלל קבוע נוסף מיד, השאר ממתין לאישורכם.
          </p>
          <p className="muted small">הודעה אחרונה התקבלה: {when(status.last_received)}</p>
          {!status.ai_configured && <div className="note error">חסר GEMINI_API_KEY בשרת, ולכן הודעות ממתינות בלי להיקרא.</div>}
          {status.last_error && <div className="note error">{status.last_error}</div>}
          <div className="actions" style={{ justifyContent: "flex-start" }}>
            <button className="btn primary" onClick={() => void openEmailInbox().catch(() => alert("טעינת ההצעות נכשלה"))}>
              {status.pending ? `תנועות מדוא״ל לאישור (${status.pending})` : "תנועות מדוא״ל לאישור"}
            </button>
            {offerPush && (
              <button className="btn" onClick={() => void notify()}>
                התראה על הצעות חדשות
              </button>
            )}
            {status.queued > 0 && (
              <button className="btn" disabled={busy} onClick={retry}>
                {`קריאת ${status.queued} הודעות ממתינות`}
              </button>
            )}
          </div>
        </>
      )}
      <div className={out?.error ? "note error" : "note muted"}>{out?.text}</div>
    </div>
  );
}
