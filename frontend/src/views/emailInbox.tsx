import { type ReactElement, useState } from "react";
import { api, formatMinor, type EmailSuggestion } from "../api";
import { BackButton } from "../components/BackButton";
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

const CONFIDENCE: Record<string, string> = { high: "ביטחון גבוה", medium: "ביטחון בינוני", low: "ביטחון נמוך" };

/** Transactions found in emails, waiting for a person to check and add them (or throw them away). */
export function emailInboxView(): ReactElement {
  const pending = state.suggestions.filter((s) => s.status === "pending");
  const failed = state.suggestions.filter((s) => s.status === "error");
  return (
    <div>
      <div className="card filterbar">
        <BackButton onClick={() => goBack("settings")} />
        <div className="grow">
          <div className="name">הצעות מדוא״ל</div>
          <div className="muted">נמצאו בהודעות עם התווית</div>
        </div>
      </div>
      <p className="muted small" style={{ margin: "10px 4px 0" }}>שום דבר לא נוסף לתנועות עד שתאשרו.</p>
      {pending.length === 0 && failed.length === 0 && <div className="empty">אין הצעות ממתינות</div>}
      {pending.map((s) => (
        <SuggestionCard key={s.id} s={s} />
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

function Source({ s }: { s: EmailSuggestion }) {
  return (
    <details className="email-src">
      <summary className="muted small">מה כתוב במייל</summary>
      <div className="small" dir="auto">
        <div>מאת: {s.sender}</div>
        <div>נושא: {s.subject}</div>
        {s.excerpt && <p className="excerpt">{s.excerpt}</p>}
      </div>
    </details>
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
      <Source s={s} />
      {err && <div className="error">{err}</div>}
      <div className="actions" style={{ justifyContent: "flex-start" }}>
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
  return (
    <div className="card suggestion failed">
      <div className="name">לא הצלחתי לקרוא את ההודעה</div>
      <div className="muted small">{s.subject || s.sender}</div>
      <Source s={s} />
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <button
          className="btn"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            await api.rejectSuggestion(s.id).catch(() => {});
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

export async function openEmailInbox() {
  state.suggestions = await fetchInbox();
  go("emailInbox");
  render();
}
