import { type ReactElement, useState } from "react";
import { api } from "../api";
import { BackButton } from "../components/BackButton";
import { go, goBack } from "../nav";
import { enabledCurrencies } from "../currencies";
import { state } from "../state";
import { load } from "../sync";
import { openEmailInbox } from "./emailInbox";
import { openLog } from "./log";
import { openAiCost } from "./aiCost";
import { openPeople } from "./people";
import { SettingsRow as Row } from "../components/SettingsRow";

export function settingsView(): ReactElement {
  return <Settings />;
}

export function dataTransferView(): ReactElement {
  return <DataTransfer />;
}

/** What the Sumit row shows: receipts waiting for a category, new ones, or nothing. */
function sumitValue(): string {
  const { held, unread } = state.sumit;
  return held ? `${held} ממתינות` : unread ? `${unread} חדשות` : "";
}

function Settings() {
  return (
    <div>
      <h1>הגדרות</h1>
      <div className="sgroup">
        <Row icon="▦" color={["#ffb340", "#ff8a00"]} label="קטגוריות" value={String(state.cats.length)} onClick={() => go("categories")} />
        <Row icon="₪" color={["#5fe27f", "#22b84a"]} label="מטבעות" value={enabledCurrencies().join(" · ")} onClick={() => go("currencies")} />
      </div>
      <div className="sgroup">
        <Row icon="🧾" color={["#2dd4bf", "#0f766e"]} label="קבלות מ-Sumit" value={sumitValue()} onClick={() => go("sumit")} />
        {state.email && <Row icon="✉" color={["#e879f9", "#a21caf"]} label="חיובים מדוא״ל" value={state.email.pending ? `${state.email.pending} ממתינות` : ""} onClick={() => void openEmailInbox().catch(() => alert("טעינת ההצעות נכשלה"))} />}
        <Row icon="⇅" color={["#5ab4ff", "#0a6cff"]} label="ייבוא וייצוא" onClick={() => go("dataTransfer")} />
      </div>
      <div className="sgroup">
        <Row icon="↺" color={["#b4b4ba", "#7c7c82"]} label="היסטוריית שינויים" onClick={() => void openLog().catch(() => alert("טעינת ההיסטוריה נכשלה"))} />
        <Row icon="$" color={["#ffd24a", "#e0a000"]} label="עלות AI" onClick={() => void openAiCost().catch(() => alert("טעינת העלות נכשלה"))} />
        <Row icon="☺" color={["#cf7bf2", "#9b3fd0"]} label="שמות משתמשים" onClick={() => void openPeople().catch(() => alert("טעינת הרשימה נכשלה"))} />
      </div>
    </div>
  );
}

function DataTransfer() {
  const [out, setOut] = useState<{ text: string; error: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const run = (work: () => Promise<string>) => async () => {
    setBusy(true);
    setOut({ text: "מבצע…", error: false });
    try {
      setOut({ text: await work(), error: false });
      await load();
    } catch (e) {
      setOut({
        error: true,
        text:
          e instanceof TypeError
            ? "שגיאה: החיבור לשרת נותק לפני סיום הפעולה. ייתכן שהיא עדיין רצה ברקע; נסו שוב בעוד דקה."
            : `שגיאה: ${e instanceof Error ? e.message : e}`,
      });
    } finally {
      setBusy(false);
    }
  };
  const importFromPaamonim = run(async () => {
    const s = await api.importPaamonim("2000-01-01", new Date().toISOString().slice(0, 10));
    return `יובאו ${s.transactions_added} תנועות חדשות (${s.transactions_skipped} כבר קיימות), ${s.categories_added} קטגוריות חדשות.`;
  });
  const exportToSheets = run(async () => {
    const s = await api.exportSheets();
    return `יוצאו ${s.transactions} תנועות.`;
  });
  return (
    <div>
      <div className="card filterbar">
        <BackButton onClick={() => goBack("settings")} />
        <div className="grow">
          <div className="name">ייבוא וייצוא</div>
        </div>
      </div>
      <div className="sgroup">
        <Row label="ייבוא מפעמונים" action disabled={busy} onClick={importFromPaamonim} />
      </div>
      <div className="sfoot">מושך את כל ההיסטוריה מפעמונים ואינו יוצר כפילויות.</div>
      <div className="sgroup">
        <Row label="ייצוא ל-Google Sheets" action disabled={busy} onClick={exportToSheets} />
      </div>
      <div className="sfoot">כותב עותק לגיליון Google.</div>
      <div className={out?.error ? "note error" : "note muted"}>{out?.text}</div>
    </div>
  );
}
