# Email to transactions

Mail that Gmail forwards to an address on your own domain is read by an AI, which says which transaction each message describes. The answers wait in **Settings > Suggestions from email**. A suggestion becomes a transaction only when you open it, check it in the normal add dialog and save.

```
Gmail filter  --forwards-->  budget-xxxx@your-domain  (Cloudflare Email Routing)
   --> Email Worker (deploy/email-worker.js)  --POST raw email + secret-->  /api/email/inbound
   --> Gemini reads it  -->  review queue in the app
```

There is no Google OAuth, no Gmail API, no token to expire and nothing to verify with Google. The app holds no credential for any mailbox and only ever sees what your Gmail filter forwards.

## What is stored and who sees it
- Cloudflare processes the forwarded mail in transit (Email Routing and the Worker); it is not stored by the Worker.
- The text of each message (up to 12,000 characters; HTML is turned into text; attachments are ignored) is sent to the Google AI API (`gemini-3.5-flash-lite`). On the free tier Google may use prompts to improve its products; enabling billing on the key's project avoids that.
- The app keeps, per message: sender, subject, date, the AI's answer and a 600 character excerpt for you to check. The email's HTML (up to 300 KB) is kept too, for the "צפייה במייל" button; it is shown in a sandboxed frame without scripts, and images from the internet load (the sender may see that you opened it). The excerpt and the HTML are deleted when you accept or reject the suggestion. Attachments are ignored and never stored. A message the AI could not read yet is kept in full (so it can be retried) until it is read or removed.
- Anyone who learns the routed address could mail it. They can only add suggestions for you to reject, never transactions. To make that harder: use an unguessable address (`budget-x7k2q9@...`), list your own addresses in the Worker's `ALLOWED_FROM` if you forward by hand, and the server caps AI calls per day (`EMAIL_MAX_PER_DAY`, default 100; the rest wait).

## Setup

### 1. Gemini key
Create an API key in [Google AI Studio](https://aistudio.google.com/apikey) (the Google AI / Gemini Developer API, not Vertex AI).

### 2. Server settings (`.env`)
```
EMAIL_INBOUND_SECRET=<long random string, e.g. openssl rand -hex 32>
GEMINI_API_KEY=...
# Optional
# EMAIL_AI_MODEL=gemini-3.5-flash-lite
# EMAIL_MAX_PER_DAY=100
```
Restart the app. Without `EMAIL_INBOUND_SECRET` the feature and its endpoint are off (the endpoint answers 404).

### 3. Cloudflare Access
The Worker cannot sign in to Access, so make exactly one path public, like `/assets`: Zero Trust > Access > Applications > the budget app > add a policy **Bypass** (Everyone) for the path `/api/email/inbound`. The endpoint refuses every request without `Authorization: Bearer <EMAIL_INBOUND_SECRET>` (compared in constant time) and reads nothing before that check.

### 4. Cloudflare Email Routing and the Worker
1. **Email Routing:** Cloudflare dashboard > your domain > Email > Email Routing > enable it. It adds MX records to the domain; if the domain already receives mail elsewhere, use a subdomain such as `mail.your-domain` (Email Routing supports them) so nothing existing is touched.
2. **Worker:** Workers & Pages > Create > Hello World, name it `budget-email`, paste `deploy/email-worker.js`, deploy. Under Settings > Variables and Secrets add `INBOUND_URL` = `https://<your-host>/api/email/inbound`, `INBOUND_SECRET` (type **Secret**) = the same value as on the server, and optionally `FALLBACK_ADDRESS` (a destination address verified under Email Routing > Destination addresses; mail is forwarded there when the app is down, so nothing is lost). Also add `ALLOWED_FROM` = your addresses, comma-separated (as a Secret, so they stay out of the repo): the Worker silently drops mail whose envelope sender and From header are both not in the list. Note this checks the message's own sender: a Gmail filter that forwards a shop's receipt keeps the shop as the sender, so such mail is dropped. It fits mail you forward by hand (the sender is you); for filter-forwarded mail leave `ALLOWED_FROM` empty (Gmail does not authenticate who forwarded a message, so nothing can safely tell it apart from mail sent to the address; keep the address private).
3. **Address:** Email Routing > Routing rules > Create address: custom address `budget-<random>`, action **Send to a Worker** > `budget-email`.

### 5. Gmail
1. **Forwarding address:** Gmail > Settings > See all settings > Forwarding and POP/IMAP > Add a forwarding address > `budget-<random>@your-domain`. Gmail sends a confirmation code *to that address*, so first point the routing rule at your own inbox (Action: **Send to an email** > a verified destination address), read the code in your inbox, confirm it in Gmail, then switch the rule back to **Send to a Worker**.
2. **Filter:** Settings > Filters and Blocked Addresses > Create a new filter, for example `from:(your-card-issuer.example) OR subject:(receipt OR קבלה OR חשבונית)` > Create filter > tick **Forward it to** and choose the address. A filter forwards mail that arrives after it is created, not old mail.

### 6. Phone notifications (optional)
A new suggestion can notify your phone, even when the app is closed. Make a key pair once:
```
openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out vapid.pem
VAPID_PRIVATE_KEY_PEM_B64=$(base64 -w0 vapid.pem)
VAPID_PUBLIC_KEY=$(openssl ec -in vapid.pem -pubout -outform DER | tail -c 65 | base64 -w0 | tr '+/' '-_' | tr -d '=')
```
Put both in `.env` (and `VAPID_SUBJECT=mailto:you@example.com`), restart, then press "התראה על הצעות חדשות" in Settings on each device. On iPhone the app must be added to the home screen first. The push carries no text: the app asks the server how many suggestions wait. These are the same variables the Sumit receipts feature uses, so one key pair serves both.

### 7. Try it
Forward (or send) a sample receipt to the address. Within seconds **Settings > Suggestions from email** shows it. The card shows when the last message arrived and any error from the AI.

## Behaviour worth knowing
- Each message is handled once (by its `Message-ID`), however many times it is delivered.
- If the AI service is down, the daily limit is reached or the key is missing, the message is kept as "not read yet" with a button to read it now, and retried automatically once a day.
- A message the AI answers nonsense for shows up as "could not read" with a button to add the transaction by hand.

## AI cost
Every AI call that reads an email is written to the change history ("קריאת מייל ב-AI") with its tokens and price, and Settings > עלות AI lists the calls and the total for a month, year or custom period. The price is the token counts Gemini reports times `EMAIL_AI_PRICE_IN_PER_M` and `EMAIL_AI_PRICE_OUT_PER_M` (USD per million input and output tokens; thinking tokens count as output). The defaults (0.10 and 0.40) are an estimate: set them to your model's real prices. Entries keep the price used when the call was made, so changing the prices only affects later calls. Like the rest of the history they are kept for a year.

## Handling some mail without the AI
The AI is the default reader. A message that needs its own treatment (for example, one whose link must be followed to fetch a bill) gets a handler: implement `Handler` in `backend/src/email/handlers.rs` and add it to `registry()`. Handlers are asked first, in order, by sender, subject or content; the first that matches handles the message and the AI is not used for it. A handler gets the parsed mail (text, HTML and `links()`), and returns suggestions for the review inbox the same way the AI does. There are none yet.
