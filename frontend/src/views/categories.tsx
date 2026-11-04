import { type ReactElement, useEffect, useRef, useState } from "react";
import { api, type Category } from "../api";
import { bizName } from "../business";
import { openCategoryBusinessDialog } from "./business";
import { BackButton } from "../components/BackButton";
import { DialogForm } from "../components/DialogForm";
import { openReactDialog } from "../components/openReactDialog";
import { goBack, manageCtx } from "../nav";
import { render } from "../render";
import { state } from "../state";
import { load, loadHistory } from "../sync";
import { DeleteCategoryForm } from "./DeleteCategoryForm";

/** Paamonim top-level categories of a kind: what a top-level category made here can stand in for. */
export const paamonimTops = (isIncome: boolean) => state.cats.filter((c) => c.parent_id == null && c.from_paamonim && c.is_income === isIncome);

/** The Paamonim top-level category a top-level category's lines go under, if any. */
export const paamonimHome = (c: Category): number | null => (c.parent_id != null ? null : c.from_paamonim ? c.id : c.linked_to);

function PaamonimSelect({ isIncome, value, onChange, required }: { isIncome: boolean; value: string; onChange: (v: string) => void; required: boolean }) {
  return (
    <select value={value} required={required} onChange={(e) => onChange(e.target.value)}>
      <option value="">בחירת קטגוריה בפעמונים…</option>
      {paamonimTops(isIncome).map((c) => (
        <option key={c.id} value={String(c.id)}>
          {c.name}
        </option>
      ))}
    </select>
  );
}

const LOCAL_ONLY = "קטגוריה שנוספה כאן קיימת רק באפליקציה: תנועות עליה לא יועתקו לפעמונים.";

function AddCategoryForm({ isIncome, parentId, close }: { isIncome: boolean; parentId: number | null; close: () => void }) {
  const [name, setName] = useState("");
  const [parent, setParent] = useState(parentId != null ? String(parentId) : "");
  const [link, setLink] = useState("");
  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => void setTimeout(() => nameRef.current?.focus()), []);
  // Paamonim has no way to add a top-level category, so a new one stands in for one there.
  const hasTops = paamonimTops(isIncome).length > 0;
  const top = !parent;
  const p = state.cats.find((c) => String(c.id) === parent);
  const note = top
    ? hasTops
      ? "פעמונים לא מאפשר קטגוריות ראשיות חדשות. תתי-הקטגוריות של קטגוריה זו יירשמו בפעמונים תחת הקטגוריה שנבחרה."
      : LOCAL_ONLY
    : p && paamonimHome(p) != null
      ? "תת-הקטגוריה תירשם בפעמונים כסעיף באותו שם (אם כבר יש סעיף בשם זה, הוא ישמש)."
      : LOCAL_ONLY;
  return (
    <DialogForm
      title={isIncome ? "הוספת קטגוריית הכנסה" : "הוספת קטגוריית הוצאה"}
      submitLabel="הוספה"
      // Greyed out until there is a name and, for a top-level category, its Paamonim category.
      submitDisabled={!name.trim() || (top && hasTops && !link)}
      close={close}
      onSubmit={async () => {
        await api.addCategory({ name, is_income: isIncome, parent_id: top ? null : Number(parent), linked_to: top && link ? Number(link) : null });
        close();
        await load();
      }}
    >
      <label>
        שם
        <input ref={nameRef} type="text" required maxLength={100} autoComplete="off" value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label>
        סוג
        <select value={parent} onChange={(e) => setParent(e.target.value)}>
          <option value="">קטגוריה ראשית</option>
          {state.cats
            .filter((c) => c.parent_id == null && c.is_income === isIncome)
            .map((c) => (
              <option key={c.id} value={String(c.id)}>{`תת-קטגוריה של ${c.name}`}</option>
            ))}
        </select>
      </label>
      {top && hasTops && (
        <label>
          קטגוריה בפעמונים
          <PaamonimSelect isIncome={isIncome} value={link} onChange={setLink} required />
        </label>
      )}
      <div className="note muted">{note}</div>
    </DialogForm>
  );
}

export const openAddCategoryDialog = (isIncome: boolean, parentId: number | null) => openReactDialog((close) => <AddCategoryForm isIncome={isIncome} parentId={parentId} close={close} />);

/** Choose which Paamonim category a top-level category made here stands in for. */
function LinkCategoryForm({ cat, close }: { cat: Category; close: () => void }) {
  const [link, setLink] = useState(cat.linked_to != null ? String(cat.linked_to) : "");
  return (
    <DialogForm
      title={`"${cat.name}" בפעמונים`}
      submitLabel="שמירה"
      submitDisabled={!link || link === String(cat.linked_to)}
      close={close}
      onSubmit={async () => {
        await api.linkCategory(cat.id, Number(link));
        close();
        await load();
      }}
    >
      <label>
        קטגוריה בפעמונים
        <PaamonimSelect isIncome={cat.is_income} value={link} onChange={setLink} required />
      </label>
      <div className="note muted">תתי-הקטגוריות של קטגוריה זו יירשמו בפעמונים תחת הקטגוריה שנבחרה.</div>
    </DialogForm>
  );
}

export const openLinkCategoryDialog = (cat: Category) => openReactDialog((close) => <LinkCategoryForm cat={cat} close={close} />);

