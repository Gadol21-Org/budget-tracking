import { type ReactElement } from "react";
import { clickableProps } from "../components/clickable";
import { go } from "../nav";

/** An action of the business. Add a line here to give the actions page a new one; the page lays them out. */
interface BusinessAction {
  icon: string;
  title: string;
  text: string;
  open: () => void;
}

const ACTIONS: BusinessAction[] = [{ icon: "🏢", title: "ניהול מוצרים", text: "הוספה ושינוי שם, והוצאות משותפות.", open: () => go("products") }];

function ActionsPage() {
  return (
    <div className="bizhub">
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

export function actionsPage(): ReactElement {
  return <ActionsPage />;
}
