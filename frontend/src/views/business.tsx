import { Icon } from "../components/Icon";
import { type ReactElement, type ReactNode, useEffect, useRef, useState } from "react";
import { api, type Product, type Category, type Transaction } from "../api";
import {
  type BusinessCtx,
  type ProductsData,
  type BusinessTab,
  type Fig,
  allocation,
  productOf,
  figures,
  lastMonths,
  scopeName,
  scoped,
} from "../business";
import { ProductChips } from "../components/ProductChips";
import { DialogForm } from "../components/DialogForm";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { openReactDialog } from "../components/openReactDialog";
import { formatShekelsWhole, shortMonth } from "../format";
import { contains, shift, title as periodTitle } from "../period";
import { colorAt } from "../insights/model";
import { ConnectedMonthBar } from "./month";
import { DonutCard, Delta } from "./insights";
import { TransactionRow } from "./list";
import { dayTitle } from "../selectors";

/** The whole business compares its products; one product lists its own transactions instead. */
const tabsFor = (scope: "all" | number): [BusinessTab, string][] => [
  ["overview", "סקירה"],
  ["trends", "מגמות"],
  ["cats", "קטגוריות"],
  scope === "all" ? ["compare", "השוואה"] : ["txs", "תנועות"],
];

const signed = (a: number) => <Money agorot={a} whole cls={a < 0 ? "expense" : "income"} />;
const whole = (a: number, cls = "") => <Money agorot={a} whole cls={cls} />;

/** A table of the four figures per row, with a closing total row. */
function FigureTable({ heads, rows, total }: { heads: string[]; rows: ReactNode; total: ReactNode }) {
  return (
    <div className="card">
      <table className="biztable">
        <tbody>
          <tr>
            {heads.map((h) => (
              <th className="muted" key={h}>
                {h}
              </th>
            ))}
          </tr>
          {rows}
          <tr className="total">{total}</tr>
        </tbody>
      </table>
    </div>
  );
}

const figureCells = (f: Fig) => (
  <>
    <td>{whole(f.income)}</td>
    <td>{whole(f.expense)}</td>
    <td>{signed(f.profit)}</td>
  </>
);

/** Income against expenses per month, with the profit above each pair. */
function Chart({ ctx, scope }: { ctx: BusinessCtx; scope: "all" | number }) {
  const months = lastMonths(ctx.period);
  const ts = scoped(ctx, scope);
  const data = months.map((m) => figures(ts.filter((t) => t.date.startsWith(m))));
  const max = Math.max(1, ...data.flatMap((d) => [d.income, d.expense]));
  return (
    <div className="card">
      <div className="bizchart">
        {data.map((d, i) => (
          <div className="bizcol" key={months[i]}>
            <div className={`bizp num ${d.profit < 0 ? "expense" : "income"}`}>{`${d.profit < 0 ? "-" : ""}${(Math.abs(d.profit) / 100000).toFixed(1)}k`}</div>
            <div className="bizpair">
              <i className="in" style={{ height: `${(d.income / max) * 100}%` }} />
              <i className="out" style={{ height: `${(d.expense / max) * 100}%` }} />
            </div>
            <div className="bizm muted">{shortMonth(months[i]!)}</div>
          </div>
        ))}
      </div>
      <div className="bizlegend muted">
        <span>
          <i className="in" />
          הכנסות
        </span>
        <span>
          <i className="out" />
          הוצאות
        </span>
        <span>מעל העמודות: רווח באלפי ₪</span>
      </div>
    </div>
  );
}

/** One of the three figures at the top, styled like the summary's cards. */
function Stat({ label, agorot, cls }: { label: string; agorot: number; cls: string }) {
  return (
    <div className="card">
      <div className="label">{label}</div>
      <div className="value">
        <Money agorot={agorot} cls={cls} />
      </div>
    </div>
  );
}

const profitCls = (n: number) => (n < 0 ? "expense" : "income");

