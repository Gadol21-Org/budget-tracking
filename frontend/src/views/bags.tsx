import { type ReactElement, useState } from "react";
import { api, parseAgorot, type Bag } from "../api";
import { bagById, bagFamily, businessRoot, isArchived, isBusinessBag, bagPath, bagTransactions, childBags, dailyAverage, dateRange, netSpent, parentChoices } from "../bags";
import { BackButton } from "../components/BackButton";
import { DateField } from "../components/DateField";
import { DialogForm } from "../components/DialogForm";
import { Field } from "../components/Field";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { openReactDialog } from "../components/openReactDialog";
import { openTransactionDialog } from "../dialogs/transaction";
import { formatShekelsWhole } from "../format";
import { go, goBack } from "../nav";
import { catName, dayTitle } from "../selectors";
import { state } from "../state";
import { load } from "../sync";

const today = () => new Date().toISOString().slice(0, 10);
const bagsOf = () => state.bags;

/** Net spending of a bag and everything under it, over all time. */
const bagSpent = (id: number) => netSpent(bagTransactions(bagsOf(), state.txs, id));

function NoteOverlap() {
  return <div className="note muted">עסקה שנמצאת בכמה טיוגים נספרת פעם אחת בטיוג האב. סכומי טיוגים עשויים לחפוף, ולכן אינם מסתכמים לסך ההוצאות החודשי.</div>;
}

function BudgetBar({ spent, budget }: { spent: number; budget: number }) {
  const pct = Math.max(0, Math.min(100, Math.round((spent / budget) * 100)));
  return (
    <div className={spent > budget ? "bar over" : "bar"} title={`${Math.round((spent / budget) * 100)}% מהתקציב`}>
      <span style={{ width: `${pct}%` }} />
    </div>
  );
}

function BagRow({ bag, depth }: { bag: Bag; depth: number }) {
  const spent = bagSpent(bag.id);
  const kids = childBags(state.bags, bag.id).length;
  const sub = [dateRange(bag), kids ? (kids === 1 ? "תת-טיוג אחד" : `${kids} תתי-טיוגים`) : ""].filter(Boolean).join(" · ");
  return (
    <div className="row bagrow" style={{ paddingInlineStart: depth * 18 }} {...clickableProps(() => go("bag", { id: bag.id }))}>
      <div className="grow">
        <div className="name">{bag.name}</div>
        {sub && <div className="muted small">{sub}</div>}
        {bag.budget_agorot != null && bag.budget_agorot > 0 && (
          <>
            <div className="muted small">{`מתוך ${formatShekelsWhole(bag.budget_agorot)}`}</div>
            <BudgetBar spent={spent} budget={bag.budget_agorot} />
          </>
        )}
      </div>
      <Money agorot={spent} whole cls={spent < 0 ? "income" : ""} />
      <span className="chev muted">‹</span>
    </div>
  );
}

function Tree({ parent, depth, archived }: { parent: number | null; depth: number; archived: boolean }): ReactElement {
  return (
    <>
      {childBags(state.bags, parent)
        .filter((b) => isArchived(state.bags, b.id) === archived)
        .map((b) => (
          <div key={b.id}>
            <BagRow bag={b} depth={depth} />
            <Tree parent={b.id} depth={depth + 1} archived={archived} />
          </div>
        ))}
    </>
  );
}

/** Archived bags as a flat list (an archived bag may sit under an active one). */
function ArchivedList() {
  const list = state.bags.bags.filter((b) => isArchived(state.bags, b.id));
  return (
    <>
      {list.map((b) => (
        <BagRow key={b.id} bag={b} depth={0} />
      ))}
    </>
  );
}

export function bagsView(): ReactElement {
  return <BagsScreen />;
}

function BagsScreen() {
  const [archive, setArchive] = useState(false);
  const active = state.bags.bags.filter((b) => !isArchived(state.bags, b.id));
  const archived = state.bags.bags.filter((b) => isArchived(state.bags, b.id));
  return (
    <div>
      <h1>טיוגים</h1>
      <div className="seg3b bagtabs">
        <button type="button" aria-pressed={!archive} onClick={() => setArchive(false)}>
          פעילים
        </button>
        <button type="button" aria-pressed={archive} onClick={() => setArchive(true)}>
          {`ארכיון${archived.length ? ` (${archived.length})` : ""}`}
        </button>
      </div>
      {!archive && !active.length && (
        <div className="card">
          <h2>קבצו תנועות לפי מטרה</h2>
          <p className="muted">טיוג מקבץ תנועות מכמה קטגוריות, למשל חופשות › יפן 2026. תנועה יכולה להיות בכמה טיוגים, והיא נשארת בקטגוריה שלה.</p>
        </div>
      )}
      {archive && !archived.length && <div className="empty">אין טיוגים בארכיון</div>}
      {(archive ? archived.length > 0 : active.length > 0) && <div className="card rows">{archive ? <ArchivedList /> : <Tree parent={null} depth={0} archived={false} />}</div>}
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <button className="btn" onClick={() => openBagForm()}>
          ＋ טיוג חדש
        </button>
        <button className="btn" onClick={() => openBusinessForm()}>
          ＋ עסק חדש
        </button>
      </div>
      {state.bags.bags.length > 0 && <NoteOverlap />}
    </div>
  );
}

