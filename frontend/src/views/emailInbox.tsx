import { type ReactElement, useState } from "react";
import { api, formatMinor, type EmailRule, type EmailSuggestion } from "../api";
import { BackButton } from "../components/BackButton";
import { openReactDialog } from "../components/openReactDialog";
import { Money } from "../components/Money";
import { openTransactionDialog } from "../dialogs/transaction";
import { go, goBack } from "../nav";
import { render } from "../render";
import { bizName, businessOf, businessRequired, categoryBusiness } from "../business";
import { BusinessChips } from "../components/BusinessChips";
import { type CategoryChoice, CategoryPicker } from "../components/CategoryPicker";
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

/** The subject as the server matches it: no Fwd:/Re: prefixes, case or extra spaces. */
const normSubject = (t: string) => {
  let s = t.trim();
  for (let m = /^(fwd|fw|re|הועבר|תגובה):\s*/i.exec(s); m; m = /^(fwd|fw|re|הועבר|תגובה):\s*/i.exec(s)) s = s.slice(m[0].length);
  return s.split(/\s+/).join(" ").toLowerCase();
};

const UNREAD_PREFIX = "attachment not read by the AI: ";
const CONFIDENCE: Record<string, string> = { high: "ביטחון גבוה", medium: "ביטחון בינוני", low: "ביטחון נמוך" };