/** The overview, laid out like the summary: income, expense and profit cards, then a pie and a list of profit per product. */
function Overview({ ctx }: { ctx: BusinessCtx }) {
  const { scope } = ctx.view;
  const ts = scoped(ctx, scope);
  const inPeriod = (t: Transaction) => contains(ctx.period, t.date);
  const cur = figures(ts.filter(inPeriod));
  const prevP = shift(ctx.period, -1);
  const share = scope === "all" ? null : allocation(ctx, ctx.period, scope);
  // Profit of each product against the period before it; colours stay with the product, whatever the order.
  const rows = ctx.products.products
    .map((b, i) => ({
      b,
      color: colorAt(i),
      profit: figures(scoped(ctx, b.id).filter(inPeriod)).profit,
      was: prevP ? figures(scoped(ctx, b.id).filter((t) => contains(prevP, t.date))).profit : 0,
    }))
    .sort((a, b) => b.profit - a.profit);
  const positive = rows.filter((r) => r.profit > 0);
  const positiveTotal = positive.reduce((sum, r) => sum + r.profit, 0);
  const max = Math.max(1, ...rows.map((r) => Math.abs(r.profit)));
  return (
    <>
      <div className="stats">
        <Stat label="הכנסות" agorot={cur.income} cls="income" />
        <Stat label="הוצאות" agorot={cur.expense} cls="expense" />
        <Stat label="רווח" agorot={cur.profit} cls={profitCls(cur.profit)} />
      </div>
      {scope === "all" ? (
        rows.length > 0 && (
          <div className="catsgrid dash dash-cats">
            {positive.length > 0 && <DonutCard items={positive.map((r) => ({ name: r.b.is_general ? `${r.b.name} (משותף)` : r.b.name, value: r.profit, color: r.color }))} total={positiveTotal} title="רווח" />}
            <h2>רווח לפי מוצר</h2>
            <div className="card rows">
              {rows.map((r) => (
                <div className="row clickable" key={r.b.id} {...clickableProps(() => ctx.setView({ scope: r.b.id }))}>
                  <div className="grow">
                    <div className="name">{r.b.is_general ? `${r.b.name} (משותף)` : r.b.name}</div>
                    <div className="bar">
                      <span style={{ width: `${Math.max(3, (Math.abs(r.profit) / max) * 100)}%`, background: r.profit < 0 ? "var(--expense)" : r.color }} />
                    </div>
                  </div>
                  <div className="amt">
                    <Money agorot={r.profit} whole cls={profitCls(r.profit)} />
                    <div>
                      <Delta cur={r.profit} base={r.was} higherIsBad={false} />
                      {r.profit > 0 && positiveTotal > 0 && <span className="muted num">{` ${Math.round((r.profit / positiveTotal) * 100)}%`}</span>}
                    </div>
                  </div>
                  <span className="chev muted">‹</span>
                </div>
              ))}
            </div>
          </div>
        )
      ) : (
        share != null && (
          <div className="card rows">
            <div className="row">
              <div className="grow">חלק מההוצאות המשותפות (לפי הכנסות)</div>
              {signed(-share)}
            </div>
            <div className="row">
              <div className="grow name">רווח אחרי חלוקה</div>
              {signed(cur.profit - share)}
            </div>
          </div>
        )
      )}
      {scope !== "all" && ts.length > 0 && (
        <>
          <h2>הכנסות מול הוצאות · 6 חודשים</h2>
          <Chart ctx={ctx} scope={scope} />
        </>
      )}
      {!ts.length && <div className="empty">{`עוד אין תנועות ${scope === "all" ? "בעסק" : "במוצר"}.`} תייגו תנועה, או קשרו קטגוריה למוצר.</div>}
    </>
  );
}

/** The product's lines in the period, by day, as in the transactions tab. */
function ProductTransactions({ ctx }: { ctx: BusinessCtx }) {
  const txs = scoped(ctx, ctx.view.scope).filter((t) => contains(ctx.period, t.date));
  const days = [...new Set(txs.map((t) => t.date))].sort().reverse();
  if (!days.length) return <div className="empty">אין תנועות בתקופה זו</div>;
  return (
    <>
      {days.map((d) => (
        <div key={d}>
          <div className="daylabel">{dayTitle(d)}</div>
          <div className="card rows">
            {txs
              .filter((t) => t.date === d)
              .map((t, i) => (
                <TransactionRow key={i} t={t} />
              ))}
          </div>
        </div>
      ))}
    </>
  );
}

