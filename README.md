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
Off by default. With `PAAMONIM_MIRROR=1`, new transactions that use a Paamonim-imported category are written to Paamonim (rounded to whole shekels; the app keeps the exact amount). `POST /api/mirror/paamonim` retries pending ones. Deletes and edits are not mirrored yet. Not yet run against a real account.

## Repeating monthly transactions
Tick "חוזר מדי חודש" when adding an expense or income, for a set number of months or without end. A series is stored as rules (`recurring` table, `backend/src/recurring.rs`). Occurrences still ahead are generated when read and shown with an orange ⏳ (`GET /api/recurring/occurrences`; also counted in `/api/monthly-totals`). When an occurrence's date passes it is *settled*: written as an ordinary transaction (🔁, sync state `local`, so it is not mirrored to Paamonim) that appears in the totals and the Google Sheets export like any other. A series can only be changed in months that haven't settled: editing an upcoming line changes the series from that month on (`PUT /api/recurring/:series_id`, the body's `start_month` is the first affected month), and "cancel from here" stops it (`DELETE /api/recurring/:series_id?from=YYYY-MM`). The edit dialog has "update" and "cancel charge" drop-downs, each with "this month only" and "from here on". "Update this month only" (`POST /api/recurring/:series_id/months/:month`) settles that one month early with the values and date you give (a salary that came early or differed) and leaves the rest of the series alone; `DELETE` on the same path skips just that month; deleting a settled transaction also skips it. Settled transactions are never rewritten by series changes; edit or delete them individually. Each (series, month) settles once, even if its transaction is deleted. Settling runs when data is read and in the nightly export. Supported by SQLite and MySQL (an existing database needs the new `recurring*` tables/column, created on start; MySQL adds the column with `ALTER`), not by the NoSQL backend.

## Google Sheets export
Set `GOOGLE_SERVICE_ACCOUNT_JSON_B64` and `GOOGLE_SHEET_ID` (share the sheet with the service account as Editor). The `Transactions` and `Monthly` tabs are rewritten by `POST /api/export/sheets` and nightly (`SHEETS_EXPORT_INTERVAL_HOURS`). Verified live against Google Sheets.

## Layout
- `backend/src/storage.rs` – `Storage` trait (SQLite and MySQL). Monthly totals (`/api/monthly-totals`) are computed on demand from transactions, with no pre-aggregation; add a date index or rollups if a backend gets slow with very large history.
- `backend/src/mysql.rs` – MySQL implementation of `Storage`.
- `backend/src/paamonim.rs` – Paamonim client (form login + `/rishumit/*` endpoints). Unverified against a real account yet.
- `backend/src/money.rs` – agorot to shekel rounding for export.

## Status
Scaffold only: manual transaction entry works. Paamonim import/mirroring and Google Sheets export are not finished.
