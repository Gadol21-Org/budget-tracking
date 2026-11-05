import { type MouseEvent, type ReactElement, type ReactNode, useEffect, useRef, useState } from "react";
import { type Transaction } from "../api";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { openReactDialog } from "../components/openReactDialog";
import { formatShekelsWhole, longMonth, shortMonth } from "../format";
import { type BarOpts, barChart, donut, lineChart } from "../insights/charts";
import {
  type Agg,
  type Cmp,
  type Ctx,
  type Dash,
  type InsightsNav,
  OTHER_COLOR,
  SIDES,
  WEEKDAYS,
  aggregate,
  basePeriods,
  baseValue,
  catColor,
  catName,
  catPath,
  colorAt,
  dayOfWeek,
  dayTotals,
  daysInMonth,
  highlightSet,
  isFixed,
  isPartial,
  monthsEnding,
  sideOf,
  todayIso,
} from "../insights/model";
import { bounds, contains, shift, title as periodTitle, type Period } from "../period";
import { ConnectedMonthBar } from "./month";

const DASHES: [Dash, string, string][] = [
  ["overview", "🏠", "סקירה"],
  ["trends", "📈", "מגמות"],
  ["changes", "🔀", "מה השתנה"],
  ["fixed", "📌", "קבוע ומשתנה"],
  ["rhythm", "📅", "קצב חודשי"],
];

const money = (agorot: number, cls = "") => <Money agorot={agorot} whole cls={cls} />;
const swatch = (background: string) => <i style={{ background }} />;

// ---- Small pieces --------------------------------------------------------------------------------

/** "▲ 12%" against the comparison; red when it is a rise in spending, green when it is a fall. */
function Delta({ cur, base, higherIsBad }: { cur: number; base: number; higherIsBad: boolean }) {
  if (base === 0) return cur === 0 ? null : <span className="delta flat">חדש</span>;
  const pct = Math.round(((cur - base) / base) * 100);
  const flat = Math.abs(pct) < 3;
  const bad = higherIsBad ? pct > 0 : pct < 0;
  const arrow = pct > 0 ? "▲" : pct < 0 ? "▼" : "■";
  return (
    <span className={`delta ${flat ? "flat" : bad ? "bad" : "good"}`}>
      <span className="num">{`${pct > 0 ? "+" : ""}${pct}%`}</span>
      {` ${arrow}`}
    </span>
  );
}

function Kpi({ label, value, sub }: { label: string; value: ReactNode; sub?: ReactNode }) {
  return (
    <div className="kpi">
      <div className="l">{label}</div>
      <div className="v">{value}</div>
      <div>{sub || <span>{" "}</span>}</div>
    </div>
  );
}

const Note = ({ text }: { text: string }) => <span className="delta flat">{text}</span>;

/** An SVG drawn from markup; taps on a part carrying data-i are reported with its index. */
function Chart({ html, onTap }: { html: string; onTap?: (i: number) => void }) {
  return (
    <div
      className="chart"
      dangerouslySetInnerHTML={{ __html: html }}
      onClick={
        onTap &&
        ((e: MouseEvent) => {
          const hit = (e.target as Element).closest("[data-i]");
          if (hit) onTap(Number((hit as HTMLElement).dataset.i));
        })
      }
    />
  );
}

/**
 * A month bar chart whose taps only print that month's totals above its bars (nothing else opens).
 * The active period is left alone; tapping the same month again hides them.
 */
function PeekChart(o: BarOpts) {
  const [peek, setPeek] = useState<number | null>(null);
  return <Chart html={barChart({ ...o, peek })} onTap={(i) => setPeek(peek === i ? null : i)} />;
}

const Legend = ({ children, wrap = false }: { children: ReactNode; wrap?: boolean }) => <div className={wrap ? "legend wrap" : "legend"}>{children}</div>;

/** One legend line: colour, name (cut with … when long), amount and share, each in its own column so the numbers line up. */
function LegendRow({ color, name, value, total, state, onTap }: { color: string; name: string; value: number; total: number; state: "sel" | "dim" | ""; onTap?: () => void }) {
  const cls = `lrow${onTap ? " clickable" : ""}${state ? ` ${state}` : ""}`;
  return (
    <div className={cls} {...(onTap ? clickableProps(onTap) : {})}>
      {swatch(color)}
      <span className="nm" title={name}>
        {name}
      </span>
      {money(value)}
      <span className="muted num">{`${Math.max(1, Math.round((value / total) * 100))}%`}</span>
    </div>
  );
}

/**
 * A card with a donut and its legend. Tapping a slice or a legend line greys out the other slices and shows its name and
 * amount in the middle; tapping it again puts it back.
 */
function DonutCard({ items, total, title }: { items: { name: string; value: number; color: string }[]; total: number; title: string }) {
  const [sel, setSel] = useState<number | null>(null);
  const pick = (i: number) => setSel(sel === i ? null : i);
  const html = donut(items, { title, total: formatShekelsWhole(total) }, sel, sel == null ? "" : `${items[sel]!.name}: ${formatShekelsWhole(items[sel]!.value)}`);
  return (
    <div className="card donutcard">
      <Chart html={html} onTap={pick} />
      <div className="donutlegend tbl">
        {items.map((x, i) => (
          <LegendRow key={i} color={x.color} name={x.name} value={x.value} total={total} state={i === sel ? "sel" : sel != null ? "dim" : ""} onTap={() => pick(i)} />
        ))}
      </div>
    </div>
  );
}

const Insight = ({ ico, children, style }: { ico: string; children: ReactNode; style?: React.CSSProperties }) => (
  <div className="insight" style={style}>
    <span className="ico">{ico}</span>
    <div>{children}</div>
  </div>
);

// ---- Shared frame (chips, compare-to, partial note) ------------------------------------------