function Trends({ ctx }: { ctx: BusinessCtx }) {
  const { scope } = ctx.view;
  const months = lastMonths(ctx.period);
  const ts = scoped(ctx, scope);
  const rows = months.map((m) => ({ m, f: figures(ts.filter((t) => t.date.startsWith(m))) })).reverse();
  const sum = figures(ts.filter((t) => months.some((m) => t.date.startsWith(m))));
  return (
    <>
      <h2>הכנסות מול הוצאות · 6 חודשים</h2>
      <Chart ctx={ctx} scope={scope} />
      <FigureTable
        heads={["חודש", "הכנסות", "הוצאות", "רווח"]}
        rows={rows.map(({ m, f }) => (
          <tr key={m}>
            <td>{periodTitle({ kind: "month", month: m })}</td>
            {figureCells(f)}
          </tr>
        ))}
        total={
          <>
            <td>סה״כ</td>
            {figureCells(sum)}
          </>
        }
      />
    </>
  );
}

function CategorySide({ ctx, sign, title, empty }: { ctx: BusinessCtx; sign: 1 | -1; title: string; empty: string }) {
  const mine = scoped(ctx, ctx.view.scope).filter((t) => contains(ctx.period, t.date));
  const all = ctx.txs.filter((t) => contains(ctx.period, t.date));
  const top = (t: Transaction) => {
    const c = ctx.cats.find((x) => x.id === t.category_id);
    return c?.parent_id ?? t.category_id;
  };
  const name = (id: number | null) => ctx.cats.find((c) => c.id === id)?.name ?? "ללא קטגוריה";
  const of = (ts: Transaction[]) => {
    const m = new Map<number | null, number>();
    for (const t of ts.filter((t) => t.amount_agorot * sign > 0)) m.set(top(t), (m.get(top(t)) ?? 0) + t.amount_agorot * sign);
    return m;
  };
  const wholeCat = of(all);
  const rows = [...of(mine).entries()].sort((a, b) => b[1] - a[1]);
  const total = rows.reduce((s, [, v]) => s + v, 0);
  const max = rows[0]?.[1] ?? 1;
  return (
    <>
      <h2>{`${title} לפי קטגוריה · ${formatShekelsWhole(total)}`}</h2>
      {rows.length ? (
        <div className="card rows">
          {rows.map(([id, v]) => {
            const part = wholeCat.get(id) ?? 0;
            return (
              <div className="row" key={String(id)}>
                <div className="grow">
                  <div className="name">{name(id)}</div>
                  <div className="bar">
                    <span style={{ width: `${Math.max(3, (v / max) * 100)}%` }} />
                  </div>
                  {part > v ? <div className="muted small">{`${formatShekelsWhole(v)} מתוך ${formatShekelsWhole(part)} בקטגוריה`}</div> : null}
                </div>
                {whole(v)}
              </div>
            );
          })}
        </div>
      ) : (
        <div className="empty">{empty}</div>
      )}
    </>
  );
}

function Categories({ ctx }: { ctx: BusinessCtx }) {
  return (
    <>
      <CategorySide ctx={ctx} sign={-1} title="הוצאות" empty={`אין הוצאות ${ctx.view.scope === "all" ? "בעסק" : "במוצר"} בתקופה זו`} />
      <CategorySide ctx={ctx} sign={1} title="הכנסות" empty={`אין הכנסות ${ctx.view.scope === "all" ? "בעסק" : "במוצר"} בתקופה זו`} />
    </>
  );
}

function Compare({ ctx }: { ctx: BusinessCtx }) {
  const months = lastMonths(ctx.period);
  return (
    <>
      <h2>רווח לפי מוצר · 6 חודשים</h2>
      <div className="card rows">
        {ctx.products.products.map((b) => {
          const ts = scoped(ctx, b.id);
          const series = months.map((m) => figures(ts.filter((t) => t.date.startsWith(m))).profit);
          const cur = figures(ts.filter((t) => contains(ctx.period, t.date)));
          const max = Math.max(1, ...series.map(Math.abs));
          const margin = cur.income > 0 ? `שולי רווח ${Math.round((cur.profit / cur.income) * 100)}%` : "אין הכנסות";
          return (
            <div className="row clickable" key={b.id} {...clickableProps(() => ctx.setView({ tab: "overview", scope: b.id }))}>
              <div className="grow">
                <div className="name">{b.is_general ? `${b.name} (משותף)` : b.name}</div>
                <div className="muted small">{margin}</div>
              </div>
              <div className="bizmini">
                {series.map((v, i) => (
                  <i key={months[i]} className={v < 0 ? "neg" : ""} style={{ height: `${Math.max(6, (Math.abs(v) / max) * 100)}%` }} />
                ))}
              </div>
              <div className="bizamt">{signed(cur.profit)}</div>
            </div>
          );
        })}
      </div>
      <div className="note muted">ההוצאות המשותפות מוצגות בנפרד ולא מחולקות. בסקירה של כל מוצר אפשר לראות את הרווח אחרי חלוקה.</div>
    </>
  );
}

