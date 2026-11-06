import { type ReactElement } from "react";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { productOf, summaryFigures } from "../business";
import { go, openBusinessDashboard } from "../nav";
import { state } from "../state";
import { ConnectedMonthBar } from "./month";

/** An action on the business page. Add a line here to give the page a new action; the menu lays them out. */
interface BusinessAction {
  icon: string;
  title: string;
  text: string;
  open: () => void;
}

const ACTIONS: BusinessAction[] = [
  { icon: "📊", title: "סקירה ורווח והפסד", text: "הכנסות, הוצאות, מגמות והשוואות, לכל העסק או למוצר אחד.", open: openBusinessDashboard },
  { icon: "🏢", title: "ניהול מוצרים", text: "הוספה ושינוי שם, והוצאות משותפות.", open: () => go("products") },
];

const sign = (n: number) => (n < 0 ? "expense" : "income");

/** "Of which the products": profit of the products in the period; they stay in the household figures. */
function Overview() {
  const { total, per } = summaryFigures({ txs: state.txs, cats: state.cats, products: state.products, period: state.period });
  const any = state.txs.some((t) => productOf(state.products, state.cats, t) != null);
  if (!state.products.products.length || !any) {
    return (
      <div className="card">
        <div className="muted">אין עדיין תנועות של מוצרים בתקופה הזו.</div>
      </div>
    );
  }
  return (
    <div className="card bizcard clickable" {...clickableProps(openBusinessDashboard)}>
      <div className="head">
        <div className="grow">
          <div className="bizname">סקירת המוצרים</div>
          <div className="muted small">כלול גם בסיכום משק הבית</div>
        </div>
        <b>
          <span>רווח </span>
          <Money agorot={total.profit} cls={sign(total.profit)} />
        </b>
        <span className="muted">›</span>
      </div>
      {per.map(({ b, f }) => (
        <div className="row sub" key={b.id}>
          <div className="grow">{b.is_general ? `${b.name} (משותף)` : b.name}</div>
          <Money agorot={f.profit} cls={sign(f.profit)} />
        </div>
      ))}
    </div>
  );
}

function BusinessHub() {
  return (
    <div className="bizhub">
      <ConnectedMonthBar />
      <Overview />
      <h2 className="bizmenu-title">פעולות</h2>
      <div className="bizmenu">
        {ACTIONS.map((a) => (
          <div key={a.title} className="card bizaction clickable" {...clickableProps(a.open)}>
            <span className="ico" aria-hidden="true">{a.icon}</span>
            <div className="grow">
              <div className="bizname">{a.title}</div>
              <div className="muted small">{a.text}</div>
            </div>
            <span className="muted">›</span>
          </div>
        ))}
      </div>
    </div>
  );
}

export function businessHub(): ReactElement {
  return <BusinessHub />;
}
