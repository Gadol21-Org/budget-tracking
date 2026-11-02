import { el } from "../ui";
import { api } from "../api";
import { go, goBack } from "../nav";
import { state } from "../state";
import { render } from "../render";

/** Emails that made a recorded change, each with an editable display name shown in the change history. */
export function peopleView(): HTMLElement {
  const back = el("button", { className: "btn", textContent: "→ הגדרות", title: "חזרה להגדרות" });
  back.onclick = () => goBack("settings");
  const rows = state.logUsers.map((u) => {
    const input = el("input", { type: "text", value: u.name, placeholder: "שם תצוגה", maxLength: 60, style: "flex:1;min-width:0" });
    input.setAttribute("aria-label", `שם תצוגה עבור ${u.email}`);
    const save = el("button", { className: "btn", textContent: "שמירה" });
    const note = el("span", { className: "muted" });
    save.onclick = async () => {
      save.disabled = true;
      try {
        await api.setLogUserName(u.email, input.value);
        u.name = input.value.trim();
        note.textContent = "נשמר";
      } catch {
        note.textContent = "השמירה נכשלה";
      } finally {
        save.disabled = false;
      }
    };
    return el("div", { className: "row" }, el("div", { className: "grow" }, el("div", { className: "name", textContent: u.email, dir: "ltr", style: "text-align:start" }), el("div", { className: "actions", style: "justify-content:flex-start;gap:8px;flex-wrap:nowrap" }, input, save), note));
  });
  return el(
    "div",
    {},
    el("div", { className: "card filterbar" }, back, el("div", { className: "grow" }, el("div", { className: "name", textContent: "שמות משתמשים" }), el("div", { className: "muted", textContent: "כתובות שביצעו שינוי בעבר" }))),
    rows.length ? el("div", { className: "card rows" }, ...rows) : el("div", { className: "empty", textContent: "עוד לא נרשם שינוי עם כתובת אימייל" }),
  );
}

export async function openPeople() {
  state.logUsers = await api.logUsers();
  go("people");
  render();
}