/** Transactions found in emails, waiting for a person to check and add them (or throw them away). */
export function emailInboxView(): ReactElement {
  const auto = state.suggestions.filter((s) => s.status === "auto" || s.status === "added");
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
      <p className="muted small" style={{ margin: "10px 4px 0" }}>תנועות שה-AI בטוח בהן, או שיש להן כלל קבוע, נוספות מיד; כל השאר ממתין לאישורכם.</p>
      {auto.length === 0 && pending.length === 0 && queued.length === 0 && failed.length === 0 && <div className="empty">אין הצעות ממתינות</div>}
      {auto.map((s) => (
        <AutoCard key={s.id} s={s} />
      ))}
      {pending.map((s) => (
        <SuggestionCard key={s.id} s={s} />
      ))}
      {queued.map((s) => (
        <QueuedCard key={s.id} s={s} />
      ))}
      {failed.map((s) => (
        <FailedCard key={s.id} s={s} />
      ))}
      <RulesCard />
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

/** Reload the list. Nothing here changes a transaction, so the rest of the app is left alone. */
async function refresh() {
  state.suggestions = await fetchInbox();
  render();
}

/** A button that shows a spinning ring in place of its label while its action runs. */
function Btn({ busy, className = "btn", onClick, children }: { busy: boolean; className?: string; onClick: () => void; children: string }) {
  return (
    <button className={`${className}${busy ? " busy" : ""}`} disabled={busy} aria-busy={busy} onClick={onClick}>
      <span className="lbl">{children}</span>
      {busy && <span className="sync-ring" role="status" aria-label="בביצוע" />}
    </button>
  );
}

/** Make a standing rule for mail with this subject, starting from what the transaction got. */
function RuleForm({ s, close }: { s: EmailSuggestion; close: () => void }) {
  const expenseCats = state.cats.filter((c) => !c.is_income);
  const start = s.category_id != null ? catById(s.category_id) : undefined;
  const [choice, setChoice] = useState<CategoryChoice>(start ? { top: String(start.parent_id ?? start.id), sub: start.parent_id != null ? String(start.id) : "" } : { top: "", sub: "" });
  const own = s.transaction_id != null ? (state.biz.tx.get(s.transaction_id) ?? null) : null;
  const [biz, setBiz] = useState<number | null | undefined>(own);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const cat = choice.sub ? Number(choice.sub) : null;
  const given = categoryBusiness(state.biz, state.cats, cat);
  const required = businessRequired(state.biz, state.cats, cat);
  const save = async () => {
    if (cat == null) return setErr("יש לבחור קטגוריה ותת-קטגוריה");
    if (required && biz === undefined) return setErr("יש לבחור עסק");
    setBusy(true);
    try {
      await api.putEmailRule({ subject: s.subject, category_id: cat, business_id: given != null ? null : (biz ?? null) }, s.transaction_id ?? undefined);
      close();
      // The transaction itself changed too.
      await load();
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };
  return (
    <form
      className="add"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      {/* The close button comes first, so it (not the category field) takes the focus when the dialog opens. */}
      <div className="head">
        <div className="name grow" dir="auto">
          כלל קבוע לנושא <bdi>«{s.subject}»</bdi>
        </div>
        <button type="button" className="btn" onClick={close}>
          סגירה
        </button>
      </div>
      <p className="muted small">מיילים עם הנושא הזה יתווספו מיד בקטגוריה ובעסק שתבחרו. ה-AI יקרא רק את הסכום והתאריך. גם התנועה הזו תעבור לקטגוריה ולעסק שבחרתם.</p>
      <CategoryPicker cats={expenseCats} value={choice} onChange={setChoice} />
      <BusinessChips biz={state.biz} value={biz} onChange={setBiz} required={required} givenName={given != null ? bizName(state.biz, given) : null} collapsed />
      <div className="error">{err}</div>
      <div className="actions">
        <button type="button" className="btn" onClick={close}>
          ביטול
        </button>
        <button type="submit" className="btn primary" disabled={busy}>
          שמירת הכלל
        </button>
      </div>
    </form>
  );
}

/** A transaction made from an email (added at once, or approved). It stays listed for a month; it can be edited like any other. */
function AutoCard({ s }: { s: EmailSuggestion }) {
  const tx = state.txs.find((t) => t.id === s.transaction_id);
  const path = categoryPath(tx ? tx.category_id : s.category_id);
  const biz = tx ? businessOf(state.biz, state.cats, tx) : undefined;
  return (
    <div className="card suggestion auto">
      <div className="head">
        <div className="grow">
          <div className="name">{s.note || s.subject || "תנועה מדוא״ל"}</div>
          <div className="muted small">
            {fmtDate(s.date)}
            {path ? ` · ${path}` : ""}
            {biz != null ? ` · ${bizName(state.biz, biz)}` : ""}
          </div>
        </div>
        <Amount s={s} />
      </div>
      <div className="muted small">נוספה מדוא״ל לתנועות · {s.status === "auto" ? "בלי בדיקה" : "אושרה על ידכם"} · הודעה מ-{fmtDate(s.received)}</div>
      <div className="actions" style={{ justifyContent: "flex-start", flexWrap: "wrap" }}>
        <ViewButton s={s} />
        {tx && (
          <button className="btn" onClick={() => openTransactionDialog(tx)}>
            עריכה
          </button>
        )}
        {s.subject && !state.emailRules.some((r) => normSubject(r.subject) === normSubject(s.subject)) && (
          <button className="btn" onClick={() => openReactDialog((close) => <RuleForm s={s} close={close} />)}>
            כלל קבוע לנושא
          </button>
        )}
      </div>
    </div>
  );
}

function RuleRow({ r }: { r: EmailRule }) {
  const [busy, setBusy] = useState(false);
  return (
    <div className="head" style={{ marginTop: 8 }}>
      <div className="grow small" dir="auto">
        <b>{r.subject}</b>
        <div className="muted">
          {categoryPath(r.category_id)}
          {r.business_id != null ? ` · ${bizName(state.biz, r.business_id)}` : ""}
        </div>
      </div>
      <Btn
        busy={busy}
        onClick={async () => {
          setBusy(true);
          await api.deleteEmailRule(r.subject).catch(() => {});
          await refresh();
        }}
      >
        הסרה
      </Btn>
    </div>
  );
}

/** The standing rules ("always file mail with this subject like this") and a way to remove them. */
function RulesCard() {
  if (state.emailRules.length === 0) return null;
  return (
    <div className="card">
      <div className="name">כללים קבועים לפי נושא</div>
      <p className="muted small">מייל עם נושא כזה נשמר מיד בקטגוריה ובעסק שנבחרו; ה-AI קורא רק את הסכום והתאריך.</p>
      {state.emailRules.map((r) => (
        <RuleRow key={r.subject} r={r} />
      ))}
    </div>
  );
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
        <Btn busy={busy} onClick={reject}>
          דחייה
        </Btn>
        <button
          className="btn primary"
          disabled={busy}
          onClick={() => openTransactionDialog(undefined, s.amount_minor > 0, { suggestionId: s.id, subject: s.subject, date: s.date, amount_minor: s.amount_minor, currency: s.currency, category_id: s.category_id, note: s.note })}
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
        <Btn
          busy={busy}
          onClick={async () => {
            setBusy(true);
            await api.discardSuggestion(s.id).catch(() => {});
            await refresh();
          }}
        >
          הסרה
        </Btn>
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
        <Btn busy={busy} onClick={act(() => api.discardSuggestion(s.id))}>
          הסרה
        </Btn>
        <Btn busy={busy} className="btn primary" onClick={act(async () => {
          await api.emailRetry(s.id);
          await load();
        })}>
          קריאה עכשיו
        </Btn>
      </div>
    </div>
  );
}

export async function openEmailInbox() {
  state.suggestions = await fetchInbox();
  go("emailInbox");
  render();
}
