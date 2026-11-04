import { fmtDate } from "../ui";

interface Props {
  /** The date as ISO text (yyyy-mm-dd). */
  value: string;
  onChange: (value: string) => void;
  required?: boolean;
}

/** A native date input that shows dd/mm/yyyy whatever the device locale; the value stays ISO and the native picker opens on tap. */
export function DateField({ value, onChange, required }: Props) {
  return (
    <div className="datefield">
      <span className="dateshown">{fmtDate(value) || "dd/mm/yyyy"}</span>
      <input
        type="date"
        className="datenative"
        value={value}
        required={required}
        onChange={(e) => onChange(e.target.value)}
        onClick={(e) => {
          try {
            e.currentTarget.showPicker();
          } catch {
            /* the browser opens its own picker */
          }
        }}
      />
    </div>
  );
}
