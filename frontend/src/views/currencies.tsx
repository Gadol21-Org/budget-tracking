import { type PointerEvent as ReactPointerEvent, type ReactElement, useEffect, useRef, useState } from "react";
import { api, currencyInfo, type Currency } from "../api";
import { BackButton } from "../components/BackButton";
import { LRM, enabledCurrencies, loadEnabledCurrencies, rememberCurrencies, supportedCurrencies } from "../currencies";

export interface CurrenciesCtx {
  /** Back to the settings screen. */
  back: () => void;
}

/** The settings page: add currencies from the full list and put the added ones in order. */
export function currenciesView(ctx: CurrenciesCtx): ReactElement {
  return <CurrenciesScreen back={ctx.back} />;
}

const reorder = (list: Currency[], from: number, to: number) => {
  const next = [...list];
  next.splice(to, 0, ...next.splice(from, 1));
  return next;
};

function CurrencyLabel({ c }: { c: Currency }) {
  const i = currencyInfo(c);
  return (
    <div className="grow">
      <div className="name">{i.symbol === i.code ? i.name : `${LRM}${i.symbol}${LRM} ${i.name}`}</div>
      <div className="muted">{i.code}</div>
    </div>
  );
}

function CurrenciesScreen({ back }: { back: () => void }) {
  const [list, setList] = useState<Currency[]>(enabledCurrencies());
  const [supported, setSupported] = useState<Currency[]>(supportedCurrencies());
  const [msg, setMsg] = useState("");
  const [query, setQuery] = useState("");
  /** The row whose drag handle should take the focus after the list changed (keyboard reordering). */
  const [focusIndex, setFocusIndex] = useState<number | null>(null);
  const addedBox = useRef<HTMLDivElement>(null);

  // Another device may have changed the list: show the server's copy.
  useEffect(() => {
    void loadEnabledCurrencies().then(() => {
      setList(enabledCurrencies());
      setSupported(supportedCurrencies());
    });
  }, []);
  useEffect(() => {
    if (focusIndex != null) addedBox.current?.querySelector<HTMLElement>(`.row[data-i="${focusIndex}"] .draghandle`)?.focus();
  }, [list, focusIndex]);

  /** Save a new list; on failure show the error and go back to what the server has. */
  const save = async (next: Currency[], focus?: number) => {
    const before = list;
    rememberCurrencies(next);
    setList(next);
    setFocusIndex(focus ?? null);
    setMsg("");
    try {
      const saved = (await api.saveCurrencySettings(next)).enabled;
      rememberCurrencies(saved);
      setList(saved);
    } catch (e) {
      rememberCurrencies(before);
      setList(before);
      setMsg(`השמירה נכשלה: ${e instanceof Error ? e.message : e}`);
    }
  };

  /** A handle that drags its row (mouse, touch and pen alike: pointer events, the handle opts out of scrolling); arrow keys on the focused handle move it too. */
  const dragStart = (i: number) => (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (e.button > 0) return;
    e.preventDefault();
    const h = e.currentTarget;
    h.setPointerCapture(e.pointerId);
    const rows = [...(addedBox.current?.querySelectorAll<HTMLElement>(".row[data-i]") ?? [])];
    const row = rows[i]!;
    const rects = rows.map((r) => r.getBoundingClientRect());
    const startY = e.clientY;
    const step = rects.length > 1 ? rects[1]!.top - rects[0]!.top : rects[i]!.height;
    let to = i;
    row.classList.add("dragging");
    const move = (ev: PointerEvent) => {
      const dy = Math.max(rects[0]!.top - rects[i]!.top, Math.min(rects[rects.length - 1]!.top - rects[i]!.top, ev.clientY - startY));
      row.style.transform = `translateY(${dy}px)`;
      // Rows are equally tall, so the new place is the number of whole rows the handle has travelled (the clamp above lets it reach either end).
      to = Math.max(0, Math.min(rows.length - 1, i + Math.round(dy / step)));
      rows.forEach((r, k) => {
        if (k !== i) r.style.transform = i < k && k <= to ? `translateY(${-step}px)` : to <= k && k < i ? `translateY(${step}px)` : "";
      });
    };
    const end = (ev: PointerEvent) => {
      h.removeEventListener("pointermove", move);
      h.removeEventListener("pointerup", end);
      h.removeEventListener("pointercancel", end);
      rows.forEach((r) => ((r.style.transform = ""), r.classList.remove("dragging")));
      if (ev.type === "pointerup" && to !== i) void save(reorder(list, i, to));
    };
    h.addEventListener("pointermove", move);
    h.addEventListener("pointerup", end);
    h.addEventListener("pointercancel", end);
  };

  const ils = currencyInfo("ILS");
  const q = query.trim().toLowerCase();
  const all = supported.length ? supported : list;
  const rest = all.filter((c) => c !== "ILS" && !list.includes(c)).filter((c) => !q || `${currencyInfo(c).symbol} ${currencyInfo(c).name} ${c}`.toLowerCase().includes(q));

  return (
    <div>
      <div className="actions" style={{ justifyContent: "flex-start" }}>
        <BackButton onClick={back} />
      </div>
      <h1>מטבעות</h1>
      <div className="card">
        <h2>מטבעות פעילים</h2>
        <p className="muted">רק המטבעות כאן מוצגים בהוספה ועריכה של תנועות, לפי הסדר הזה. גוררים בידית ⠿ כדי לשנות את הסדר. תנועות שכבר נשמרו במטבע שהוסר ממשיכות להופיע כרגיל.</p>
        <div className="rows" ref={addedBox}>
          <div className="row fixed">
            <div className="grow">
              <div className="name">{`${ils.symbol} ${ils.name}`}</div>
              <div className="muted">ILS · ברירת מחדל, תמיד זמין</div>
            </div>
          </div>
          {list.length ? null : <div className="empty">לא נוספו מטבעות נוספים</div>}
          {list.map((c, i) => (
            <div className="row" data-i={i} key={c} draggable={false}>
              <button
                type="button"
                className="btn iconbtn draghandle"
                title="גרירה לשינוי סדר"
                aria-label={`שינוי סדר: ${currencyInfo(c).name}. חצים למעלה ולמטה מזיזים`}
                onPointerDown={dragStart(i)}
                onKeyDown={(e) => {
                  const to = e.key === "ArrowUp" ? i - 1 : e.key === "ArrowDown" ? i + 1 : -1;
                  if (to < 0 || to >= list.length) return;
                  e.preventDefault();
                  void save(reorder(list, i, to), to);
                }}
              >
                ⠿
              </button>
              <CurrencyLabel c={c} />
              <button type="button" className="btn iconbtn" title="הסרה" aria-label="הסרה" onClick={() => void save(list.filter((x) => x !== c))}>
                ✕
              </button>
            </div>
          ))}
        </div>
        <div className="note error">{msg}</div>
      </div>
      <div className="card">
        <h2>הוספת מטבע</h2>
        <input type="search" className="cursearch" placeholder="חיפוש מטבע לפי שם או קוד…" autoComplete="off" spellCheck={false} value={query} onChange={(e) => setQuery(e.target.value)} />
        <div className="muted">{`${rest.length} מטבעות זמינים`}</div>
        <div className="rows curlist">
          {rest.length ? (
            rest.map((c) => (
              <div className="row" key={c}>
                <CurrencyLabel c={c} />
                <button type="button" className="btn" style={{ whiteSpace: "nowrap" }} title={`הוספת ${currencyInfo(c).name}`} aria-label={`הוספת ${currencyInfo(c).name}`} onClick={() => void save([...list, c])}>
                  הוספה
                </button>
              </div>
            ))
          ) : (
            <div className="empty">{q ? "אין תוצאות" : "כל המטבעות נוספו"}</div>
          )}
        </div>
      </div>
    </div>
  );
}
