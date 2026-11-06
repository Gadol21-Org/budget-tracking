# Email to transactions (Gmail + AI)

Mail you label in Gmail is read by the server, an AI says which transaction each message describes, and the answers wait in **Settings > Suggestions from email**. A suggestion becomes a transaction only when you open it, check it in the normal add dialog and save.

## What the app can see
Gmail has **no permission limited to one label**. The only scope that includes message bodies is `gmail.readonly`, and it can read the whole mailbox. The limit is in the code, not in Google's grant:

- the push watch is created for the label only (`labelIds: [label]`, `INCLUDE`);
- every list and history call passes the label;
- before a message is read, its labels are checked without its content; an unlabelled message is never fetched (covered by a test);
- the push notification's content is never used, it only makes the server run a sync.

What is left: if the server or its database were compromised, the stored refresh token could read the whole mailbox. To keep that small, **use a dedicated Gmail account** that only receives what you forward to it (a filter in your main account that forwards card/bank/receipt mail), and put the label on it with a filter. Revoke any time at <https://myaccount.google.com/permissions>.

The text of a labelled email (up to 12,000 characters, no attachments) is sent to the Gemini API (`gemini-3.5-flash-lite`) for parsing. A 600 character excerpt is kept next to the suggestion so you can check it, and is deleted when you accept or reject it. The refresh token is stored in the app's database like the rest of the data.

## One-time setup in Google Cloud
Use the project that already has the Pub/Sub topic.

1. **APIs & Services > Library:** enable **Gmail API** (Pub/Sub API is already on).
2. **Google Auth Platform > Branding:** app name, your email as support and developer contact.
3. **Audience:** user type **External**, then **Publish app** so that **Publishing status = In production**. Do **not** leave it in *Testing*: in Testing the refresh token expires after 7 days. An unverified app is fine for you alone; you will see "Google hasn't verified this app" once (Advanced > Go to ... (unsafe)). Do not submit it for verification.
4. **Data access > Add or remove scopes:** add `https://www.googleapis.com/auth/gmail.readonly`.
5. **Clients > Create client:** type **Web application**. Authorized redirect URI: `https://<your-host>/api/email/gmail/callback` (add the staging one too if you test there). Copy the client ID and secret.
6. **Pub/Sub** (optional, for instant updates; without it the label is checked once a day, or when you press **Read now**): 
   1. The topic exists and `gmail-api-push@system.gserviceaccount.com` has **Pub/Sub Publisher** on it.
   2. Create a service account `budget-push` (no roles needed on the project).
   3. Create a **push subscription** on the topic: endpoint `https://<your-host>/api/email/push?token=<GMAIL_PUSH_TOKEN>`, tick **Enable authentication**, service account `budget-push`, audience `https://<your-host>/api/email/push`. Acknowledgement deadline 30 s.
   4. Pub/Sub must be allowed to create tokens for that account: grant `service-<project number>@gcp-sa-pubsub.iam.gserviceaccount.com` the role **Service Account Token Creator** on `budget-push` (the console offers to do this).
7. **Gmail (the account you connect):** create a label (default name `Budget`) and a filter that applies it, for example `from:(card-issuer.example) OR subject:(receipt OR קבלה)` > Apply the label. Optionally, in the main account, a filter that forwards matching mail to the dedicated account.

## Cloudflare
Pub/Sub cannot sign in to Cloudflare Access, so exactly one path is made public:

Zero Trust > Access > Applications > the budget app > add a policy **Bypass** (Everyone) for the path `/api/email/push` (same as what was done for `/assets`). Everything else, including the connect page and the suggestions, stays behind Access. The endpoint checks a secret in the URL and Google's signed token (audience and service account), and does nothing with the message besides starting a sync.

## Server settings (`.env`)
```
GMAIL_CLIENT_ID=...
GMAIL_CLIENT_SECRET=...
GMAIL_REDIRECT_URI=https://<your-host>/api/email/gmail/callback
GMAIL_LABEL=Budget
GEMINI_API_KEY=...
GMAIL_PUBSUB_TOPIC=projects/<project>/topics/<topic>
GMAIL_PUSH_TOKEN=<long random string, the same as in the push URL>
GMAIL_PUSH_AUDIENCE=https://<your-host>/api/email/push
GMAIL_PUSH_SERVICE_ACCOUNT=budget-push@<project>.iam.gserviceaccount.com
```
Restart the app, open **Settings > Suggestions from email > Connect Gmail**, sign in with the mailbox, accept the warning and the read-only permission. You come back to the app connected; the watch is started and renewed automatically (Google ends a watch after 7 days; the server renews it when under 2 days are left, checked daily).

## Behaviour worth knowing
- Mail already labelled when you connect is not read automatically; press **Read now** (looks back 7 days) to pick it up. Re-reading is safe: each message is handled once.
- If Gmail's history is too old, or a push was dropped, the daily check catches up by reading the label for the last 7 days.
- If the AI service is down, nothing is lost: the same messages are tried again at the next check (use **Read now** to retry sooner).
- A message the AI answers nonsense for shows up as "could not read" with a button to add it by hand.
- If Google rejects the saved sign-in (you revoked it, changed the password, or it went unused for 6 months) Settings shows the error: press Connect again.
