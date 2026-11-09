type RowProps = { icon: string; color: [string, string]; label: string; value?: string; onClick: () => void };

/** One row of the grouped list: a gradient icon, the name, the current value and a chevron (like iOS Settings). */
export function SettingsRow({ icon, color, label, value, onClick }: RowProps) {
  return (
    <button className="srow" onClick={onClick}>
      <span className="sicon" style={{ background: `linear-gradient(180deg, ${color[0]}, ${color[1]})` }} aria-hidden="true">
        {icon}
      </span>
      <span className="slabel">{label}</span>
      {value && <span className="sval">{value}</span>}
      <span className="schev" aria-hidden="true">‹</span>
    </button>
  );
}
