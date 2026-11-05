import { useState } from "react";
import { api } from "../api";
import { bagPath, businessRoot, childBags, isArchived, isBusinessBag, type BagState } from "../bags";
import { openReactDialog } from "../components/openReactDialog";
import { state } from "../state";
import { load } from "../sync";

interface Props {
  title: string;
  /** The bags ticked when the dialog opens. */
  selected: number[];
  /** Offer only these bags (for taking lines out of the bags they are in); default: every bag that is not archived, and any that is already ticked. */
  only?: number[];
  /** Whether a bag can be made on the spot. */
  canCreate: boolean;
  confirmLabel: string;
  onDone: (ids: number[]) => void | Promise<void>;
  close: () => void;
}

/** Bags in tree order (a bag, then its sub-bags), with how deep each one is. A bag's sub-bags show only while it is open (or while searching). */
function ordered(s: BagState, parent: number | null, depth: number, keep: (id: number) => boolean, open: (id: number) => boolean): { id: number; depth: number }[] {
  return childBags(s, parent)
    .sort((a, b) => Number(b.role === "business_root") - Number(a.role === "business_root")) // the businesses come first
    .filter((b) => keep(b.id) || hasKept(s, b.id, keep))
    .flatMap((b) => [{ id: b.id, depth }, ...(open(b.id) ? ordered(s, b.id, depth + 1, keep, open) : [])]);
}
const hasKept = (s: BagState, id: number, keep: (id: number) => boolean): boolean => childBags(s, id).some((b) => keep(b.id) || hasKept(s, b.id, keep));

/** The ids of every bag above the given ones, so a ticked sub-bag is visible when the list opens. */
function ancestorsOf(s: BagState, ids: Iterable<number>): Set<number> {
  const out = new Set<number>();
  for (const id of ids) for (let cur = s.bags.find((b) => b.id === id)?.parent_id; cur != null && !out.has(cur); cur = s.bags.find((b) => b.id === cur)?.parent_id) out.add(cur);
  return out;
}

interface ChoiceProps {
  picked: Set<number>;
  onToggle: (id: number) => void;
  /** Offer only these bags (for taking lines out of the bags they are in); default: every bag that is not archived, and any that is already ticked. */
  only?: number[];
  /** Whether a bag can be made on the spot. */
  canCreate: boolean;
  /** Offer the businesses too, under the fixed עסקים node: a line is in at most one of them, so ticking one unticks the other. */
  biz?: { value: number | null | undefined; onPick: (id: number | null) => void };
}