/** The insights screen: a row of dashboards to pick from, the comparison, and the open dashboard. */
export function insightsView(ctx: Ctx): ReactElement {
  return <InsightsScreen ctx={ctx} />;
}

const BODIES = { overview: Overview, trends: Trends, changes: Changes, fixed: Fixed, rhythm: Rhythm };

function InsightsScreen({ ctx }: { ctx: Ctx }) {
  // The category breakdown moved to the summary; "cats" only remains as the drill-down marker in history entries.
  const dash: Exclude<Dash, "cats"> = ctx.ins.dash === "cats" ? "overview" : ctx.ins.dash;
  const chips = useRef<HTMLDivElement>(null);
  // With six chips the open one can sit off-screen: bring it into view after the page is laid out.
  useEffect(() => void requestAnimationFrame(() => chips.current?.querySelector<HTMLElement>('[aria-pressed="true"]')?.scrollIntoView({ inline: "center", block: "nearest" })), []);
  const base = basePeriods(ctx.period, ctx.ins.cmp);
  const hasCompare = dash !== "trends" && dash !== "fixed";
  const Body = BODIES[dash];
  return (
    <div>
      <ConnectedMonthBar />
      <div className="chips" role="tablist" ref={chips}>
        {DASHES.map(([id, ico, label]) => (
          <button key={id} className="chip" type="button" aria-pressed={dash === id} onClick={() => ctx.navigate({ dash: id, cat: undefined, sub: undefined }, "replace")}>
            {`${ico} ${label}`}
          </button>
        ))}
      </div>
      {hasCompare && (
        <div className="cmpbar">
          <span className="muted">השוואה:</span>
          <CompareButton ctx={ctx} label={base.label} />
        </div>
      )}
      {hasCompare && isPartial(ctx.period) && <div className="partialnote">התקופה טרם הסתיימה: הסכומים חלקיים, ולכן ההשוואה למחזור מלא תראה נמוכה. לתמונה מלאה בחרו חודש שהסתיים.</div>}
      <div className={`dash dash-${dash}`}>
        <Body ctx={ctx} baseLabel={base.label} />
      </div>
    </div>
  );
}

function CompareButton({ ctx, label }: { ctx: Ctx; label: string }) {
  return (
    <button className="cmpbtn" type="button" onClick={() => openReactDialog((close) => <CompareForm ctx={ctx} close={close} />)}>
      {`${label} ▾`}
    </button>
  );
}

function CompareForm({ ctx, close }: { ctx: Ctx; close: () => void }) {
  const p = ctx.period;
  const cur = ctx.ins.cmp;
  const unit = p.kind === "month" ? "חודשים" : p.kind === "year" ? "שנים" : "טווחים";
  const options: [string, string][] = [
    ["prev", p.kind === "month" ? "החודש הקודם" : p.kind === "year" ? "השנה הקודמת" : "הטווח הקודם"],
    ["avg3", `ממוצע 3 ה${unit} שלפני`],
    ["avg6", `ממוצע 6 ה${unit} שלפני`],
    ["avg12", `ממוצע 12 ה${unit} שלפני`],
  ];
  if (p.kind !== "range") options.push(["custom", p.kind === "month" ? "חודש אחר…" : "שנה אחרת…"]);
  const initial = cur.kind === "avg" ? `avg${cur.n}` : cur.kind;
  const [sel, setSel] = useState(options.some(([v]) => v === initial) ? initial : "prev");
  const [monthIn, setMonthIn] = useState(cur.kind === "custom" && cur.period.kind === "month" ? cur.period.month : ((shift(p, -1) as { month: string } | null)?.month ?? ""));
  const [yearIn, setYearIn] = useState(String(cur.kind === "custom" && cur.period.kind === "year" ? cur.period.year : p.kind === "year" ? p.year - 1 : new Date().getFullYear() - 1));
  return (
    <form
      className="add"
      method="dialog"
      onSubmit={(e) => {
        e.preventDefault();
        if (sel === "custom") {
          const period: Period | null = p.kind === "month" ? (monthIn ? { kind: "month", month: monthIn } : null) : yearIn ? { kind: "year", year: Number(yearIn) } : null;
          if (!period) return;
          ctx.setCmp({ kind: "custom", period });
        } else if (sel.startsWith("avg")) ctx.setCmp({ kind: "avg", n: Number(sel.slice(3)) } as Cmp);
        else ctx.setCmp({ kind: "prev" });
        close();
      }}
    >
      <h2>{`השוואת ${periodTitle(p)} אל…`}</h2>
      <label>
        השוואה
        <select value={sel} onChange={(e) => setSel(e.target.value)}>
          {options.map(([v, t]) => (
            <option key={v} value={v}>
              {t}
            </option>
          ))}
        </select>
      </label>
      <label hidden={sel !== "custom"}>
        {p.kind === "month" ? "חודש להשוואה" : "שנה להשוואה"}
        {p.kind === "month" ? <input type="month" value={monthIn} onChange={(e) => setMonthIn(e.target.value)} /> : <input type="number" min="2000" max="2100" value={yearIn} onChange={(e) => setYearIn(e.target.value)} />}
      </label>
      <div className="actions">
        <button type="button" className="btn" onClick={close}>
          ביטול
        </button>
        <button type="submit" className="btn primary">
          החלה
        </button>
      </div>
    </form>
  );
}

// ---- Dashboard: overview ---------------------------------------------------------------------

type BodyProps = { ctx: Ctx; baseLabel: string };

