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

To run on an Oracle Cloud VM with Oracle NoSQL, with automatic deploys from `main`, see [`deploy/README.md`](deploy/README.md).

## Storage
Oracle's Rust SDK requires OpenSSL. On Linux/macOS the system OpenSSL is used (`apt install libssl-dev pkg-config`). On Windows it is built from source automatically (the `openssl` crate's `vendored` feature, enabled only for Windows targets), which needs the MSVC build tools and [Strawberry Perl](https://strawberryperl.com/) and adds a couple of minutes to the first build.

`STORAGE=sqlite` (default) needs nothing and is used for tests. `STORAGE=mysql` uses MySQL 8 (Oracle MySQL HeatWave Always Free, or any server) through `MYSQL_URL=mysql://user:password@host:3306/budget`; tables are created on startup, and the monthly totals are aggregated in the database. Tested against MariaDB only; the integration test runs with `MYSQL_URL=... cargo test -- --ignored`. `STORAGE=nosql` uses Oracle NoSQL; the SDK is configured from `ORACLE_NOSQL_*` variables (see `.env.example`), so the same code runs against:
- **Local:** the [Oracle NoSQL Cloud Simulator](https://www.oracle.com/downloads/cloud/nosql-cloud-sdk-downloads.html) with `ORACLE_NOSQL_AUTH=cloudsim` and `ORACLE_NOSQL_ENDPOINT=http://localhost:8080`.
- **Cloud:** `ORACLE_NOSQL_AUTH=user` with an OCI config file, region and compartment.

Two tables (`transactions`, `lookups`) are created on startup, leaving one free under the Always Free limit of three. The NoSQL backend has not yet been run against the simulator or the cloud service.

## Paamonim import
`cargo run -p budget-backend -- probe probe-out` logs in (needs `PAAMONIM_USERNAME`/`PAAMONIM_PASSWORD`) and saves raw responses from the `/rishumit/*` endpoints to `probe-out/` (git-ignored, contains your financial data). The import parser is written from those samples.

## Mirroring to Paamonim
Off by default. With `PAAMONIM_MIRROR=1`, new transactions that use a Paamonim-imported category are written to Paamonim (rounded to whole shekels; the app keeps the exact amount). `POST /api/mirror/paamonim` retries pending ones. Deletes and edits are not mirrored yet. Not yet run against a real account.

## Google Sheets export
Set `GOOGLE_SERVICE_ACCOUNT_JSON_B64` and `GOOGLE_SHEET_ID` (share the sheet with the service account as Editor). The `Transactions` and `Monthly` tabs are rewritten by `POST /api/export/sheets` and nightly (`SHEETS_EXPORT_INTERVAL_HOURS`). Verified live against Google Sheets.

## Layout
- `backend/src/storage.rs` – `Storage` trait (SQLite, MySQL and Oracle NoSQL). Monthly totals (`/api/monthly-totals`) are computed on demand from transactions, with no pre-aggregation; add a date index or rollups if a backend gets slow with very large history.
- `backend/src/mysql.rs` – MySQL implementation of `Storage`.
- `backend/src/nosql.rs` – Oracle NoSQL implementation of `Storage`.
- `backend/src/paamonim.rs` – Paamonim client (form login + `/rishumit/*` endpoints). Unverified against a real account yet.
- `backend/src/money.rs` – agorot to shekel rounding for export.

## Status
Scaffold only: manual transaction entry works. Paamonim import/mirroring and Google Sheets export are not finished.
