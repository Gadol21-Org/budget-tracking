# Budget Tracker

PWA (TypeScript, Vite) with a Rust (axum) backend. Amounts are stored as exact integer agorot, so non-round values are supported; they are rounded to whole shekels only when mirrored to Paamonim.

## Run locally
```
cp .env.example .env   # quote values containing '#', e.g. PAAMONIM_PASSWORD='pass#'
cargo run                        # builds the frontend too, then serves http://127.0.0.1:3000
```
`cargo run` runs `npm install` / `npm run build` in `frontend/` via `backend/build.rs` (needs Node.js; set `SKIP_FRONTEND_BUILD=1` to skip). For hot reload while editing the UI: `cd frontend && npm run dev` and open http://localhost:5173 (it proxies `/api` to :3000).

## Deployment
`cargo build --release` produces one executable with the web app embedded. Copy the binary and a `.env`, and run it; no other files are needed.

To run on an Oracle Cloud VM with MySQL HeatWave, with automatic deploys from `main`, see [`deploy/README.md`](deploy/README.md).

## Storage
The MySQL connection uses the system TLS library (`native-tls`): on Linux install OpenSSL's headers (`apt install libssl-dev pkg-config`); Windows and macOS use the built-in one, so nothing extra is needed.

`STORAGE=sqlite` (default) needs nothing and is used for tests. `STORAGE=mysql` uses MySQL 8 (Oracle MySQL HeatWave Always Free, or any server) through `MYSQL_URL=mysql://user:password@host:3306/budget`; the database and tables are created on startup, and the monthly totals are aggregated in the database. Tested against MariaDB only; the integration test runs with `MYSQL_URL=... cargo test -- --ignored`.

## Paamonim import
`cargo run -p budget-backend -- probe probe-out` logs in (needs `PAAMONIM_USERNAME`/`PAAMONIM_PASSWORD`) and saves raw responses from the `/rishumit/*` endpoints to `probe-out/` (git-ignored, contains your financial data). The import parser is written from those samples.

## Mirroring to Paamonim
On whenever `PAAMONIM_USERNAME`/`PAAMONIM_PASSWORD` are set (`PAAMONIM_MIRROR=0` turns it off); without credentials it is off. The full flow has only run against a pretend Paamonim and one manual test line (try `PAAMONIM_MIRROR_DRY_RUN=1` first). While on, transactions in a Paamonim-imported category are kept in step with Paamonim: new ones are added, edits replace the line (new line first, then the old one is deleted) and deletes remove it. Settled repeating charges count like any other transaction. Amounts are rounded to whole shekels (half away from zero) on the way out, and anything under ₪1 is sent as ₪1; the app keeps the exact amount. Zero amounts, refunds on an expense item and entries in a top-level category made here are not mirrored (`skipped`; an edit that makes them mirrorable sends them). A sub-category made here under a Paamonim category gets an item of its own in Paamonim ("הוספת סעיף", `POST /rishumit/save_family_custom_item`) the first time one of its transactions is mirrored; an existing item with the same name under that category is reused.

**Linked top-level categories.** Paamonim can't add top-level categories, so a new top-level category made here must be linked to a Paamonim top-level category of the same kind (`linked_to`; set when it is created or later with `PUT /api/categories/:id/link`). Its sub-categories are mirrored as items under that Paamonim category: a sub-category with the same name as an item already there sends its lines to that item, and a new name gets its own item. Several categories can be linked to one Paamonim category, for example one income category per person. A sub-category can be moved under another top-level category with its transactions and repeating rules (`POST /api/categories/:id/move`). One that is an item in Paamonim can only move between categories that go under the same Paamonim category, so nothing changes there, and a re-import leaves it where it was moved. An item can be held by only one sub-category, so a line added only in Paamonim is imported into that one.

In the transactions list, a line on its way to Paamonim shows a small spinning ring, then ✅ for a moment once saved, without a refresh (the page checks every 1.5 s while anything is in flight). ⚠️ stays while a line isn't in Paamonim: it didn't take it (`POST /api/mirror/paamonim/retry/:id` after a look), or the line can't go there as it is (`skipped`: a top-level category made here, a zero amount, a refund on an expense item). Nothing is shown when mirroring is off (`GET /api/mirror/status`).

