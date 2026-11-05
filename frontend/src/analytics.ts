import Clarity from "@microsoft/clarity";

const CLARITY_PROJECT_ID = "z8xczfjhsy";

/**
 * Microsoft Clarity session analytics, for the production build only.
 * The flag VITE_ENABLE_CLARITY is set at image build time by the production workflow (deploy.yml);
 * staging and local dev never set it, so Clarity is not even started there.
 * Everything on the page is masked through data-clarity-mask on <body> in index.html,
 * so no amounts, names or other text is recorded.
 */
export function startAnalytics() {
  if (import.meta.env.VITE_ENABLE_CLARITY !== "true") return;
  Clarity.init(CLARITY_PROJECT_ID);
}