function Overview({ ctx, baseLabel }: BodyProps) {
  const cur = aggregate(ctx, ctx.period);
  const bal = cur.income - cur.expense;
  const bExp = baseValue(ctx, (a) => a.expense);
  const bInc = baseValue(ctx, (a) => a.income);
  const bBal = baseValue(ctx, (a) => a.income - a.expense);
  const rate = cur.income > 0 ? Math.round((bal / cur.income) * 100) : null;
  const months = monthsEnding(ctx.period);
  const series = months.map((m) => aggregate(ctx, { kind: "month", month: m }).expense);
  const full = series.filter((_, i) => months[i]! < todayIso().slice(0, 7));
  const avg = full.length ? full.reduce((a, b) => a + b, 0) / full.length : undefined;

  // Plain-language insights.
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => aggregate(ctx, p));
  const changes = [...cur.groups.entries()]
    .map(([id, g]) => ({ id, now: g.total, was: bases.reduce((s, b) => s + (b.groups.get(id)?.total ?? 0), 0) / bases.length }))
    .map((x) => ({ ...x, d: x.now - x.was }));
  const lines: ReactNode[] = [];
  const up = [...changes].sort((a, b) => b.d - a.d)[0];
  const down = [...changes].sort((a, b) => a.d - b.d)[0];
  if (up && up.d > 0 && up.was > 0)
    lines.push(
      <Insight key="up" ico="📈">
        <b>{catName(ctx, up.id)}</b>: {money(up.d, "expense")}
        {` יותר (${Math.round((up.d / up.was) * 100)}%) ${baseLabel}.`}
      </Insight>,
    );
  if (down && down.d < 0 && down !== up)
    lines.push(
      <Insight key="down" ico="📉">
        <b>{catName(ctx, down.id)}</b>: {money(-down.d, "income")}
        {` פחות ${baseLabel}.`}
      </Insight>,
    );
  const upcoming = ctx.txs.filter((t) => t.upcoming && contains(ctx.period, t.date));
  if (upcoming.length) {
    const out = -upcoming.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);
    const inc = upcoming.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
    lines.push(
      <Insight key="upcoming" ico="⏳">
        הסכומים כוללים {money(out)} חיובים חוזרים
        {inc ? (
          <>
            {" ו-"}
            {money(inc)}
            {" הכנסות חוזרות"}
          </>
        ) : null}
        {" שטרם בוצעו. יתרה צפויה בסוף התקופה: "}
        {money(bal, bal < 0 ? "expense" : "income")}.
      </Insight>,
    );
  }
  const biggest = ctx.txs.filter((t) => t.amount_agorot < 0 && contains(ctx.period, t.date)).sort((a, b) => a.amount_agorot - b.amount_agorot)[0];
  if (biggest) {
    const c = ctx.cats.find((x) => x.id === biggest.category_id);
    lines.push(
      <Insight key="biggest" ico="🧾">
        ההוצאה הגדולה ביותר: <b>{biggest.note || c?.name || "ללא קטגוריה"}</b>
        {" – "}
        {money(-biggest.amount_agorot)}.
      </Insight>,
    );
  }

  return (
    <div>
      <div className="kpis">
        <Kpi label="הוצאות" value={money(cur.expense, "expense")} sub={<Delta cur={cur.expense} base={bExp} higherIsBad />} />
        <Kpi label="הכנסות" value={money(cur.income, "income")} sub={<Delta cur={cur.income} base={bInc} higherIsBad={false} />} />
        <Kpi
          label="יתרה"
          value={money(bal, bal < 0 ? "expense" : "income")}
          sub={<span className={`num delta ${Math.abs(bal - bBal) < 100 ? "flat" : bal > bBal ? "good" : "bad"}`}>{`${bal >= bBal ? "+" : "−"}${formatShekelsWhole(Math.abs(bal - bBal))}`}</span>}
        />
        <Kpi label="שיעור חיסכון" value={<span className="num">{rate == null ? "–" : `${rate}%`}</span>} sub={<Note text="מההכנסות" />} />
      </div>
      <h2>תובנות</h2>
      <div className="card">{lines.length ? lines : <div className="empty">אין עדיין נתונים בתקופה זו</div>}</div>
      <h2>הוצאות ב-12 החודשים האחרונים</h2>
      <div className="card">
        <PeekChart labels={months.map(shortMonth)} tips={months.map((m, i) => `${longMonth(m)}: ${formatShekelsWhole(series[i]!)}`)} series={[series]} colors={["var(--expense)"]} highlight={highlightSet(ctx, months)} avg={avg} />
        <Legend>
          <span>
            {swatch("var(--expense)")}
            הוצאות
          </span>
          {avg != null && (
            <span>
              {swatch("#d97706")}
              ממוצע החודשים שהסתיימו
            </span>
          )}
          <span>הקשה על עמודה מציגה את הסכום מעליה</span>
        </Legend>
      </div>
    </div>
  );
}

// ---- Dashboard: categories and drill-down -----------------------------------------------------

