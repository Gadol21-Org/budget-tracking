/** The button at the top of a sub-screen that goes back to its parent. */
export function BackButton({ label = "→ הגדרות", title = "חזרה להגדרות", onClick }: { label?: string; title?: string; onClick: () => void }) {
  return (
    <button className="btn" title={title} onClick={onClick}>
      {label}
    </button>
  );
}
