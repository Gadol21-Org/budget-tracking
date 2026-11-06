import { type ReactElement, useState } from "react";
import { api, formatMinor, type EmailSuggestion } from "../api";
import { BackButton } from "../components/BackButton";
import { openReactDialog } from "../components/openReactDialog";
import { Money } from "../components/Money";
import { openTransactionDialog } from "../dialogs/transaction";
import { go, goBack } from "../nav";
import { render } from "../render";
import { catById } from "../selectors";
import { state } from "../state";
import { fetchInbox, load } from "../sync";
import { fmtDate } from "../ui";

/** "Parent › Child" of a category, or empty. */
function categoryPath(id: number | null): string {
  const c = id != null ? catById(id) : undefined;
  if (!c) return "";
  const parent = c.parent_id != null ? catById(c.parent_id) : undefined;
  return parent ? `${parent.name} › ${c.name}` : c.name;
}

const UNREAD_PREFIX = "attachment not read by the AI: ";
const CONFIDENCE: Record<string, string> = { high: "ביטחון גבוה", medium: "ביטחון בינוני", low: "ביטחון נמוך" };

/** Transactions found in emails, waiting for a person to check and add them (or throw them away). */
export function emailInboxView(): ReactElement {
  const pending = state.suggestions.filter((s) => s.status === "pending");
  const queued = state.suggestions.filter((s) => s.status === "queued");
  const failed = state.suggestions.filter((s) => s.status === "error");
  return (
    <div>
      <div className="card filterbar">
        <BackButton onClick={() => goBack("settings")} />
        <div className="grow">
          <div className="name">הצעות מדוא״ל</div>
          <div className="muted">נמצאו בהודעות שהועברו מ-Gmail</div>
        </div>
      </div>
      <p className="muted small" style={{ margin: "10px 4px 0" }}>שום דבר לא נוסף לתנועות עד שתאשרו.</p>
      {pending.length === 0 && queued.length === 0 && failed.length === 0 && <div className="empty">אין הצעות ממתינות</div>}
      {pending.map((s) => (
        <SuggestionCard key={s.id} s={s} />
      ))}
      {queued.map((s) => (
        <QueuedCard key={s.id} s={s} />
      ))}
      {failed.map((s) => (
        <FailedCard key={s.id} s={s} />
      ))}
    </div>
  );
}

function Amount({ s }: { s: EmailSuggestion }) {
  return s.currency === "ILS" ? <Money agorot={s.amount_minor} cls={s.amount_minor < 0 ? "expense" : "income"} /> : <b dir="ltr" className={s.amount_minor < 0 ? "expense" : "income"}>{formatMinor(s.amount_minor, s.currency)}</b>;
}

/** The email as it was sent, in a frame without scripts (images from the internet are shown). */
function openEmailView(s: EmailSuggestion) {
  openReactDialog(
    (close) => (
      <div className="email-view">
        <div className="head">
          <div className="grow small" dir="auto">
            <b>{s.subject || s.sender}</b>
            <div className="muted">{s.sender}</div>
          </div>
          <button className="btn" onClick={close}>
            סגירה
          </button>
        </div>
        <iframe title="תצוגת המייל" sandbox="" src={`/api/email/suggestions/${s.id}/view`} />
      </div>
    ),
    "fullpage",
  );
}

function ViewButton({ s }: { s: EmailSuggestion }) {
  if (!s.has_html) return null;
  return (
    <button className="btn" onClick={() => openEmailView(s)}>
      צפייה במייל
    </button>
  );
}

async function refresh() {
  state.suggestions = await fetchInbox();
  await load();
}

function SuggestionCard({ s }: { s: EmailSuggestion }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const reject = async () => {
    setBusy(true);
    try {
      await api.rejectSuggestion(s.id);
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };
  const path = categoryPath(s.category_id);
  return (
    <div className="card suggestion">
      <div className="head">
        <div className="grow">
          <div className="name">{s.note || s.subject || "תנועה מדוא״ל"}</div>
          <div className="muted small">
            {fmtDate(s.date)}
            {path ? ` · ${path}` : " · בלי קטגוריה, תצטרכו לבחור"}
          </div>
        </div>
        <Amount s={s} />
      </div>
      <div className="muted small">
        הודעה מ-{fmtDate(s.received)}
        {s.confidence ? ` · ${CONFIDENCE[s.confidence] ?? ""}` : ""}
      </div>
      {err && <div className="error">{err}</div>}
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <ViewButton s={s} />
        <button className="btn" disabled={busy} onClick={reject}>
          דחייה
        </button>
        <button
          className="btn primary"
          disabled={busy}
          onClick={() => openTransactionDialog(undefined, s.amount_minor > 0, { suggestionId: s.id, date: s.date, amount_minor: s.amount_minor, currency: s.currency, category_id: s.category_id, note: s.note })}
        >
          בדיקה והוספה
        </button>
      </div>
    </div>
  );
}

function FailedCard({ s }: { s: EmailSuggestion }) {
  const [busy, setBusy] = useState(false);
  // The server says which attachments it could not read: "attachment not read by the AI: a.jpg, b.png".
  const attachments = s.error.startsWith(UNREAD_PREFIX) ? s.error.slice(UNREAD_PREFIX.length) : "";
  return (
    <div className="card suggestion failed">
      <div className="name">לא הצלחתי לקרוא את ההודעה</div>
      <div className="muted small">{s.subject || s.sender}</div>
      {attachments && (
        <div className="muted small" dir="auto">
          {`קובץ מצורף שה-AI לא קורא (${attachments}), ייתכן שהתנועה נמצאת בו`}
        </div>
      )}
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <ViewButton s={s} />
        <button
          className="btn"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            await api.discardSuggestion(s.id).catch(() => {});
            await refresh();
          }}
        >
          הסרה
        </button>
        <button className="btn" onClick={() => openTransactionDialog()}>
          הוספה ידנית
        </button>
      </div>
    </div>
  );
}

/** A message the AI has not read yet (the service was busy, or the daily limit was reached). */
function QueuedCard({ s }: { s: EmailSuggestion }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const act = (work: () => Promise<unknown>) => async () => {
    setBusy(true);
    try {
      await work();
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };
  return (
    <div className="card suggestion queued">
      <div className="name">ההודעה עדיין לא נקראה</div>
      <div className="muted small">{s.subject || s.sender}</div>
      <div className="muted small">{s.error}</div>
      {err && <div className="error">{err}</div>}
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <ViewButton s={s} />
        <button className="btn" disabled={busy} onClick={act(() => api.discardSuggestion(s.id))}>
          הסרה
        </button>
        <button className="btn primary" disabled={busy} onClick={act(() => api.emailRetry(s.id))}>
          קריאה עכשיו
        </button>
      </div>
    </div>
  );
}

export async function openEmailInbox() {
  state.suggestions = await fetchInbox();
  go("emailInbox");
  render();
}
