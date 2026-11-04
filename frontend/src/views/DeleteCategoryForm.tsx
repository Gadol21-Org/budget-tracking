import { useState } from "react";
import { api, type Category } from "../api";
import { state } from "../state";
import { load } from "../sync";
import { CategoryPicker, type CategoryChoice } from "../components/CategoryPicker";
import { DialogForm } from "../components/DialogForm";

/** Delete a category (and its sub-categories), moving what is on it to another category when anything is. */
export function DeleteCategoryForm({ cat, close }: { cat: Category; close: () => void }) {
  const subs = state.cats.filter((c) => c.parent_id === cat.id);
  const doomed = new Set([cat.id, ...subs.map((c) => c.id)]);
  // Occurrences not yet settled are not in txs, so repeating rules are counted separately.
  const lines = state.txs.filter((t) => t.category_id != null && doomed.has(t.category_id) && !t.upcoming).length;
  const rules = new Set(state.recurring.filter((r) => r.category_id != null && doomed.has(r.category_id)).map((r) => r.series_id)).size;
  const used = lines + rules;
  const avail = state.cats.filter((c) => c.is_income === cat.is_income && !doomed.has(c.id));

  const [choice, setChoice] = useState<CategoryChoice>({ top: "", sub: "" });

  /** The category the transactions move to: the sub-category if one is picked, or the category itself when it has no sub-categories. */
  const hasKids = (top: string) => avail.some((k) => String(k.parent_id) === top);
  const target = choice.sub || (choice.top && !hasKids(choice.top) ? choice.top : "");

  return (
    <DialogForm
      title={`מחיקת "${cat.name}"`}
      submitLabel="מחיקה"
      submitDisabled={used > 0 && !target}
      close={close}
      onSubmit={async () => {
        await api.deleteCategory(cat.id, target ? Number(target) : undefined);
        close();
        await load();
      }}
    >
      {subs.length > 0 && <p className="muted">{`יימחקו גם ${subs.length} תתי-קטגוריות.`}</p>}
      {used > 0 ? (
        <div>
          <p>{`${lines} תנועות ו-${rules} הוצאות חוזרות משויכות לקטגוריה. להעביר אותן אל:`}</p>
          <div>
            <CategoryPicker cats={avail} value={choice} onChange={setChoice} />
          </div>
        </div>
      ) : (
        <p className="muted">אין תנועות משויכות.</p>
      )}
    </DialogForm>
  );
}