const BODIES = { overview: Overview, trends: Trends, cats: Categories, compare: Compare, txs: ProductTransactions };

/** The business dashboards: scope (the whole business or one of its products) and four tabs. */
export function businessDashboard(ctx: BusinessCtx): ReactElement {
  return <BusinessDashboard ctx={ctx} />;
}

function BusinessDashboard({ ctx }: { ctx: BusinessCtx }) {
  const tabs = tabsFor(ctx.view.scope);
  // A tab the scope does not have (the comparison, or a product's lines) falls back to the overview.
  const tab = tabs.some(([k]) => k === ctx.view.tab) ? ctx.view.tab : "overview";
  const Body = BODIES[tab];
  const openScopes = () =>
    openReactDialog((close) => {
      const item = (label: string, scope: "all" | number, cls = "") => (
        <button
          key={String(scope)}
          type="button"
          className={`scopeitem ${cls}`.trim()}
          onClick={() => (close(), ctx.setView({ scope }))}
        >{`${label}${scope === ctx.view.scope ? "  ✓" : ""}`}</button>
      );
      return (
        <div className="add">
          <h2>הצגת נתונים של</h2>
          {item("🏢 כל העסק", "all")}
          {ctx.products.products.filter((b) => !b.is_general).map((b) => item(b.name, b.id, "ind"))}
          {ctx.products.products.filter((b) => b.is_general).map((b) => item(`${b.name} (משותף)`, b.id, "ind"))}
        </div>
      );
    });
  return (
    <div className="bizdash">
      <div className="card filterbar">
        <div className="grow">
          <div className="name">סקירה</div>
        </div>
        <button className="btn" title="בחירת מוצר" onClick={openScopes}>{`${scopeName(ctx, ctx.view.scope)} ▾`}</button>
      </div>
      <div className="seg4">
        {tabs.map(([k, label]) => (
          <button key={k} type="button" aria-pressed={tab === k} onClick={() => ctx.setView({ tab: k })}>
            {label}
          </button>
        ))}
      </div>
      <ConnectedMonthBar />
      <Body ctx={ctx} />
    </div>
  );
}

// ---- Managing products ------------------------------------------------------------------------------

export interface ManageCtx {
  products: ProductsData;
  cats: Category[];
  txs: Transaction[];
  reload: () => Promise<void>;
  back: () => void;
  /** The back button's label (it defaults to the settings, where this screen used to live). */
  backLabel?: string;
}

const checkRow = { flexDirection: "row", alignItems: "center", gap: 8 } as const;

function NameForm({ ctx, existing, close }: { ctx: ManageCtx; existing?: Product; close: () => void }) {
  const [name, setName] = useState(existing?.name ?? "");
  const [general, setGeneral] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => void setTimeout(() => nameRef.current?.focus()), []);
  const hasGeneral = ctx.products.products.some((b) => b.is_general);
  return (
    <DialogForm
      title={existing ? "שינוי שם" : "מוצר חדש"}
      submitLabel={existing ? "שמירה" : "הוספה"}
      submitDisabled={!name.trim() || name.trim() === existing?.name}
      close={close}
      onSubmit={async () => {
        if (existing) await api.renameProduct(existing.id, name.trim());
        else await api.addProduct(name.trim(), general);
        close();
        await ctx.reload();
      }}
    >
      <label>
        שם
        <input ref={nameRef} type="text" required maxLength={60} autoComplete="off" value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      {!existing && (
        <label style={checkRow}>
          <input type="checkbox" disabled={hasGeneral} checked={general} onChange={(e) => setGeneral(e.target.checked)} />
          {hasGeneral ? "כבר יש מוצר משותף" : "הוצאות משותפות (לא שייכות למוצר מסוים)"}
        </label>
      )}
    </DialogForm>
  );
}

