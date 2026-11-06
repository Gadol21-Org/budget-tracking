// Cloudflare Email Worker: hands every message sent to the routed address to the budget app.
// Paste it into a Worker (Workers & Pages > Create > Hello World > Edit code), see deploy/EMAIL.md.
//
// Variables (Settings > Variables and Secrets):
//   INBOUND_URL      https://<your-host>/api/email/inbound
//   INBOUND_SECRET   (secret) the same value as EMAIL_INBOUND_SECRET on the server
//   FALLBACK_ADDRESS (optional) a verified destination address; mail is forwarded there when the app is unreachable
//   ALLOWED_FROM     (optional) comma-separated sender addresses that are accepted; anything else is dropped.
//                    Matches the envelope sender or the From header. Empty or unset: no filtering.
const MAX_BYTES = 1_500_000;

// The address inside "Name <address>" (or a bare address), lower case.
const addressOf = (v) => {
  const m = /<([^>]+)>/.exec(v || "");
  return (m ? m[1] : v || "").trim().toLowerCase();
};

const allowed = (message, env) => {
  const list = (env.ALLOWED_FROM || "").split(",").map((a) => a.trim().toLowerCase()).filter(Boolean);
  if (!list.length) return true;
  return [message.from, message.headers.get("from")].some((v) => list.includes(addressOf(v)));
};

export default {
  async email(message, env) {
    // Not from a listed sender: drop it without a bounce (a bounce would confirm the address exists).
    if (!allowed(message, env)) return;
    if (message.rawSize > MAX_BYTES) {
      message.setReject("Message too large");
      return;
    }
    const raw = await new Response(message.raw).arrayBuffer();
    let status = 0;
    try {
      const res = await fetch(env.INBOUND_URL, {
        method: "POST",
        headers: { authorization: `Bearer ${env.INBOUND_SECRET}`, "content-type": "message/rfc822" },
        body: raw,
      });
      status = res.status;
    } catch (e) {
      status = 599;
    }
    // 200: handled. 422: the app refused it (not an email, not from the expected forwarder): drop it quietly.
    if (status === 200 || status === 422) return;
    // The app is down or misconfigured: keep the mail instead of losing it.
    if (env.FALLBACK_ADDRESS) {
      await message.forward(env.FALLBACK_ADDRESS);
      return;
    }
    throw new Error(`budget app answered ${status}`);
  },
};
