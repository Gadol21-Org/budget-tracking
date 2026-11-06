import { type ReactElement, useState } from "react";
import { api } from "../api";
import { LinkCard } from "../components/LinkCard";
import { go } from "../nav";
import { load } from "../sync";
import { state } from "../state";
import { EmailCard } from "./EmailCard";
import { openLog } from "./log";
import { openPeople } from "./people";

export function settingsView(): ReactElement {
  return <Settings />;
}

function Settings() {
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
      <h1>הגדרות</h1>
      <div className="card">
        <p className="muted">ייבוא מושך את כל ההיסטוריה מפעמונים ואינו יוצר כפילויות. ייצוא כותב עותק לגיליון Google.</p>
        <div className="actions" style={{ justifyContent: "flex-start" }}>
          <button className="btn" disabled={busy} onClick={importFromPaamonim}>
            ייבוא מפעמונים
          </button>
          <button className="btn" disabled={busy} onClick={exportToSheets}>
            ייצוא ל-Google Sheets
          </button>
        </div>
        <div className={out?.error ? "note error" : "note muted"}>{out?.text}</div>
      </div>
      <EmailCard status={state.email} />
      <LinkCard title="קטגוריות" text="הוספה ומחיקה של קטגוריות ותתי-קטגוריות." button="עריכת קטגוריות" onClick={() => go("categories")} />
      <LinkCard title="עסקים" text="עסקים ותתי-עסקים לצפייה ברווח והפסד, בלי להוציא את התנועות מסיכום משק הבית." button="ניהול עסקים" onClick={() => go("businesses")} />
      <LinkCard title="מטבעות" text="בחירת המטבעות שמוצגים בהוספת תנועה, וסדר הופעתם." button="ניהול מטבעות" onClick={() => go("currencies")} />
      <LinkCard
        title="היסטוריית שינויים"
        text="כל הוספה, עריכה ומחיקה של תנועות, קטגוריות והוצאות חוזרות, עם הערכים לפני ואחרי. נשמרת שנה אחת."
        button="צפייה בהיסטוריית שינויים"
        onClick={() => void openLog().catch(() => alert("טעינת ההיסטוריה נכשלה"))}
      />
      <LinkCard
        title="שמות משתמשים"
        text="מי ביצע כל שינוי נרשם בהיסטוריה לפי כתובת האימייל. אפשר לתת לכל כתובת שם תצוגה."
        button="ניהול שמות"
        onClick={() => void openPeople().catch(() => alert("טעינת הרשימה נכשלה"))}
      />
    </div>
  );
}
