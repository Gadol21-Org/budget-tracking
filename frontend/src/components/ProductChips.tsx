import { useState } from "react";
import { type ProductsData } from "../business";

interface Props {
  products: ProductsData;
  /** undefined = nothing chosen yet (only while a choice is required); null = None. */
  value: number | null | undefined;
  onChange: (value: number | null) => void;
  /** A line must say which product it is in: the buttons show with nothing chosen. */
  required: boolean;
  /** The product the category already gives (no choice), or null for the choice. */
  givenName: string | null;
  /** Start as a "+ mark for a product" link instead of the open row. */
  collapsed?: boolean;
}

/** Pick the product a line counts in: None or one of the products, as chips. */
export function ProductChips({ products, value, onChange, required, givenName, collapsed = false }: Props) {
  const [expanded, setExpanded] = useState(!collapsed);
  const showRow = givenName == null && (expanded || required || value != null);
  const options = [{ id: null as number | null, name: "ללא" }, ...products.products.map((b) => ({ id: b.id as number | null, name: b.name }))];
  return (
    <div className="productpick">
      <button type="button" className="linkbtn" hidden={givenName != null || showRow} onClick={() => setExpanded(true)}>
        + סימון לעסק
      </button>
      <div className="muted small" hidden={!(showRow || givenName != null)}>
        {required && givenName == null ? "מוצר (חובה לבחור)" : "מוצר"}
      </div>
      <div className="qchips" hidden={!showRow}>
        {options.map((o) => (
          <button key={String(o.id)} type="button" className="qchip" aria-pressed={o.id === value} onClick={() => onChange(o.id)}>
            {o.name}
          </button>
        ))}
      </div>
      <div hidden={givenName == null}>
        {givenName != null && (
          <>
            <span className="productchip big">{givenName}</span>
            <span className="muted small"> לפי הקטגוריה</span>
          </>
        )}
      </div>
    </div>
  );
}
