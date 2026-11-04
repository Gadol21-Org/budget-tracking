import { useState } from "react";
import { api, type LogUser } from "../api";
import { BackButton } from "../components/BackButton";
import { mountReact } from "../components/mount";
import { go, goBack } from "../nav";
import { render } from "../render";
import { state } from "../state";

/** Emails that made a recorded change, each with an editable display name shown in the change history. */
export function peopleView(): HTMLElement {
  return mountReact(<People users={state.logUsers} />);
}

function People({ users }: { users: LogUser[] }) {
  return (
    <div>
      <div className="card filterbar">
        <BackButton onClick={() => goBack("settings")} />
        <div className="grow">
          <div className="name">שמות משתמשים</div>
          <div className="muted">כתובות שביצעו שינוי בעבר</div>
        </div>
      </div>
      {users.length ? (
        <div className="card rows">
          {users.map((u) => (
            <PersonRow key={u.email} user={u} />
          ))}
        </div>
      ) : (
        <div className="empty">עוד לא נרשם שינוי עם כתובת אימייל</div>
      )}
    </div>
  );
}

function PersonRow({ user }: { user: LogUser }) {
  const [name, setName] = useState(user.name);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  const save = async () => {
    setBusy(true);
    try {
      await api.setLogUserName(user.email, name);
      user.name = name.trim();
      setNote("נשמר");
    } catch {
      setNote("השמירה נכשלה");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="row">
      <div className="grow">
        <div className="name" dir="ltr" style={{ textAlign: "start" }}>
          {user.email}
        </div>
        <div className="actions" style={{ justifyContent: "flex-start", gap: 8, flexWrap: "nowrap" }}>
          <input type="text" value={name} placeholder="שם תצוגה" maxLength={60} aria-label={`שם תצוגה עבור ${user.email}`} style={{ flex: 1, minWidth: 0 }} onChange={(e) => setName(e.target.value)} />
          <button className="btn" disabled={busy} onClick={save}>
            שמירה
          </button>
        </div>
        <span className="muted">{note}</span>
      </div>
    </div>
  );
}

export async function openPeople() {
  state.logUsers = await api.logUsers();
  go("people");
  render();
}