/** One bag: its figures, sub-bags, breakdown by category and lines. */
export function bagView(): ReactElement {
  return <BagScreen />;
}

function BagScreen() {
  const id = state.filter?.id ?? null;
  const bag = bagById(state.bags, id);
  if (!bag) return <div className="empty">הטיוג לא נמצא</div>;
  const txs = bagTransactions(state.bags, state.txs, bag.id).sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : 0));
  const spent = netSpent(txs);
  const avg = dailyAverage(bag, spent, today());
  const kids = childBags(state.bags, bag.id);
  // By top-level category, expenses only (income would cancel out what it refunds).
  const byCat = new Map<number | null, number>();
  for (const t of txs) {
    if (t.amount_agorot >= 0) continue;
    const c = state.cats.find((x) => x.id === t.category_id);
    const top = c?.parent_id ?? t.category_id;
    byCat.set(top, (byCat.get(top) ?? 0) + -t.amount_agorot);
  }
  const rows = [...byCat.entries()].sort((a, b) => b[1] - a[1]);
  const max = rows[0]?.[1] ?? 1;
  const budget = bag.budget_agorot;
  // A business is a bag under the business root; it also has profit and loss figures of its own.
  const isBusiness = bag.parent_id != null && bag.parent_id === businessRoot(state.bags)?.id;
  const days = [...new Set(txs.map((t) => t.date))];
  return (
    <div className="dash">
      <div className="card filterbar">
        <BackButton label="→ טיוגים" title="חזרה לטיוגים" onClick={() => goBack("bags")} />
        <div className="grow">
          <div className="muted small">{bag.parent_id != null ? bagPath(state.bags, bag.parent_id) : "טיוג"}</div>
          <div className="name">{bag.name}</div>
          {dateRange(bag) && <div className="muted small">{dateRange(bag)}</div>}
        </div>
        {bag.role !== "business_root" && (
          <button className="del" title="עריכה" onClick={() => openBagForm(bag)}>
            ✎
          </button>
        )}
      </div>
      {isBusiness && (
        <button className="btn bizlink" onClick={() => ((state.bizView = { tab: "overview", scope: bag.id }), go("business"))}>
          לדוח רווח והפסד של העסק ›
        </button>
      )}
      <div className="kpis">
        <div className="kpi">
          <div className="l">סה״כ</div>
          <div className="v">
            <Money agorot={spent} whole />
          </div>
        </div>
        <div className="kpi">
          <div className="l">{budget != null && budget > 0 ? (spent > budget ? "חריגה מהתקציב" : "תקציב נותר") : "תנועות"}</div>
          <div className="v">{budget != null && budget > 0 ? <Money agorot={Math.abs(budget - spent)} whole cls={spent > budget ? "expense" : ""} /> : txs.length}</div>
        </div>
        {avg != null && (
          <div className="kpi">
            <div className="l">ממוצע ליום</div>
            <div className="v">
              <Money agorot={avg} whole />
            </div>
          </div>
        )}
        {(avg != null || (budget != null && budget > 0)) && (
          <div className="kpi">
            <div className="l">תנועות</div>
            <div className="v">{txs.length}</div>
          </div>
        )}
      </div>
      {budget != null && budget > 0 && (
        <div className="card">
          <BudgetBar spent={spent} budget={budget} />
          <div className="muted small">{`${formatShekelsWhole(spent)} מתוך ${formatShekelsWhole(budget)}`}</div>
        </div>
      )}
      {kids.length > 0 && (
        <>
          <h2>תתי-טיוגים</h2>
          <div className="card rows">
            {kids.map((k) => (
              <BagRow key={k.id} bag={k} depth={0} />
            ))}
          </div>
          <NoteOverlap />
        </>
      )}
      {rows.length > 0 && (
        <>
          <h2>לפי קטגוריה</h2>
          <div className="card rows">
            {rows.map(([cid, v]) => (
              <div className="row" key={String(cid)}>
                <div className="grow">
                  <div className="name">{catName(cid)}</div>
                  <div className="bar">
                    <span style={{ width: `${(v / max) * 100}%` }} />
                  </div>
                </div>
                <Money agorot={v} whole />
              </div>
            ))}
          </div>
        </>
      )}
      <h2>{`תנועות (${txs.length})`}</h2>
      {days.length ? (
        days.map((d) => (
          <div key={d}>
            <div className="daylabel">{dayTitle(d)}</div>
            <div className="card rows">
              {txs
                .filter((t) => t.date === d)
                .map((t) => (
                  <div className="row" key={t.id} {...clickableProps(() => openTransactionDialog(t))}>
                    <div className="grow">
                      <div className="name">{catName(t.category_id)}</div>
                      {t.note && <div className="muted">{t.note}</div>}
                    </div>
                    <Money agorot={t.amount_agorot} cls={t.amount_agorot < 0 ? "expense" : "income"} />
                  </div>
                ))}
            </div>
          </div>
        ))
      ) : (
        <div className="empty">עוד אין תנועות בטיוג. אפשר להוסיף מעריכת תנועה, או לבחור כמה תנועות ברשימה.</div>
      )}
    </div>
  );
}

