import { el } from "./ui";
import { root } from "./state";
import { markLoaded, refresh, refreshing } from "./sync";

// iOS Safari ignores user-scalable=no and touch-action for pinch, but still fires these gesture events.
for (const g of ["gesturestart", "gesturechange"]) document.addEventListener(g, (e) => e.preventDefault());

// Pull to refresh (touch screens only): the page follows the finger from the top, the spinner rides
// along, a short haptic tick marks the threshold (navigator.vibrate; unavailable on iOS Safari),
// and release glides back with an eased transition.
export const PULL_THRESHOLD = 72;

export const PULL_MAX = 120;

export const spinner = el("div", { className: "ptr" }, el("div", { className: "ptr-dot" }));

document.body.append(spinner);

export let pullStart: number | undefined;

export let pullDist = 0;

export let armed = false;

export function setPull(dist: number) {
  const r = document.documentElement.style;
  r.setProperty("--pull", `${dist}px`);
  r.setProperty("--pull-p", String(Math.min(1, dist / PULL_THRESHOLD)));
}

export function glideTo(dist: number) {
  root.classList.add("gliding");
  setPull(dist);
}

document.addEventListener(
  "touchstart",
  (e) => {
    pullStart = window.scrollY <= 0 && e.touches.length === 1 && !document.querySelector("dialog[open]") && !refreshing ? e.touches[0].clientY : undefined;
    pullDist = 0;
    armed = false;
    root.classList.remove("gliding");
  },
  { passive: true },
);

document.addEventListener(
  "touchmove",
  (e) => {
    if (pullStart === undefined) return;
    const dy = e.touches[0].clientY - pullStart;
    if (dy <= 0 || window.scrollY > 0) {
      if (pullDist === 0) pullStart = undefined;
      else setPull((pullDist = 0));
      return;
    }
    e.preventDefault();
    // Rubber band: resistance grows as the page is pulled further.
    pullDist = PULL_MAX * (1 - Math.exp(-dy / (PULL_MAX * 1.6)));
    setPull(pullDist);
    if (!armed && pullDist >= PULL_THRESHOLD) navigator.vibrate?.(12);
    armed = pullDist >= PULL_THRESHOLD;
    spinner.classList.toggle("ready", armed);
  },
  { passive: false },
);

export async function endPull() {
  if (pullStart === undefined) return;
  pullStart = undefined;
  if (!armed) return glideTo(0);
  spinner.classList.add("spin");
  glideTo(PULL_THRESHOLD * 0.8);
  markLoaded();
  const [ok] = await Promise.all([refresh(), new Promise((r) => setTimeout(r, 600))]);
  if (!ok) navigator.vibrate?.([30, 40, 30]);
  spinner.classList.remove("spin", "ready");
  glideTo(0);
  armed = false;
}

document.addEventListener("touchend", endPull);

document.addEventListener("touchcancel", endPull);