Safeguards:
- **No replay.** The first start with mirroring on sets aside everything unsent or edited until then (it becomes `local`); only later changes are mirrored. Turning mirroring off and on again across a restart sets aside what happened in between.
- **Verify before trusting.** A row is `verifying` before a request is sent and `synced` only once the new line is found in that day's history with the same date, amount, item and direction (and the note and payment method are checked). After an error answer the history is checked again two seconds later (the line may have been saved anyway); an unclear outcome leaves it `verifying`/`error`, it is never re-sent blindly, and the run stops at the first problem. After checking Paamonim by hand, `POST /api/mirror/paamonim/retry/:id` queues an `error` row again.
- **Import back is safe.** An import links a line we posted to its local transaction (matching date, category and the rounded amount) instead of adding it again, and keeps the exact local amount. Lines deleted here are not re-imported while waiting to be deleted in Paamonim.
- `PAAMONIM_MIRROR_DRY_RUN=1` returns what would be sent from `POST /api/mirror/paamonim` (`planned`) and changes nothing. `PAAMONIM_MIRROR_MAX_OPS` (default 50) caps requests per run; a background sweep every `PAAMONIM_MIRROR_INTERVAL_MINUTES` (default 10) also settles repeating charges and works off the rest.
- **Lines are found by id.** Before replacing or deleting a line, its day is read from its own edit form (`edit_item_line?line_id=`, 404 once deleted), so a stale local date can't make a delete miss. A deleted line counts as done only once that form answers 404.
- **Installments are left alone.** A history line marked `N מתוך M` belongs to an installment group, and the site deletes the whole group at once, so such lines are never replaced or deleted (counted as `unsupported`).
- **Payment methods**: only an id the site's form offers is sent; otherwise the field is left empty. (The importer's "payment methods" are really installment groups from the history, not method ids.)
- **Delete endpoint** (checked with one labelled test line on a real account, saved and deleted by hand): `POST /rishumit/delete_history_line` with `line_id`, the same call the site makes. `PAAMONIM_DELETE_PATH` / `PAAMONIM_DELETE_PARAM` override it. An edit is a new line plus a delete of the old one (the site's own edit form keeps the line id in the server session, which a mirror can't rely on). The site itself rounds amounts to whole shekels (a test of 1.50 showed as 2), in line with our rounding.

Rounding means Paamonim totals can differ slightly from the app's (each line rounds on its own, and sub-shekel entries count as ₪1 there). Changes made in Paamonim itself are not pulled into existing local transactions.

## Repeating monthly transactions
Tick "חוזר מדי חודש" when adding an expense or income, for a set number of months or without end. A series is stored as rules (`recurring` table, `backend/src/recurring.rs`). Occurrences still ahead are generated when read and shown with an orange ⏳ (`GET /api/recurring/occurrences`; also counted in `/api/monthly-totals`). When an occurrence's date passes it is *settled*: written as an ordinary transaction (🔁, sync state `pending`, so it is mirrored to Paamonim like any other) that appears in the totals and the Google Sheets export like any other. A series can only be changed in months that haven't settled: editing an upcoming line changes the series from that month on (`PUT /api/recurring/:series_id`, the body's `start_month` is the first affected month), and "cancel from here" stops it (`DELETE /api/recurring/:series_id?from=YYYY-MM`). The edit dialog has "update" and "cancel charge" drop-downs, each with "this month only" and "from here on". "Update this month only" (`POST /api/recurring/:series_id/months/:month`) settles that one month early with the values and date you give (a salary that came early or differed) and leaves the rest of the series alone; `DELETE` on the same path skips just that month; deleting a settled transaction also skips it. Settled transactions are never rewritten by series changes; edit or delete them individually. Each (series, month) settles once, even if its transaction is deleted. Settling runs when data is read and in the nightly export. Supported by SQLite and MySQL (an existing database needs the new `recurring*` tables/column, created on start; MySQL adds the column with `ALTER`).

## Google Sheets export
Set `GOOGLE_SERVICE_ACCOUNT_JSON_B64` and `GOOGLE_SHEET_ID` (share the sheet with the service account as Editor). One tab per stored table (Transactions, Categories, Payment methods, Recurring, Recurring settled, Change log, Paamonim pending deletes, Paamonim meta) makes a complete backup; all are cleared and rewritten by `POST /api/export/sheets` and nightly (`SHEETS_EXPORT_INTERVAL_HOURS`). Verified live against Google Sheets.

## Layout
- `backend/src/storage.rs` – `Storage` trait (SQLite and MySQL). Monthly totals (`/api/monthly-totals`) are computed on demand from transactions, with no pre-aggregation; add a date index or rollups if a backend gets slow with very large history.
- `backend/src/mysql.rs` – MySQL implementation of `Storage`.
- `backend/src/paamonim.rs` – Paamonim client (form login + `/rishumit/*` endpoints). The server logs in once and keeps the session; when Paamonim redirects a request to the login page (session expired), the client logs in again and resends it once.
- `backend/src/money.rs` – agorot to shekel rounding for export.

## Status
Scaffold only: manual transaction entry works. Paamonim import/mirroring and Google Sheets export are not finished.