// ---- Add / edit ---------------------------------------------------------------------------------------

function BagForm({ bag, parentId, close }: { bag?: Bag; parentId: number | null; close: () => void }) {
  const [name, setName] = useState(bag?.name ?? "");
  const [parent, setParent] = useState(bag ? (bag.parent_id != null ? String(bag.parent_id) : "") : parentId != null ? String(parentId) : "");
  const [start, setStart] = useState(bag?.start_date ?? "");
  const [end, setEnd] = useState(bag?.end_date ?? "");
  const [budget, setBudget] = useState(bag?.budget_agorot != null ? String(bag.budget_agorot / 100) : "");
  const [archived, setArchived] = useState(bag?.archived ?? false);
  const businessBag = !!bag && isBusinessBag(state.bags, bag.id);
  const choices = parentChoices(state.bags, bag?.id ?? null);
  const input = () => ({
    name: name.trim(),
    parent_id: parent ? Number(parent) : null,
    archived,
    budget_agorot: budget.trim() ? parseAgorot(budget) : null,
    start_date: start || null,
    end_date: end || null,
  });
  const family = bag ? bagFamily(state.bags, bag.id).size - 1 : 0;
  return (
    <DialogForm
      title={bag ? "עריכת טיוג" : "טיוג חדש"}
      submitLabel={bag ? "עדכון" : "יצירה"}
      submitDisabled={!name.trim()}
      close={close}
      onSubmit={async () => {
        const body = input();
        if (body.start_date && body.end_date && body.end_date < body.start_date) throw new Error("תאריך הסיום קודם להתחלה");
        if (bag) await api.changeBag(bag.id, body);
        else await api.addBag(body);
        close();
        await load();
      }}
    >
      <Field label="שם">
        <input type="text" required maxLength={100} autoComplete="off" value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field label="טיוג אב">
        <select value={parent} disabled={businessBag} onChange={(e) => setParent(e.target.value)}>
          <option value="">ללא (טיוג ראשי)</option>
          {businessBag && <option value={parent}>{bagPath(state.bags, Number(parent))}</option>}
          {choices.map((b) => (
            <option key={b.id} value={String(b.id)}>
              {bagPath(state.bags, b.id)}
            </option>
          ))}
        </select>
      </Field>
      <div className="amountrow">
        <Field label="מתאריך (אופציונלי)">
          <DateField value={start} onChange={setStart} />
        </Field>
        <Field label="עד תאריך (אופציונלי)">
          <DateField value={end} onChange={setEnd} />
        </Field>
      </div>
      <Field label="תקציב ב-₪ (אופציונלי)">
        <input type="text" inputMode="decimal" placeholder="0" autoComplete="off" value={budget} onChange={(e) => setBudget(e.target.value)} />
      </Field>
      {bag && (
        <>
          <label className="archiverow">
            <input type="checkbox" checked={archived} onChange={(e) => setArchived(e.target.checked)} />
            <span>בארכיון: לא מוצע בבחירת טיוג</span>
          </label>
          <button
            type="button"
            className="btn danger"
            onClick={async () => {
              const msg = family ? `למחוק את הטיוג ואת ${family} תתי-הטיוגים שלו? התנועות עצמן לא יימחקו.` : "למחוק את הטיוג? התנועות עצמן לא יימחקו.";
              if (!confirm(msg)) return;
              await api.deleteBag(bag.id);
              close();
              if (state.view === "bag") goBack("bags");
              await load();
            }}
          >
            מחיקת הטיוג
          </button>
        </>
      )}
    </DialogForm>
  );
}

/** A new business is a bag directly under the fixed עסקים bag; one of them may hold the shared costs. */
function BusinessForm({ close }: { close: () => void }) {
  const [name, setName] = useState("");
  const [general, setGeneral] = useState(false);
  const hasGeneral = state.biz.businesses.some((b) => b.is_general);
  return (
    <DialogForm
      title="עסק חדש"
      submitLabel="יצירה"
      submitDisabled={!name.trim()}
      close={close}
      onSubmit={async () => {
        await api.addBusiness(name.trim(), general);
        close();
        await load();
      }}
    >
      <Field label="שם">
        <input type="text" required maxLength={60} autoComplete="off" value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <label className="archiverow">
        <input type="checkbox" disabled={hasGeneral} checked={general} onChange={(e) => setGeneral(e.target.checked)} />
        <span>{hasGeneral ? "כבר יש עסק משותף" : "הוצאות משותפות (לא שייכות לעסק מסוים)"}</span>
      </label>
    </DialogForm>
  );
}

const openBusinessForm = () => openReactDialog((close) => <BusinessForm close={close} />);

export function openBagForm(bag?: Bag, parentId: number | null = null) {
  openReactDialog((close) => <BagForm bag={bag} parentId={parentId} close={close} />);
}
