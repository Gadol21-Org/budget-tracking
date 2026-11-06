import { useState } from "react";
import { api } from "../api";
import { load } from "../sync";
import { openEmailInbox } from "./emailInbox";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString("he-IL", { dateStyle: "short", timeStyle: "short" }) : "עדיין לא");

/** Settings card: connect Gmail, see its state, read the label now, disconnect. Hidden when the server has no email support. */
export function EmailCard({ status }: { status: import("../api").EmailStatus | null }) {
  const [busy, setBusy] = useState(false);
  const [out, setOut] = useState<{ text: string; error: boolean } | null>(null);
  if (!status) return null;
  const run = (work: () => Promise<string>) => async () => {
    setBusy(true);
    setOut({ text: "מבצע…", error: false });
    try {
      setOut({ text: await work(), error: false });
      await load();
    } catch (e) {
      setOut({ text: `שגיאה: ${e instanceof Error ? e.message : e}`, error: true });
    } finally {
      setBusy(false);
    }
  };
  const scan = run(async () => {
    const s = await api.emailSync(7);
    return s.messages === 0 ? "אין הודעות חדשות עם התווית." : `נקראו ${s.messages} הודעות: ${s.suggestions} הצעות, ${s.ignored} בלי תנועה${s.failed ? `, ${s.failed} שלא נקראו` : ""}.`;
  });
  const disconnect = run(async () => {
    if (!confirm("לנתק את Gmail? ההצעות שכבר נמצאו יישארו.")) return "הניתוק בוטל.";
    await api.emailDisconnect();
    return "Gmail נותק.";
  });
  return (
    <div className="card">
      <h2>הצעות תנועות מדוא״ל</h2>
      {!status.configured ? (
        <p className="muted">החיבור ל-Gmail עוד לא הוגדר בשרת (GMAIL_CLIENT_ID, GMAIL_CLIENT_SECRET ו-GMAIL_REDIRECT_URI). ההוראות ב-deploy/GMAIL.md.</p>
      ) : !status.connected ? (
        <>
          <p className="muted">
            מחברים תיבת Gmail, והאפליקציה קוראת רק הודעות שמסומנות בתווית “{status.label}”, ושואלת AI איזו תנועה כל הודעה מתארת. תנועה נוספת רק אחרי שאישרתם אותה.
          </p>
          <p className="muted small">שימו לב: גוגל לא מאפשרת הרשאה לתווית אחת בלבד, אלא קריאה לכל התיבה. לכן מומלץ לחבר תיבה ייעודית שמקבלת רק הודעות שהועברו אליה.</p>
          <div className="actions" style={{ justifyContent: "flex-start" }}>
            <a className="btn primary" href="/api/email/gmail/connect">
              חיבור Gmail
            </a>
          </div>
        </>
      ) : (
        <>
          <p className="muted">
            מחובר: <bdi dir="ltr">{status.email}</bdi> · תווית “{status.label}” · {status.push_enabled ? "עדכון מיידי (Pub/Sub) ובדיקה תקופתית" : "בדיקה יומית"}
          </p>
          <p className="muted small">סנכרון אחרון: {when(status.last_sync)}</p>
          {!status.ai_configured && <div className="note error">חסר GEMINI_API_KEY בשרת, ולכן הודעות לא נקראות.</div>}
          {status.last_error && <div className="note error">{status.last_error}</div>}
          <div className="actions" style={{ justifyContent: "flex-start" }}>
            <button className="btn primary" onClick={() => void openEmailInbox().catch(() => alert("טעינת ההצעות נכשלה"))}>
              {status.pending ? `הצעות לבדיקה (${status.pending})` : "הצעות לבדיקה"}
            </button>
            <button className="btn" disabled={busy} onClick={scan}>
              קריאה עכשיו
            </button>
            <button className="btn" disabled={busy} onClick={disconnect}>
              ניתוק
            </button>
          </div>
        </>
      )}
      <div className={out?.error ? "note error" : "note muted"}>{out?.text}</div>
    </div>
  );
}
