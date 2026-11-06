/** Google Analytics 4 measurement ID. It is public by design: it ships in every page's JavaScript. */
const GA_MEASUREMENT_ID = "G-F459M13TGD";

type Gtag = (...args: unknown[]) => void;

let enabled = false;
let lastView: string | undefined;

/**
 * Google Analytics, for the production build only.
 * The flag VITE_ENABLE_ANALYTICS is set at image build time by the production workflow (deploy.yml);
 * staging and local dev never set it, so gtag.js is not even downloaded there.
 *
 * Privacy: the app shows financial data, so only a page view per screen is sent, with a fixed
 * screen name as its path (e.g. "/insights"). Never amounts, categories, notes, ids or query strings.
 */
export function startAnalytics() {
  if (import.meta.env.VITE_ENABLE_ANALYTICS !== "true" || enabled) return;
  enabled = true;
  const w = window as unknown as { dataLayer?: unknown[]; gtag?: Gtag };
  w.dataLayer = w.dataLayer ?? [];
  // gtag.js reads `arguments` objects from dataLayer, so this must be a plain function, not an arrow.
  w.gtag = function () {
    // eslint-disable-next-line prefer-rest-params
    w.dataLayer!.push(arguments);
  };
  w.gtag("js", new Date());
  // Page views are sent by hand per screen (the app is one URL); the automatic one would send the raw URL.
  w.gtag("config", GA_MEASUREMENT_ID, { send_page_view: false, allow_google_signals: false, allow_ad_personalization_signals: false });
  const script = document.createElement("script");
  script.async = true;
  script.src = `https://www.googletagmanager.com/gtag/js?id=${GA_MEASUREMENT_ID}`;
  document.head.append(script);
}

/** Record that a screen was opened. Repeats of the same screen (re-renders) are ignored. */
export function trackScreen(view: string) {
  if (!enabled || view === lastView) return;
  lastView = view;
  (window as unknown as { gtag: Gtag }).gtag("event", "page_view", { page_title: view, page_location: `${location.origin}/${view}`, page_path: `/${view}` });
}