/** Move a sub-category, with its transactions and repeating rules, under another top-level category. */
function MoveCategoryForm({ cat, close }: { cat: Category; close: () => void }) {
  const [target, setTarget] = useState("");
  const from = state.cats.find((c) => c.id === cat.parent_id);
  const home = from ? paamonimHome(from) : null;
  // An item in Paamonim stays where it is there, so it can only move within its Paamonim category.
  const targets = state.cats.filter((c) => c.parent_id == null && c.is_income === cat.is_income && c.id !== cat.parent_id && (!cat.from_paamonim || paamonimHome(c) === home));
  const lines = state.txs.filter((t) => t.category_id === cat.id && !t.upcoming).length;
  return (
    <DialogForm
      title={`העברת "${cat.name}"`}
      submitLabel="העברה"
      submitDisabled={!target}
      close={close}
      onSubmit={async () => {
        await api.moveCategory(cat.id, Number(target));
        close();
        await load();
      }}
    >
      <p className="muted">{`כרגע תחת "${from?.name ?? ""}". ${lines} התנועות שלה והתנועות החוזרות עוברות איתה.`}</p>
      {targets.length ? (
        <label>
          העברה אל
          <select value={target} required onChange={(e) => setTarget(e.target.value)}>
            <option value="">בחירת קטגוריה…</option>
            {targets.map((c) => (
              <option key={c.id} value={String(c.id)}>
                {c.name}
              </option>
            ))}
          </select>
        </label>
      ) : (
        <p className="muted">אין קטגוריה ראשית אחרת שאפשר להעביר אליה.</p>
      )}
      {cat.from_paamonim && <div className="note muted">זהו סעיף בפעמונים, ולכן אפשר להעביר אותו רק לקטגוריות המקושרות לאותה קטגוריה בפעמונים. בפעמונים דבר לא משתנה.</div>}
    </DialogForm>
  );
}

export const openMoveCategoryDialog = (cat: Category) => openReactDialog((close) => <MoveCategoryForm cat={cat} close={close} />);

export const openDeleteCategoryDialog = (cat: Category) => openReactDialog((close) => <DeleteCategoryForm cat={cat} close={close} />);

export function categoriesView(): ReactElement {
  return (
    <div>
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <BackButton onClick={() => goBack("settings")} />
      </div>
      <h1>קטגוריות</h1>
      <CategoriesCard />
    </div>
  );
}

function IconButton({ text, title, onClick }: { text: string; title: string; onClick: () => void }) {
  return (
    <button type="button" className="btn" title={title} onClick={onClick}>
      {text}
    </button>
  );
}

function CategoryRow({ c, sub }: { c: Category; sub: boolean }) {
  const linked = state.cats.find((x) => x.id === c.linked_to)?.name;
  return (
    <div className={sub ? "row sub" : "row"}>
      <div className="grow">
        <div className="name">{c.name}</div>
        {!sub && c.linked_to != null && <div className="muted">{`בפעמונים: ${linked ?? ""}`}</div>}
        {!sub && !c.from_paamonim && c.linked_to == null && <div className="muted">לא מקושרת לפעמונים</div>}
        {state.biz.cat.has(c.id) ? (
          <div>
            <span className="bizchip">{`עסק: ${bizName(state.biz, state.biz.cat.get(c.id))}`}</span>
          </div>
        ) : null}
      </div>
      {state.biz.businesses.length > 0 && <IconButton text="💼" title="עסק של הקטגוריה" onClick={() => openCategoryBusinessDialog(manageCtx(), c)} />}
      {!sub && !c.from_paamonim && <IconButton text="🔗" title="קטגוריה בפעמונים" onClick={() => openLinkCategoryDialog(c)} />}
      {!sub && <IconButton text="+" title="הוספת תת-קטגוריה" onClick={() => openAddCategoryDialog(c.is_income, c.id)} />}
      {sub && <IconButton text="⇄" title="העברה לקטגוריה אחרת" onClick={async () => (await loadHistory(""), openMoveCategoryDialog(c))} />}
      <IconButton text="🗑" title="מחיקה" onClick={async () => (await loadHistory(""), openDeleteCategoryDialog(c))} />
    </div>
  );
}

function CategoriesCard() {
  const kinds = [
    [false, "הוצאות"],
    [true, "הכנסות"],
  ] as const;
  const ofKind = state.cats.filter((c) => c.is_income === state.catKind);
  const tops = ofKind.filter((c) => c.parent_id == null);
  return (
    <div className="card">
      <div className="seg">
        {kinds.map(([inc, label]) => (
          <button key={label} type="button" aria-pressed={state.catKind === inc} onClick={() => ((state.catKind = inc), render())}>
            {label}
          </button>
        ))}
      </div>
      <div className="rows">
        {tops.length ? (
          tops.flatMap((c) => [
            <CategoryRow key={c.id} c={c} sub={false} />,
            ...ofKind.filter((k) => k.parent_id === c.id).map((k) => <CategoryRow key={k.id} c={k} sub />),
          ])
        ) : (
          <div className="empty">אין קטגוריות</div>
        )}
      </div>
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <IconButton text="הוספת קטגוריה" title="" onClick={() => openAddCategoryDialog(state.catKind, null)} />
      </div>
    </div>
  );
}