/** The searchable tree of bags with a checkbox each, and "create" for a typed name that matches none. Used in a dialog and inside the transaction form. */
export function BagChoice({ picked, onToggle, only, canCreate, biz }: ChoiceProps) {
  const [query, setQuery] = useState("");
  /** Top-level bags start closed, so the list stays short; the ones holding a ticked bag start open. */
  const root = businessRoot(state.bags);
  const [opened, setOpened] = useState(() => {
    const o = ancestorsOf(state.bags, picked);
    if (biz?.value != null && root) o.add(root.id);
    return o;
  });
  const [, setTick] = useState(0);
  const [err, setErr] = useState("");
  const s = state.bags;
  const offered = (id: number) => (biz ? true : !isBusinessBag(s, id)) && (only ? only.includes(id) : !isArchived(s, id) || picked.has(id));
  const q = query.trim().toLowerCase();
  const rows = ordered(s, null, 0, (id) => offered(id) && (!q || bagPath(s, id).toLowerCase().includes(q)), (id) => !!q || opened.has(id));
  const toggleOpen = (id: number) =>
    setOpened((o) => {
      const n = new Set(o);
      if (!n.delete(id)) n.add(id);
      return n;
    });
  const exact = s.bags.some((b) => b.parent_id == null && b.name === query.trim());
  const createBusiness = async () => {
    try {
      setErr("");
      const id = await api.addBusiness(query.trim());
      await load();
      biz?.onPick(id);
      setQuery("");
      setTick((n) => n + 1);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };
  const create = async () => {
    try {
      setErr("");
      const id = await api.addBag({ name: query.trim(), parent_id: null, archived: false, budget_agorot: null, start_date: null, end_date: null });
      await load();
      onToggle(id);
      setQuery("");
      setTick((n) => n + 1);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <div className="bagchoice">
      <input type="search" placeholder="חיפוש טיוג…" autoComplete="off" value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === "Enter" && e.preventDefault()} />
      <div className="bagtree">
        {rows.map(({ id, depth }) => {
          const b = s.bags.find((x) => x.id === id)!;
          const hasKids = childBags(s, id).some((k) => offered(k.id) || hasKept(s, k.id, offered));
          const isOpen = !!q || opened.has(id);
          const isRoot = !!root && id === root.id;
          const isBiz = !!biz && isBusinessBag(s, id);
          return (
            <label key={id} className="bagnode" style={{ paddingInlineStart: 4 + depth * 14 }}>
              {hasKids && !q ? (
                <button type="button" className="bagtoggle" aria-expanded={isOpen} aria-label={isOpen ? `סגירת ${b.name}` : `פתיחת ${b.name}`} onClick={(e) => (e.preventDefault(), toggleOpen(id))}>
                  {isOpen ? "▾" : "◂"}
                </button>
              ) : (
                <span className="bagtoggle" aria-hidden="true" />
              )}
              <input type="checkbox" className="selbox" checked={isBiz ? biz.value === id : picked.has(id)} onChange={() => (isBiz ? biz.onPick(biz.value === id ? null : id) : onToggle(id))} />
              <span className="grow">
                <span className="name">{isBiz ? `💼 ${b.name}` : b.name}{isRoot && biz && <span className="muted small"> · הוצאות משותפות</span>}</span>
                {q && b.parent_id != null && <span className="muted small">{` · ${bagPath(s, id)}`}</span>}
                {isArchived(s, id) && <span className="muted small"> · בארכיון</span>}
              </span>
            </label>
          );
        })}
        {!rows.length && <div className="combo-empty">{s.bags.length ? "אין תוצאות" : "עוד אין טיוגים"}</div>}
      </div>
      {canCreate && query.trim() && !exact && (
        <button type="button" className="btn" onClick={() => void create()}>
          {`＋ יצירת טיוג "${query.trim()}"`}
        </button>
      )}
      {biz && query.trim() && !s.bags.some((b) => isBusinessBag(s, b.id) && b.name === query.trim()) && (
        <button type="button" className="btn" onClick={() => void createBusiness()}>
          {`＋ יצירת עסק "${query.trim()}"`}
        </button>
      )}
      <div className="error">{err}</div>
    </div>
  );
}

interface Props {
  title: string;
  /** The bags ticked when the dialog opens. */
  selected: number[];
  only?: number[];
  canCreate: boolean;
  confirmLabel: string;
  onDone: (ids: number[]) => void | Promise<void>;
  close: () => void;
}

/** A dialog around the choice, for the list's bulk actions. */
function BagPickerForm({ title, selected, only, canCreate, confirmLabel, onDone, close }: Props) {
  const [picked, setPicked] = useState(new Set(selected));
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const toggle = (id: number) =>
    setPicked((p) => {
      const n = new Set(p);
      if (!n.delete(id)) n.add(id);
      return n;
    });
  return (
    <form
      className="add bagpicker"
      method="dialog"
      onSubmit={async (e) => {
        e.preventDefault();
        setBusy(true);
        try {
          await onDone([...picked]);
          close();
        } catch (e2) {
          setErr(e2 instanceof Error ? e2.message : String(e2));
          setBusy(false);
        }
      }}
    >
      <h2>{title}</h2>
      <BagChoice picked={picked} onToggle={toggle} only={only} canCreate={canCreate} />
      <div className="error">{err}</div>
      <div className="actions">
        <button type="button" className="btn" onClick={close}>
          ביטול
        </button>
        <button type="submit" className="btn primary" disabled={busy || !picked.size}>
          {`${confirmLabel}${picked.size ? ` (${picked.size})` : ""}`}
        </button>
      </div>
    </form>
  );
}

export function openBagPicker(props: Omit<Props, "close">) {
  openReactDialog((close) => <BagPickerForm {...props} close={close} />);
}
