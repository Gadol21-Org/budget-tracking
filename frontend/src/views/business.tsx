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
  scoped,
} from "../business";
import { ProductChips } from "../components/ProductChips";
import { DialogForm } from "../components/DialogForm";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { openReactDialog } from "../components/openReactDialog";
import { formatShekelsWhole, longMonth, shortMonth } from "../format";
import { bounds, contains, shift, title as periodTitle } from "../period";
import { WEEKDAYS, colorAt, dayOfWeek, daysInMonth, todayIso } from "../insights/model";
import { CumulativeChart } from "../components/CumulativeChart";
import { ConnectedMonthBar } from "./month";
import { Chart as SvgChart, DonutCard, Delta, Insight } from "./insights";
import { TransactionRow } from "./list";
import { dayTitle } from "../selectors";

/** The tabs always cover the whole business; choosing a product only changes the overview. */
const tabsFor = (): [BusinessTab, string][] => [
  ["overview", "סקירה"],
  ["trends", "מגמות"],
  ["rhythm", "קצב חודשי"],
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
  // The month tapped: its figures show under the bars.
  const [sel, setSel] = useState<number | null>(null);
  const months = lastMonths(ctx.period);
  const ts = scoped(ctx, scope);
  const data = months.map((m) => figures(ts.filter((t) => t.date.startsWith(m))));
  const max = Math.max(1, ...data.flatMap((d) => [d.income, d.expense]));
  return (
    <div className="card">
      <div className="bizwrap">
      {sel != null && (
        <div className="biztip" role="status" style={{ right: `${Math.min(0.78, Math.max(0.22, (sel + 0.5) / months.length)) * 100}%` }}>
          <b>{longMonth(months[sel]!)}</b>
          <div>
            הכנסות <span className="income num">{formatShekelsWhole(data[sel]!.income)}</span>
          </div>
          <div>
            הוצאות <span className="expense num">{formatShekelsWhole(data[sel]!.expense)}</span>
          </div>
          <div>
            רווח <span className={`${data[sel]!.profit < 0 ? "expense" : "income"} num`}>{formatShekelsWhole(data[sel]!.profit)}</span>
          </div>
        </div>
      )}
      <div className="bizchart">
        {data.map((d, i) => (
          <div className={sel === i ? "bizcol sel" : "bizcol"} key={months[i]} {...clickableProps(() => setSel(sel === i ? null : i))}>
            <div className={`bizp num ${d.profit < 0 ? "expense" : "income"}`}>{`${d.profit < 0 ? "-" : ""}${(Math.abs(d.profit) / 100000).toFixed(1)}k`}</div>
            <div className="bizpair">
              <i className="in" style={{ height: `${(d.income / max) * 100}%` }} />
              <i className="out" style={{ height: `${(d.expense / max) * 100}%` }} />
            </div>
            <div className="bizm muted">{shortMonth(months[i]!)}</div>
          </div>
        ))}
      </div>
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
        <span>מעל העמודות: רווח באלפי ₪. הקשה על חודש מציגה את הנתונים שלו</span>
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
          <h2>תנועות</h2>
          <ProductTransactions ctx={ctx} />
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
  return (
    <>
      {!days.length && <div className="empty">אין תנועות בתקופה זו</div>}
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

/** Net result (income minus expenses) per day of a month, in agorot; index 0 = the 1st. Can be negative. */
function dayNet(ts: Transaction[], month: string): number[] {
  const out = Array<number>(daysInMonth(month)).fill(0);
  for (const t of ts) if (t.date.startsWith(month)) out[Number(t.date.slice(8)) - 1]! += t.amount_agorot;
  return out;
}

/** Signed bars (green above the line, red below) with the figure by each bar; drawn right to left like the other charts. */
function signedBars(labels: string[], values: number[], peak: number): string {
  const W = 360, padX = 6, H = 100, top = 16, bottom = 18;
  const slot = (W - 2 * padX) / labels.length;
  const max = Math.max(1, ...values.map(Math.abs));
  const half = H / 2;
  const zero = top + half;
  let out = `<svg viewBox="0 0 ${W} ${H + top + bottom}" width="100%" role="img" style="direction:ltr"><line x1="${padX}" x2="${W - padX}" y1="${zero}" y2="${zero}" stroke="var(--line)"/>`;
  values.forEach((v, i) => {
    const x0 = padX + (labels.length - 1 - i) * slot;
    const h = (Math.abs(v) / max) * (half - 10);
    const y = v >= 0 ? zero - h : zero;
    const color = v < 0 ? "var(--expense)" : "var(--income)";
    out += `<rect x="${x0 + slot * 0.14}" y="${y}" width="${slot * 0.72}" height="${h}" rx="3" fill="${color}" opacity="${i === peak ? 1 : 0.55}"><title>${labels[i]}: ${formatShekelsWhole(v)}</title></rect>`;
    out += `<text x="${x0 + slot / 2}" y="${v >= 0 ? y - 3 : y + h + 9}" font-size="8" font-weight="700" text-anchor="middle" fill="${color}">${Math.round(v / 100).toLocaleString("he-IL")}</text>`;
    out += `<text x="${x0 + slot / 2}" y="${H + top + 13}" font-size="9" text-anchor="middle" fill="var(--muted)">${labels[i]}</text>`;
  });
  return out + "</svg>";
}

/** The business's monthly rhythm: daily result as a calendar, the month so far against the last one, and the average per weekday. */
function Rhythm({ ctx }: { ctx: BusinessCtx }) {
  const p = ctx.period;
  const ts = scoped(ctx, ctx.view.scope);
  const today = todayIso();
  const parts: ReactNode[] = [];

  if (p.kind === "month") {
    const days = dayNet(ts, p.month);
    const nowMonth = today.slice(0, 7);
    const elapsed = p.month === nowMonth ? Number(today.slice(8)) : p.month > nowMonth ? 0 : days.length;
    const shown = days.slice(0, elapsed);
    const abs = shown.map(Math.abs).filter((v) => v > 0).sort((a, b) => a - b);
    const scale = abs.length ? abs[Math.min(abs.length - 1, Math.floor(abs.length * 0.9))]! : 1;
    const lead = dayOfWeek(`${p.month}-01`); // the grid starts on Sunday
    const best = shown.reduce((m, v, i) => (v > shown[m]! ? i : m), 0);
    const worst = shown.reduce((m, v, i) => (v < shown[m]! ? i : m), 0);
    parts.push(
      <h2 key="h-daily">תוצאה יומית</h2>,
      <div className="card" key="daily">
        <div className="hm">
          {WEEKDAYS.map((d) => (
            <div className="hmhead" key={d}>
              {d}
            </div>
          ))}
          {Array.from({ length: lead }, (_, i) => (
            <div className="hmgap" key={`gap${i}`} />
          ))}
          {days.map((v, i) => {
            const future = i >= elapsed;
            const style: React.CSSProperties = {};
            if (!future && v !== 0) {
              const t = Math.min(1, Math.abs(v) / scale);
              style.background = `color-mix(in srgb, var(${v < 0 ? "--expense" : "--income"}) ${Math.round(12 + t * 78)}%, transparent)`;
              if (t > 0.55) style.color = "#fff";
            }
            return (
              <div key={i} className={`hmcell${future ? " future" : ""}`} title={`${i + 1}: ${formatShekelsWhole(v)}`} style={style}>
                {String(i + 1)}
              </div>
            );
          })}
        </div>
        <div className="muted small" style={{ marginTop: 8 }}>
          ירוק = יום ברווח, אדום = יום בהפסד. כהה יותר = סכום גדול יותר.
        </div>
        {shown[best]! > 0 && (
          <Insight ico="💰" style={{ marginTop: 8 }}>
            {"היום הרווחי ביותר: ה-"}
            <b>{String(best + 1)}</b>
            {" ("}
            {signed(shown[best]!)}
            {")"}
          </Insight>
        )}
        {shown[worst]! < 0 && (
          <Insight ico="📉" style={{ marginTop: 8 }}>
            {"היום החלש ביותר: ה-"}
            <b>{String(worst + 1)}</b>
            {" ("}
            {signed(shown[worst]!)}
            {")"}
          </Insight>
        )}
      </div>,
    );

    // Cumulative result through the month, against the same days of the previous month.
    const cum = (vs: number[]) => vs.reduce<number[]>((a, v, i) => (a.push((a[i - 1] ?? 0) + v), a), []);
    const prev = shift(p, -1);
    let ghost: number[] | undefined;
    if (prev && prev.kind === "month") {
      const c = cum(dayNet(ts, prev.month));
      ghost = Array.from({ length: days.length }, (_, i) => c[Math.min(i, c.length - 1)] ?? 0);
    }
    const cur = cum(shown);
    parts.push(
      <h2 key="h-cum">רווח מצטבר במהלך החודש</h2>,
      <div className="card" key="cum">
        {cur.length ? <CumulativeChart days={days.map((_, i) => String(i + 1))} cur={cur} prev={ghost} color="var(--biz)" curLabel="החודש" prevLabel="חודש קודם" signed /> : <div className="empty">החודש עוד לא התחיל</div>}
      </div>,
    );
  } else {
    parts.push(
      <div className="muted small pad" key="note">
        המפה היומית זמינה כשבוחרים חודש בודד. להלן ממוצע לפי יום בשבוע לתקופה שנבחרה.
      </div>,
    );
  }

  // Average result per weekday over the days of the period that have passed.
  const { from, to } = bounds(p);
  const end = to < today ? to : today;
  const sums = Array<number>(7).fill(0), counts = Array<number>(7).fill(0);
  for (let d = from; d <= end; d = new Date(Date.parse(`${d}T00:00:00Z`) + 864e5).toISOString().slice(0, 10)) counts[dayOfWeek(d)]!++;
  for (const t of ts) if (t.date >= from && t.date <= end) sums[dayOfWeek(t.date)]! += t.amount_agorot;
  const avg = sums.map((s, i) => (counts[i] ? s / counts[i]! : 0));
  const top = avg.reduce((m, v, i) => (v > avg[m]! ? i : m), 0);
  parts.push(
    <h2 key="h-week">ממוצע לפי יום בשבוע</h2>,
    <div className="card" key="week">
      <SvgChart html={signedBars(WEEKDAYS, avg, top)} />
      {avg[top]! > 0 && (
        <Insight ico="📅" style={{ marginTop: 8 }}>
          {"יום "}
          <b>{WEEKDAYS[top]}</b>
          {" הוא היום הרווחי ביותר: "}
          {signed(Math.round(avg[top]!))}
          {" בממוצע."}
        </Insight>
      )}
    </div>,
  );
  return <div>{parts}</div>;
}

const BODIES = { overview: Overview, trends: Trends, rhythm: Rhythm };

/** The business dashboards: scope (the whole business or one of its products) and four tabs. */
export function businessDashboard(ctx: BusinessCtx): ReactElement {
  return <BusinessDashboard ctx={ctx} />;
}

function BusinessDashboard({ ctx }: { ctx: BusinessCtx }) {
  const tabs = tabsFor();
  // A tab the scope does not have (the comparison, or a product's lines) falls back to the overview.
  const tab = tabs.some(([k]) => k === ctx.view.tab) ? ctx.view.tab : "overview";
  const Body = BODIES[tab];
  return (
    <div className="bizdash">
      <div className="seg4">
        {tabs.map(([k, label]) => (
          <button key={k} type="button" aria-pressed={tab === k} onClick={() => ctx.setView({ tab: k, scope: "all" })}>
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
