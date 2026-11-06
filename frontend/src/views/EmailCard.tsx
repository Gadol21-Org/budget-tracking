import { useState } from "react";
import { api, type EmailStatus } from "../api";
import { load } from "../sync";
import { openEmailInbox } from "./emailInbox";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString("he-IL", { dateStyle: "short", timeStyle: "short" }) : "עדיין לא");

/** Settings card: how mail reaches the app, when the last message came, and the way into the suggestions. Hidden offline. */
export function EmailCard({ status }: { status: EmailStatus | null }) {
  const [busy, setBusy] = useState(false);
  const [out, setOut] = useState<{ text: string; error: boolean } | null>(null);
  if (!status) return null;
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
      <h2>הצעות תנועות מדוא״ל</h2>
      {!status.configured ? (
        <p className="muted">קבלת מיילים עוד לא הוגדרה בשרת (EMAIL_INBOUND_SECRET). ההוראות ב-deploy/EMAIL.md.</p>
      ) : (
        <>
          <p className="muted">
            הודעות ש-Gmail מעביר לכתובת הייעודית נקראות על ידי AI והופכות להצעות לבדיקה. תנועה נוספת רק אחרי שאישרתם אותה, והאפליקציה לא מחזיקה גישה לתיבת הדואר.
          </p>
          <p className="muted small">הודעה אחרונה התקבלה: {when(status.last_received)}</p>
          {!status.ai_configured && <div className="note error">חסר GEMINI_API_KEY בשרת, ולכן הודעות ממתינות בלי להיקרא.</div>}
          {status.last_error && <div className="note error">{status.last_error}</div>}
          <div className="actions" style={{ justifyContent: "flex-start" }}>
            <button className="btn primary" onClick={() => void openEmailInbox().catch(() => alert("טעינת ההצעות נכשלה"))}>
              {status.pending ? `הצעות לבדיקה (${status.pending})` : "הצעות לבדיקה"}
            </button>
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
