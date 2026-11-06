import { type ReactElement } from "react";
import { BackButton } from "../components/BackButton";
import { Money } from "../components/Money";
import { clickableProps } from "../components/clickable";
import { businessOf, summaryFigures } from "../business";
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
  { icon: "📊", title: "סקירה ורווח והפסד", text: "הכנסות, הוצאות, מגמות והשוואות, לכל העסק או לעסק אחד.", open: openBusinessDashboard },
  { icon: "🏢", title: "ניהול עסקים", text: "הוספה ושינוי שם של עסקים והוצאות משותפות.", open: () => go("businesses") },
];

const sign = (n: number) => (n < 0 ? "expense" : "income");

/** "Of which the businesses": profit of the businesses in the period; they stay in the household figures. */
function Overview() {
  const { total, per } = summaryFigures({ txs: state.txs, cats: state.cats, biz: state.biz, period: state.period });
  const any = state.txs.some((t) => businessOf(state.biz, state.cats, t) != null);
  if (!state.biz.businesses.length || !any) {
    return (
      <div className="card">
        <div className="muted">אין עדיין תנועות של עסקים בתקופה הזו.</div>
      </div>
    );
  }
  return (
    <div className="card bizcard clickable" {...clickableProps(openBusinessDashboard)}>
      <div className="head">
        <div className="grow">
          <div className="bizname">סקירת העסקים</div>
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

function BusinessHub({ back }: { back: () => void }) {
  return (
    <div className="bizhub">
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <BackButton label="→ סיכום" title="חזרה לסיכום" onClick={back} />
      </div>
      <h1>העסק</h1>
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

export function businessHub(props: { back: () => void }): ReactElement {
  return <BusinessHub {...props} />;
}