/** The donut and the category list of one side; `heading` replaces the default hint above the list. */
function CatsList({ ctx, heading }: { ctx: Ctx; heading?: ReactNode }) {
  const side = sideOf(ctx), S = SIDES[side];
  const cur = aggregate(ctx, ctx.period, side);
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => aggregate(ctx, p, side));
  const rows = [...cur.groups.entries()].sort((a, b) => b[1].total - a[1].total);
  const max = rows[0]?.[1].total ?? 1;
  if (rows.length === 0)
    return (
      <div className="dash dash-cats">
        {heading}
        <div className="empty">{S.empty}</div>
      </div>
    );
  return (
    <div className="catsgrid dash dash-cats">
      {/* On the summary the heading (with the comparison chip) leads the section; in the old tab it sat between donut and list. */}
      {heading}
      <DonutCard items={rows.map(([id, g]) => ({ name: catName(ctx, id), value: g.total, color: catColor(ctx, id) }))} total={cur.sideTotal} title={S.title} />
      {heading ? null : <h2>הקשה על קטגוריה פותחת אותה</h2>}
      <div className="card rows">
        {rows.map(([id, g]) => {
          const was = bases.reduce((s, b) => s + (b.groups.get(id)?.total ?? 0), 0) / bases.length;
          return (
            <div className="row clickable" key={String(id)} {...clickableProps(() => ctx.navigate({ dash: "cats", cat: id, sub: undefined }, "push"))}>
              <div className="grow">
                <div className="name">{catName(ctx, id)}</div>
                <div className="bar">
                  <span style={{ width: `${Math.max(3, (g.total / max) * 100)}%`, background: catColor(ctx, id) }} />
                </div>
              </div>
              <div className="amt">
                {money(g.total)}
                <div>
                  <Delta cur={g.total} base={was} higherIsBad={S.higherIsBad} /> <span className="muted num">{`${Math.round((g.total / cur.sideTotal) * 100)}%`}</span>
                </div>
              </div>
              <span className="chev muted">‹</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/**
 * The category breakdown shown on the summary page, for the side in `ctx.ins.side`: the donut and list with
 * the comparison, or, once a category is open, its drill-down (and a sub-category's). The month bar and the
 * income/expense cards stay with the summary.
 */
export function SummaryCategories({ ctx }: { ctx: Ctx }) {
  const base = basePeriods(ctx.period, ctx.ins.cmp);
  const S = SIDES[sideOf(ctx)];
  const body =
    ctx.ins.cat !== undefined && ctx.ins.sub !== undefined ? (
      <SubDetail ctx={ctx} />
    ) : ctx.ins.cat !== undefined ? (
      <CatDetail ctx={ctx} baseLabel={base.label} />
    ) : (
      <CatsList
        ctx={ctx}
        heading={
          <h2 className="sumhead">
            <span>{`${S.title} לפי קטגוריה`}</span>
            <CompareButton ctx={ctx} label={base.label} />
          </h2>
        }
      />
    );
  return (
    <div>
      {ctx.ins.cat === undefined && isPartial(ctx.period) && <div className="partialnote">התקופה טרם הסתיימה: הסכומים חלקיים, ולכן ההשוואה למחזור מלא תראה נמוכה.</div>}
      {body}
    </div>
  );
}

function Crumbs({ ctx, parts }: { ctx: Ctx; parts: { text: string; to?: Partial<InsightsNav> }[] }) {
  return (
    <div className="crumbs">
      {parts.map((p, i) => (
        <span key={i} style={{ display: "contents" }}>
          {i ? <span className="muted">›</span> : null}
          {p.to ? (
            <button type="button" className="crumb" onClick={() => ctx.navigate({ dash: "cats", ...p.to }, "push")}>
              {p.text}
            </button>
          ) : (
            <b>{p.text}</b>
          )}
        </span>
      ))}
    </div>
  );
}

/** The breadcrumb of an open category (or sub-category), shown in the summary's top bar. */
export function DrillCrumbs({ ctx }: { ctx: Ctx }) {
  const id = ctx.ins.cat!, sub = ctx.ins.sub;
  const root = { text: `${SIDES[sideOf(ctx)].title} לפי קטגוריה`, to: { cat: undefined, sub: undefined } };
  return <Crumbs ctx={ctx} parts={sub === undefined ? [root, { text: catName(ctx, id) }] : [root, { text: catName(ctx, id), to: { cat: id, sub: undefined } }, { text: catName(ctx, sub) }]} />;
}

/** The monthly chart of one category or sub-category. */
function trendCard(ctx: Ctx, value: (a: Agg) => number, color: string) {
  const months = monthsEnding(ctx.period);
  const series = months.map((m) => value(aggregate(ctx, { kind: "month", month: m }, sideOf(ctx))));
  const full = series.filter((_, i) => months[i]! < todayIso().slice(0, 7));
  const avg = full.length ? full.reduce((a, b) => a + b, 0) / full.length : series.reduce((a, b) => a + b, 0) / series.length;
  return {
    avg,
    node: (
      <div className="card">
        <PeekChart labels={months.map(shortMonth)} tips={months.map((m, i) => `${longMonth(m)}: ${formatShekelsWhole(series[i]!)}`)} series={[series]} colors={[color]} highlight={highlightSet(ctx, months)} avg={avg} />
        <Legend>
          <span>
            {swatch("#d97706")}
            ממוצע החודשים שהסתיימו
          </span>
        </Legend>
      </div>
    ),
  };
}

function CatDetail({ ctx, baseLabel }: BodyProps) {
  const id = ctx.ins.cat!;
  const side = sideOf(ctx), S = SIDES[side];
  const cur = aggregate(ctx, ctx.period, side);
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => aggregate(ctx, p, side));
  const g = cur.groups.get(id) ?? { total: 0, direct: 0, kids: new Map<number, number>() };
  const was = bases.reduce((s, b) => s + (b.groups.get(id)?.total ?? 0), 0) / bases.length;
  const trend = trendCard(ctx, (a) => a.groups.get(id)?.total ?? 0, catColor(ctx, id));
  const kids = [...g.kids.entries()].sort((a, b) => b[1] - a[1]);
  const kidMax = Math.max(1, kids[0]?.[1] ?? 0, g.direct);
  const kidRow = (key: string, name: string, v: number, wasV: number, onTap?: () => void) => (
    <div className={onTap ? "row clickable" : "row"} key={key} {...(onTap ? clickableProps(onTap) : {})}>
      <div className="grow">
        <div className="name">{name}</div>
        <div className="bar">
          <span style={{ width: `${Math.max(3, (v / kidMax) * 100)}%`, background: catColor(ctx, id) }} />
        </div>
      </div>
      <div className="amt">
        {money(v)}
        <div>
          <Delta cur={v} base={wasV} higherIsBad={S.higherIsBad} />
        </div>
      </div>
      {onTap ? <span className="chev muted">‹</span> : null}
    </div>
  );
  const wasKid = (kid: number) => bases.reduce((s, b) => s + (b.groups.get(id)?.kids.get(kid) ?? 0), 0) / bases.length;
  const wasDirect = bases.reduce((s, b) => s + (b.groups.get(id)?.direct ?? 0), 0) / bases.length;
  // Pie of the sub-categories (what is booked on the category itself shows as a grey slice).
  const slices = [...kids.map(([k, v], i) => ({ name: catName(ctx, k), value: v, color: colorAt(i) })), ...(g.direct > 0 && kids.length > 0 ? [{ name: "ללא תת-קטגוריה", value: g.direct, color: OTHER_COLOR }] : [])];
  return (
    <div className="dash dash-cats">
      <div className="kpis">
        <Kpi
          label={periodTitle(ctx.period)}
          value={money(g.total, S.cls)}
          sub={
            <span>
              <Delta cur={g.total} base={was} higherIsBad={S.higherIsBad} />
              {` ${baseLabel}`}
            </span>
          }
        />
        <Kpi label="ממוצע חודשי" value={money(Math.round(trend.avg))} sub={<Note text={cur.sideTotal ? `${Math.round((g.total / cur.sideTotal) * 100)}% ${S.total}` : ""} />} />
      </div>
      {kids.length > 0 && <h2>{`${catName(ctx, id)} לפי תת-קטגוריה`}</h2>}
      {kids.length > 0 && <DonutCard items={slices} total={g.total} title={S.title} />}
      <h2>{`${catName(ctx, id)} לאורך זמן`}</h2>
      {trend.node}
      <h2>{kids.length ? "תתי-קטגוריות" : ""}</h2>
      {kids.length || g.direct ? (
        <div className="card rows">
          {kids.map(([k, v]) => kidRow(String(k), catName(ctx, k), v, wasKid(k), () => ctx.navigate({ dash: "cats", cat: id, sub: k }, "push")))}
          {g.direct > 0 && kids.length > 0 && kidRow("direct", "ללא תת-קטגוריה", g.direct, wasDirect)}
        </div>
      ) : (
        <div className="empty">{`אין ${S.title} בקטגוריה זו בתקופה`}</div>
      )}
      {id != null && (
        <button type="button" className="btn" onClick={() => ctx.openList(id)}>
          {`הצגת התנועות של ${catName(ctx, id)} ‹`}
        </button>
      )}
    </div>
  );
}

function SubDetail({ ctx }: { ctx: Ctx }) {
  const id = ctx.ins.cat!, sub = ctx.ins.sub!;
  const side = sideOf(ctx), S = SIDES[side];
  const value = (a: Agg) => a.groups.get(id)?.kids.get(sub) ?? 0;
  const cur = value(aggregate(ctx, ctx.period, side));
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => value(aggregate(ctx, p, side)));
  const was = bases.reduce((a, b) => a + b, 0) / bases.length;
  const trend = trendCard(ctx, value, catColor(ctx, id));
  const txs = ctx.txs.filter((t) => t.category_id === sub && S.pick(t) && contains(ctx.period, t.date)).sort((a, b) => Math.abs(b.amount_agorot) - Math.abs(a.amount_agorot)).slice(0, 8);
  return (
    <div className="dash dash-cats">
      <div className="kpis">
        <Kpi label={periodTitle(ctx.period)} value={money(cur, S.cls)} sub={<Delta cur={cur} base={was} higherIsBad={S.higherIsBad} />} />
        <Kpi
          label="ממוצע חודשי"
          value={money(Math.round(trend.avg))}
          sub={<Note text={trend.avg ? `${cur >= trend.avg ? "+" : ""}${Math.round(((cur - trend.avg) / trend.avg) * 100)}% מהממוצע` : ""} />}
        />
      </div>
      <h2>{`${catName(ctx, sub)} לאורך זמן`}</h2>
      {trend.node}
      <h2>התנועות הגדולות בתקופה</h2>
      {txs.length ? (
        <div className="card rows">
          {txs.map((t, i) => (
            <div className="row" key={i}>
              <div className="grow">
                <div className="name">{t.note || catName(ctx, sub)}</div>
                <div className="muted small">{`${t.upcoming ? "⏳ " : ""}${new Date(`${t.date}T00:00:00`).toLocaleDateString("he-IL", { day: "numeric", month: "long" })}`}</div>
              </div>
              {money(Math.abs(t.amount_agorot), S.cls)}
            </div>
          ))}
        </div>
      ) : (
        <div className="empty">אין תנועות בתקופה</div>
      )}
      <button type="button" className="btn" onClick={() => ctx.openList(sub)}>
        {`הצגת כל התנועות של ${catName(ctx, sub)} ‹`}
      </button>
    </div>
  );
}

// ---- Dashboard: trends ---------------------------------------------------------------------------

function Trends({ ctx }: BodyProps) {
  const months = monthsEnding(ctx.period);
  const aggs = months.map((m) => aggregate(ctx, { kind: "month", month: m }));
  const income = aggs.map((a) => a.income), expense = aggs.map((a) => a.expense);
  const labels = months.map(shortMonth);
  const tips = months.map((m, i) => `${longMonth(m)}: הכנסות ${formatShekelsWhole(income[i]!)}, הוצאות ${formatShekelsWhole(expense[i]!)}`);
  const hi = highlightSet(ctx, months);

  // Averages count only months that have ended and hold data.
  const thisMonth = todayIso().slice(0, 7);
  const done = months.map((_, i) => i).filter((i) => months[i]! < thisMonth && (income[i] || expense[i]));
  const mean = (xs: number[]) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : 0);
  const avgExp = mean(done.map((i) => expense[i]!)), avgSave = mean(done.map((i) => income[i]! - expense[i]!));

  const cumulative: number[] = [];
  for (let i = 0, run = 0; i < months.length; i++) cumulative.push((run += income[i]! - expense[i]!));

  // Top categories over the 12 months, the rest as "other".
  const totals = new Map<number | null, number>();
  for (const a of aggs) for (const [id, g] of a.groups) totals.set(id, (totals.get(id) ?? 0) + g.total);
  const order = [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([id]) => id);
  const shown = order.slice(0, 6);
  const stackSeries = [...shown.map((id) => aggs.map((a) => a.groups.get(id)?.total ?? 0)), aggs.map((a) => [...a.groups].filter(([id]) => !shown.includes(id)).reduce((s, [, g]) => s + g.total, 0))];
  const stackColors = [...shown.map((id) => catColor(ctx, id)), OTHER_COLOR];

  return (
    <div>
      <div className="kpis">
        <Kpi label="הוצאות בחודש (ממוצע)" value={money(Math.round(avgExp), "expense")} sub={<Note text={`${done.length} חודשים שהסתיימו`} />} />
        <Kpi label="חיסכון בחודש (ממוצע)" value={money(Math.round(avgSave), avgSave < 0 ? "expense" : "income")} sub={<Note text="הכנסות פחות הוצאות" />} />
      </div>
      <h2>הכנסות מול הוצאות</h2>
      <div className="card">
        <PeekChart labels={labels} tips={tips} series={[income, expense]} colors={["var(--income)", "var(--expense)"]} mode="grouped" highlight={hi} />
        <Legend>
          <span>
            {swatch("var(--income)")}
            הכנסות
          </span>
          <span>
            {swatch("var(--expense)")}
            הוצאות
          </span>
        </Legend>
      </div>
      <h2>חיסכון מצטבר</h2>
      <div className="card">
        <Chart html={lineChart(cumulative, labels, "var(--brand)")} />
        <div className="muted small">הקו עולה כשהחודש נסגר בפלוס, ויורד כשבמינוס.</div>
      </div>
      <h2>הוצאות לפי קטגוריה</h2>
      <div className="card">
        <PeekChart labels={labels} tips={tips} series={stackSeries} colors={stackColors} mode="stacked" highlight={hi} height={140} />
        <Legend wrap>
          {shown.map((id, i) => (
            <span key={String(id)}>
              {swatch(stackColors[i]!)}
              {catName(ctx, id)}
            </span>
          ))}
          {stackSeries.at(-1)!.some((v) => v > 0) && (
            <span>
              {swatch(OTHER_COLOR)}
              אחר
            </span>
          )}
        </Legend>
      </div>
      <div className="muted small pad">הקשה על חודש בגרף מציגה את הסכום מעליו, בלי לשנות את התקופה הנבחרת.</div>
    </div>
  );
}

// ---- Dashboard: what changed ---------------------------------------------------------------------

function Changes({ ctx, baseLabel }: BodyProps) {
  const cur = aggregate(ctx, ctx.period);
  const bases = basePeriods(ctx.period, ctx.ins.cmp).periods.map((p) => aggregate(ctx, p));
  interface Item { top: number | null; sub?: number; name: string; parent?: string; now: number; was: number }
  const items = new Map<string, Item>();
  const put = (a: Agg, weight: number, isNow: boolean) => {
    for (const [top, g] of a.groups) {
      const add = (key: string, sub: number | undefined, name: string, parent: string | undefined, v: number) => {
        const it = items.get(key) ?? { top, sub, name, parent, now: 0, was: 0 };
        if (isNow) it.now += v;
        else it.was += v * weight;
        items.set(key, it);
      };
      for (const [kid, v] of g.kids) add(`${top}:${kid}`, kid, catName(ctx, kid), catName(ctx, top), v);
      if (g.direct > 0) add(`${top}:`, undefined, catName(ctx, top), undefined, g.direct);
    }
  };
  put(cur, 1, true);
  for (const b of bases) put(b, 1 / bases.length, false);
  const list = [...items.values()].map((i) => ({ ...i, d: i.now - i.was }));
  const up = list.filter((i) => i.d > 0).sort((a, b) => b.d - a.d).slice(0, 6);
  const down = list.filter((i) => i.d < 0).sort((a, b) => a.d - b.d).slice(0, 6);
  const wasTotal = bases.reduce((s, b) => s + b.expense, 0) / bases.length;
  const net = cur.expense - wasTotal;
  const max = Math.max(1, ...up.map((i) => i.d), ...down.map((i) => -i.d));
  const row = (i: (typeof list)[number], grow: boolean) => {
    const color = grow ? "var(--expense)" : "var(--income)";
    return (
      <div className="row clickable" key={`${i.top}:${i.sub ?? ""}`} {...clickableProps(() => ctx.openCategory(i.top, i.sub))}>
        <div className="grow">
          <div className="name">{i.name}</div>
          {i.parent ? <div className="muted small">{i.parent}</div> : null}
        </div>
        <div className="changebar">
          <span style={{ width: `${Math.max(4, (Math.abs(i.d) / max) * 100)}%`, background: color }} />
        </div>
        <div className="amt">
          <span className="num" style={{ color }}>{`${grow ? "+" : "−"}${formatShekelsWhole(Math.abs(i.d))}`}</span>
          {i.was === 0 && <div className="delta flat">חדש</div>}
        </div>
        <span className="chev muted">‹</span>
      </div>
    );
  };
  return (
    <div>
      <div className="card" style={{ textAlign: "center" }}>
        <div className="muted small">{`סך ההוצאות ${baseLabel}`}</div>
        <div className="num" style={{ fontSize: "1.5rem", fontWeight: 800, color: net > 0 ? "var(--expense)" : "var(--income)" }}>{`${net > 0 ? "+" : "−"}${formatShekelsWhole(Math.abs(net))}`}</div>
        <div className="muted small">{`${formatShekelsWhole(cur.expense)} לעומת ${formatShekelsWhole(wasTotal)}`}</div>
      </div>
      <h2>מה הגדיל את ההוצאות</h2>
      {up.length ? <div className="card rows">{up.map((i) => row(i, true))}</div> : <div className="empty">שום דבר לא עלה</div>}
      <h2>מה הקטין את ההוצאות</h2>
      {down.length ? <div className="card rows">{down.map((i) => row(i, false))}</div> : <div className="empty">שום דבר לא ירד</div>}
      <div className="muted small pad">הקשה על שורה פותחת אותה בפירוט הקטגוריות.</div>
    </div>
  );
}

// ---- Dashboard: fixed vs variable ----------------------------------------------------------------

const TotalRow = ({ label, value }: { label: string; value: number }) => (
  <div className="row totalrow">
    <div className="grow">
      <b>{label}</b>
    </div>
    <b className="num">{formatShekelsWhole(value)}</b>
  </div>
);

const dayMonth = (iso: string) => new Date(`${iso}T00:00:00`).toLocaleDateString("he-IL", { day: "numeric", month: "long" });

function Fixed({ ctx }: BodyProps) {
  const inP = ctx.txs.filter((t) => contains(ctx.period, t.date));
  const expenses = inP.filter((t) => t.amount_agorot < 0);
  const fixedTx = expenses.filter(isFixed);
  const fixedSum = -fixedTx.reduce((s, t) => s + t.amount_agorot, 0);
  const total = -expenses.reduce((s, t) => s + t.amount_agorot, 0);
  const variable = total - fixedSum;
  const income = inP.filter((t) => t.amount_agorot > 0).reduce((s, t) => s + t.amount_agorot, 0);
  const free = income - fixedSum;

  // Each charge is listed once: what already went through (one row per repeating series) here,
  // and what is still ahead further down. Together they add up to the fixed total above.
  const bySeries = new Map<number, { name: string; sum: number }>();
  for (const t of fixedTx.filter((t) => !t.upcoming)) {
    const k = t.recurring_series_id!;
    const g = bySeries.get(k) ?? { name: t.note || catPath(ctx, t.category_id), sum: 0 };
    g.sum += -t.amount_agorot;
    bySeries.set(k, g);
  }
  const seriesRows = [...bySeries.values()].sort((a, b) => b.sum - a.sum);
  const paidSum = seriesRows.reduce((s, r) => s + r.sum, 0);
  const upcoming = inP.filter((t) => t.upcoming).sort((a, b) => a.date.localeCompare(b.date));
  const upcomingOut = -upcoming.filter((t) => t.amount_agorot < 0).reduce((s, t) => s + t.amount_agorot, 0);

  // Month-end forecast: variable spending continues at its pace so far; fixed and income are what is known.
  let forecast: ReactNode = null;
  if (ctx.period.kind === "month" && isPartial(ctx.period)) {
    const day = new Date().getDate(), n = daysInMonth(ctx.period.month);
    const varSoFar = -expenses.filter((t) => !isFixed(t)).reduce((s, t) => s + t.amount_agorot, 0);
    const projVar = (varSoFar / day) * n;
    const bal = income - fixedSum - projVar;
    forecast = (
      <div>
        <h2>תחזית לסוף החודש</h2>
        <div className="card">
          <div className="rows">
            <div className="row">
              <div className="grow">{`הוצאה משתנה עד היום (${day} מתוך ${n} ימים)`}</div>
              {money(varSoFar)}
            </div>
            <div className="row">
              <div className="grow">בקצב הזה, עד סוף החודש</div>
              {money(Math.round(projVar))}
            </div>
            <div className="row">
              <div className="grow">
                <b>💡 יתרה צפויה בסוף החודש</b>
              </div>
              <b className={`num ${bal < 0 ? "expense" : "income"}`}>{formatShekelsWhole(Math.round(bal))}</b>
            </div>
          </div>
          {day < 7 && <div className="muted small">מוקדם בחודש: התחזית גסה וצפויה להשתפר ככל שהחודש מתקדם.</div>}
          <div className="muted small">מבוססת על קצב ההוצאה המשתנה עד היום, החיובים החוזרים וההכנסות שידועים.</div>
        </div>
      </div>
    );
  }
  const pct = (v: number, of: number) => (of > 0 ? `${Math.round((v / of) * 100)}%` : "–");
  const varColor = "color-mix(in srgb, var(--brand) 35%, transparent)";
  return (
    <div>
      <div className="card">
        <div className="muted small">מתוך ההוצאות בתקופה</div>
        <div className="splitbar">
          <div style={{ width: `${total ? (fixedSum / total) * 100 : 0}%`, background: "#4f46e5" }} />
          <div style={{ flex: 1, background: varColor }} />
        </div>
        <div className="splitlegend">
          <span>
            {swatch("#4f46e5")}
            {"קבוע "}
            <b className="num">{formatShekelsWhole(fixedSum)}</b>
            {` (${pct(fixedSum, total)})`}
          </span>
          <span>
            {swatch(varColor)}
            {"משתנה "}
            <b className="num">{formatShekelsWhole(variable)}</b>
          </span>
        </div>
      </div>
      <div className="kpis">
        <Kpi label="הכנסה" value={money(income, "income")} sub={<Note text={`${pct(fixedSum, income)} מהן קבועות`} />} />
        <Kpi label="נשאר אחרי קבועות" value={money(free, free < 0 ? "expense" : "income")} sub={<Note text="לחיים ולחיסכון" />} />
      </div>
      <h2>חיובים קבועים ששולמו</h2>
      {seriesRows.length ? (
        <div className="card rows">
          {seriesRows.map((r, i) => (
            <div className="row" key={i}>
              <div className="grow">
                <div className="name">{`🔁 ${r.name}`}</div>
              </div>
              {money(r.sum)}
            </div>
          ))}
          {seriesRows.length > 1 && <TotalRow label="סה״כ ששולם" value={paidSum} />}
        </div>
      ) : (
        <div className="empty">{fixedTx.length ? "עוד לא שולמו חיובים קבועים בתקופה." : "אין הוצאות חוזרות בתקופה. הוצאה שמוגדרת כחוזרת תיחשב כאן קבועה."}</div>
      )}
      {upcoming.length ? (
        <div>
          <h2>צפוי עוד בתקופה</h2>
          <div className="card rows">
            {upcoming.map((t: Transaction, i) => (
              <div className="row upcoming" key={i}>
                <div className="grow">
                  <div className="name">{`⏳ ${t.note || catPath(ctx, t.category_id)}`}</div>
                  <div className="muted small">{dayMonth(t.date)}</div>
                </div>
                {money(Math.abs(t.amount_agorot), t.amount_agorot < 0 ? "expense" : "income")}
              </div>
            ))}
            {upcomingOut > 0 && upcoming.length > 1 && <TotalRow label="סה״כ חיובים צפויים" value={upcomingOut} />}
          </div>
        </div>
      ) : null}
      {forecast}
    </div>
  );
}

// ---- Dashboard: monthly rhythm -------------------------------------------------------------------

function Rhythm({ ctx }: BodyProps) {
  const p = ctx.period;
  const { from, to } = bounds(p);
  const today = todayIso();
  const end = to < today ? to : today;
  const parts: ReactNode[] = [];

  if (p.kind === "month") {
    const days = dayTotals(ctx, p.month);
    const elapsed = isPartial(p) ? new Date().getDate() : days.length;
    const shown = days.slice(0, elapsed);
    const nz = shown.filter((v) => v > 0).sort((a, b) => a - b);
    const scale = nz.length ? nz[Math.min(nz.length - 1, Math.floor(nz.length * 0.9))]! : 1;
    const lead = dayOfWeek(`${p.month}-01`); // grid starts on Sunday
    const peak = shown.reduce((m, v, i) => (v > shown[m]! ? i : m), 0);
    parts.push(
      <h2 key="h-daily">הוצאה יומית</h2>,
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
            const t = future ? 0 : Math.min(1, v / scale);
            const style: React.CSSProperties = {};
            if (!future && v > 0) {
              style.background = `color-mix(in srgb, var(--expense) ${Math.round(12 + t * 78)}%, transparent)`;
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
          כהה יותר = יותר הוצאה באותו יום. ללא חיובים חוזרים, כדי שהרגלים יבלטו.
        </div>
        {shown[peak]! > 0 && (
          <Insight ico="🛒" style={{ marginTop: 8 }}>
            {"היום עם ההוצאה הגדולה ביותר: ה-"}
            <b>{String(peak + 1)}</b>
            {" ("}
            {money(shown[peak]!)}
            {")."}
          </Insight>
        )}
      </div>,
    );

    // Cumulative spending through the month, against the comparison.
    const cum = (vs: number[]) => vs.reduce<number[]>((a, v, i) => (a.push((a[i - 1] ?? 0) + v), a), []);
    const bases = basePeriods(p, ctx.ins.cmp).periods.filter((b): b is { kind: "month"; month: string } => b.kind === "month");
    let ghost: number[] | undefined;
    if (bases.length) {
      const per = bases.map((b) => cum(dayTotals(ctx, b.month)));
      ghost = Array.from({ length: days.length }, (_, i) => per.reduce((s, c) => s + (c[Math.min(i, c.length - 1)] ?? 0), 0) / per.length);
    }
    parts.push(
      <h2 key="h-cum">הוצאה מצטברת במהלך החודש</h2>,
      <div className="card" key="cum">
        <Chart html={lineChart(cum(shown), days.map((_, i) => String(i + 1)), "var(--expense)", ghost, 5)} />
        {ghost ? (
          <Legend>
            <span>
              {swatch("var(--expense)")}
              החודש
            </span>
            <span>
              {swatch("var(--muted)")}
              {basePeriods(p, ctx.ins.cmp).label.replace(/^מול /, "")}
            </span>
          </Legend>
        ) : null}
      </div>,
    );
  } else {
    parts.push(
      <div className="muted small pad" key="note">
        מפת החום היומית זמינה כשבוחרים חודש בודד. להלן ממוצע לפי יום בשבוע לתקופה שנבחרה.
      </div>,
    );
  }

  // Average spending per weekday over the days of the period that have passed.
  const sums = Array<number>(7).fill(0), counts = Array<number>(7).fill(0);
  for (let d = from; d <= end; d = new Date(Date.parse(`${d}T00:00:00Z`) + 864e5).toISOString().slice(0, 10)) counts[dayOfWeek(d)]!++;
  for (const t of ctx.txs) if (t.amount_agorot < 0 && !isFixed(t) && t.date >= from && t.date <= end) sums[dayOfWeek(t.date)]! += -t.amount_agorot;
  const avg = sums.map((s, i) => (counts[i] ? s / counts[i]! : 0));
  const peakDay = avg.reduce((m, v, i) => (v > avg[m]! ? i : m), 0);
  parts.push(
    <h2 key="h-week">ממוצע לפי יום בשבוע</h2>,
    <div className="card" key="week">
      <Chart html={barChart({ labels: WEEKDAYS, tips: WEEKDAYS.map((d, i) => `${d}: ${formatShekelsWhole(avg[i]!)}`), series: [avg], colors: ["var(--expense)"], highlight: new Set([peakDay]), height: 90 })} />
      {avg[peakDay]! > 0 && (
        <Insight ico="📅" style={{ marginTop: 8 }}>
          {"יום "}
          <b>{WEEKDAYS[peakDay]}</b>
          {" הוא היום היקר ביותר: "}
          {money(Math.round(avg[peakDay]!))}
          {" בממוצע."}
        </Insight>
      )}
    </div>,
  );
  return <div>{parts}</div>;
}
