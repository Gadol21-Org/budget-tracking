import { type ReactNode } from "react";

interface RowProps {
  /** A gradient icon at the start of the row; rows in a sub-list go without one. */
  icon?: string;
  color?: [string, string];
  label: ReactNode;
  /** The current value, shown before the chevron. */
  value?: string;
  /** Extra content (tags) before the chevron. */
  tags?: ReactNode;
  /** Tapping the row. Without it the row is plain text (a notice). */
  onClick?: () => void;
  /** A button that does something instead of opening something: blue, no chevron. */
  action?: boolean;
  disabled?: boolean;
  /** A label of more than one line: the row grows and keeps vertical padding. */
  multiline?: boolean;
}

/**
 * One row of a grouped list (iOS Settings style), 48px high at least: an optional gradient icon, the label, the current value
 * and tags, and a chevron that always points left (forward in a right-to-left page), drawn as a shape so no font or text direction can flip it. Every row of a settings-style list is one of these so they all look alike.
 */
export function SettingsRow({ icon, color, label, value, tags, onClick, action, disabled, multiline }: RowProps) {
  const body = (
    <>
      {icon && (
        <span className="sicon" style={{ background: `linear-gradient(180deg, ${color?.[0] ?? "#b4b4ba"}, ${color?.[1] ?? "#7c7c82"})` }} aria-hidden="true">
          {icon}
        </span>
      )}
      <span className="slabel">{label}</span>
      {tags}
      {value && <span className="sval">{value}</span>}
      {onClick && !action && (
        <svg className="schev" viewBox="0 0 8 14" width="8" height="14" aria-hidden="true">
          <path d="M7 1 1 7l6 6" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      )}
    </>
  );
  const cls = `srow${icon ? "" : " noicon"}${action ? " sbtn" : ""}${multiline ? " multi" : ""}`;
  if (!onClick) return <div className={cls}>{body}</div>;
  return (
    <button className={cls} disabled={disabled} onClick={onClick}>
      {body}
    </button>
  );
}