const openNameDialog = (ctx: ManageCtx, existing?: Product) => openReactDialog((close) => <NameForm ctx={ctx} existing={existing} close={close} />);

export function productsView(ctx: ManageCtx): ReactElement {
  return <Products ctx={ctx} />;
}

function Products({ ctx }: { ctx: ManageCtx }) {
  const tagged = (id: number) => ctx.txs.filter((t) => productOf(ctx.products, ctx.cats, t) === id && !t.upcoming).length;
  return (
    <div>
      <h1>מוצרים</h1>
      <div className="card">
        <p className="muted">
          כל התנועות נשארות בסיכום משק הבית. מוצר הוא רק מבט נוסף: אפשר לראות רווח והפסד לכל מוצר ולכל העסק יחד. הוצאה מסומנת במוצר בעת ההוספה, והכנסות נספרות לפי הקטגוריה שמקושרת למוצר (כפתור 💼 בהגדרות הקטגוריות).
        </p>
        <div className="rows">
          {ctx.products.products.length ? (
            ctx.products.products.map((b) => (
              <div className="row" key={b.id}>
                <div className="grow">
                  <div className="name">{b.is_general ? `${b.name} (משותף)` : b.name}</div>
                  <div className="muted">{`${tagged(b.id)} תנועות`}</div>
                </div>
                <button type="button" className="btn iconbtn" title="שינוי שם" aria-label="שינוי שם" onClick={() => openNameDialog(ctx, b)}>
                  <Icon name="pencil" />
                </button>
                <button
                  type="button"
                  className="btn iconbtn"
                  title="מחיקה"
                  aria-label="מחיקה"
                  onClick={async () => {
                    if (!confirm(`למחוק את "${b.name}"? התנועות נשארות, רק הסימון שלהן למוצר יוסר.`)) return;
                    await api.deleteProduct(b.id);
                    await ctx.reload();
                  }}
                >
                  <Icon name="trash" />
                </button>
              </div>
            ))
          ) : (
            <div className="empty">עוד אין מוצרים</div>
          )}
        </div>
        <div className="actions" style={{ justifyContent: "flex-start" }}>
          <button type="button" className="btn" onClick={() => openNameDialog(ctx)}>
            <Icon name="plus" size={18} />
            הוספת מוצר
          </button>
        </div>
      </div>
    </div>
  );
}

/** Choose the product for a category (all its transactions count in it), or none. */
function CategoryProductForm({ ctx, cat, close }: { ctx: ManageCtx; cat: Category; close: () => void }) {
  const [product, setProduct] = useState<number | null>(ctx.products.cat.get(cat.id) ?? null);
  const [needed, setNeeded] = useState(ctx.products.required.has(cat.id) && product == null);
  return (
    <DialogForm
      title={`מוצר של "${cat.name}"`}
      submitLabel="שמירה"
      close={close}
      onSubmit={async () => {
        await api.linkCategoryProduct(cat.id, product);
        await api.requireCategoryProduct(cat.id, needed && product == null);
        close();
        await ctx.reload();
      }}
    >
      <p className="muted">{cat.parent_id == null ? "כל התנועות בקטגוריה ובתתי-הקטגוריות שלה (שלא קושרו למוצר אחר) ייספרו במוצר, בלי תיוג נוסף." : "כל התנועות בתת-קטגוריה ייספרו במוצר, בלי תיוג נוסף."}</p>
      <ProductChips products={ctx.products} value={product} onChange={(v) => (setProduct(v), v != null && setNeeded(false))} required={false} givenName={null} />
      <label style={checkRow}>
        <input type="checkbox" disabled={product != null} checked={needed} onChange={(e) => setNeeded(e.target.checked)} />
        חובה לבחור מוצר בכל תנועה (ללא מוצר קבוע)
      </label>
    </DialogForm>
  );
}

export const openCategoryProductDialog = (ctx: ManageCtx, cat: Category) => openReactDialog((close) => <CategoryProductForm ctx={ctx} cat={cat} close={close} />);
