import { type Category } from "../api";
import { catPath } from "../selectors";
import { Combo } from "./Combo";
import { Field } from "./Field";

export interface CategoryChoice {
  /** The chosen top-level category id, as text ("" for none). */
  top: string;
  /** The chosen sub-category id, as text ("" for none). */
  sub: string;
}

interface Props {
  /** The categories that may be picked (already limited to one kind). */
  cats: Category[];
  value: CategoryChoice;
  onChange: (value: CategoryChoice) => void;
  /** Top-level categories to offer; defaults to every top-level category in `cats`. */
  tops?: Category[];
}

/**
 * The category and sub-category pickers used by the transaction form and the delete-category dialog.
 * The sub-category list follows the category: just its sub-categories once one is picked, otherwise every
 * sub-category (named with its parent). It is only locked for a category that has none. Picking a
 * sub-category first fills in its category.
 */
export function CategoryPicker({ cats, value, onChange, tops }: Props) {
  const topItems = (tops ?? cats.filter((c) => c.parent_id == null)).map((c) => ({ value: String(c.id), label: c.name }));
  const kids = value.top ? cats.filter((k) => String(k.parent_id) === value.top) : cats.filter((k) => k.parent_id != null);
  const subItems = kids.map((k) => ({ value: String(k.id), label: value.top ? k.name : catPath(k.id) }));
  const noSubs = value.top !== "" && kids.length === 0;
  return (
    <>
      <Field label="קטגוריה">
        <Combo items={topItems} value={value.top} placeholder="חיפוש קטגוריה…" onChange={(top) => onChange({ top, sub: "" })} />
      </Field>
      <Field label="תת-קטגוריה">
        <Combo
          items={subItems}
          value={value.sub}
          disabled={noSubs}
          placeholder={noSubs ? "אין תתי-קטגוריות" : "חיפוש תת-קטגוריה…"}
          onChange={(sub) => {
            const parent = cats.find((c) => String(c.id) === sub)?.parent_id;
            onChange({ top: parent != null ? String(parent) : value.top, sub });
          }}
        />
      </Field>
    </>
  );
}
