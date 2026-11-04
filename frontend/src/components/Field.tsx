import { type ReactNode } from "react";

/** A labelled form field. */
export function Field({ label, className = "", children }: { label: ReactNode; className?: string; children: ReactNode }) {
  return (
    <div className={`field ${className}`.trim()}>
      <span>{label}</span>
      {children}
    </div>
  );
}
