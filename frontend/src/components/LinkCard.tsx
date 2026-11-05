/** A settings card: a title, a line of explanation and one button that opens something. */
export function LinkCard({ title, text, button, onClick }: { title: string; text: string; button: string; onClick: () => void }) {
  return (
    <div className="card">
      <h2>{title}</h2>
      <p className="muted">{text}</p>
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <button className="btn" onClick={onClick}>
          {button}
        </button>
      </div>
    </div>
  );
}
