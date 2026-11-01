import { el } from "./ui";
import { type InsightsNav } from "./insights";
import { type Screen, type View, state } from "./state";
import { render } from "./render";
import { load } from "./sync";

export function screenState(extra: Partial<Screen> = {}): Screen {
  return { view: state.view, filter: state.filter, ins: { dash: state.insights.dash, side: state.insights.side, cat: state.insights.cat, sub: state.insights.sub }, n: history.state?.n ?? 0, ...extra };
}

history.replaceState(screenState({ n: 0 }), "");

/** Go to a screen as a new history entry. */
export function go(view: View, filter?: { id: number | null }) {
  if (view === state.view && filter?.id === state.filter?.id && (filter === undefined) === (state.filter === undefined)) return;
  state.view = view;
  state.filter = filter;
  history.pushState(screenState({ n: (history.state?.n ?? 0) + 1 }), "");
  render();
}

/** The in-app back buttons: the same as the phone's back, or a fresh step when there is nothing to go back to. */
export function goBack(view: View) {
  if ((history.state?.n ?? 0) > 0 && !history.state?.dlg) history.back();
  else {
    state.view = view;
    state.filter = undefined;
    render();
  }
}

/** A modal takes a history entry while open: back closes it, and closing it any other way drops the entry. */
export function trackDialog(dlg: HTMLDialogElement) {
  history.pushState(screenState({ dlg: true }), "");
  dlg.addEventListener("close", () => {
    if (!dlg.dataset.viaBack && history.state?.dlg) history.back();
  });
}

window.addEventListener("popstate", (e) => {
  const open = [...document.querySelectorAll<HTMLDialogElement>("dialog[open]")];
  const top = open[open.length - 1];
  if (top) {
    top.dataset.viaBack = "1";
    top.close();
    return;
  }
  const st = e.state as Screen | null;
  if (st?.dlg) return;
  state.view = st?.view ?? "month";
  state.filter = st?.filter;
  restoreInsights(st?.ins);
  render();
});

export function restoreInsights(ins?: InsightsNav) {
  state.insights.dash = ins?.dash ?? "overview";
  state.insights.side = ins?.side;
  state.insights.cat = ins?.cat;
  state.insights.sub = ins?.sub;
}

/** Move inside the insights tab. A drill-down step is a history entry (back goes up a level); switching dashboards replaces it. */
export function navigateInsights(patch: Partial<InsightsNav>, mode: "push" | "replace") {
  Object.assign(state.insights, { cat: undefined, sub: undefined }, patch);
  const st = screenState(mode === "push" ? { n: (history.state?.n ?? 0) + 1 } : {});
  if (mode === "push") history.pushState(st, "");
  else history.replaceState(st, "");
  window.scrollTo(0, 0);
  render();
}

export function showCategory(id: number | null) {
  go("list", { id });
}

export function manageCtx() {
  return { biz: state.biz, cats: state.cats, txs: state.txs, openDialog, reload: load, back: () => goBack("settings") };
}

/** Open a modal built by `build`, which gets a way to close it. */
export function openDialog(build: (close: () => void) => HTMLElement) {
  const dlg = el("dialog");
  dlg.append(build(() => dlg.close()));
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  trackDialog(dlg);
  dlg.showModal();
}
