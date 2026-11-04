import { type ReactNode, useState } from "react";

interface Props {
  title?: string;
  submitLabel: string;
  /** Keeps the submit button greyed out (it is also disabled while the submit is running). */
  submitDisabled?: boolean;
  /** Runs on submit; an error thrown from it is shown in the form and the form stays open. */
  onSubmit: () => Promise<void>;
  close: () => void;
  children?: ReactNode;
}

/** The shell shared by the small dialogs: title, fields, error line and Cancel / submit buttons. */
export function DialogForm({ title, submitLabel, submitDisabled = false, onSubmit, close, children }: Props) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="add"
      method="dialog"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          setBusy(true);
          await onSubmit();
        } catch (e2) {
          setError(e2 instanceof Error ? e2.message : String(e2));
          setBusy(false);
        }
      }}
    >
      {title && <h2>{title}</h2>}
      {children}
      <div className="error">{error}</div>
      <div className="actions">
        <button type="button" className="btn" onClick={close}>
          ביטול
        </button>
        <button type="submit" className="btn primary" disabled={busy || submitDisabled}>
          {submitLabel}
        </button>
      </div>
    </form>
  );
}
