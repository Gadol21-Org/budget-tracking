# Budget Tracker

PWA (TypeScript, Vite) with a Rust (axum) backend. Amounts are stored as exact integer agorot, so non-round values are supported; they are rounded to whole shekels only when mirrored to Paamonim.

## Run locally
```
cp .env.example .env   # quote values containing '#', e.g. PAAMONIM_PASSWORD='pass#'
(cd frontend && npm install && npm run build)
cargo run -p budget-backend      # serves API + built PWA on :3000
# or: (cd frontend && npm run dev) for hot reload, proxying /api to :3000
```

## Storage
OpenSSL is built from source (the `openssl` crate's `vendored` feature), because Oracle's Rust SDK requires it, so no system OpenSSL is needed. The first build is slower (about 2 minutes) and needs a C compiler plus Perl (and `make` on Linux/macOS); on Windows install the MSVC build tools and [Strawberry Perl](https://strawberryperl.com/).

`STORAGE=sqlite` (default) needs nothing and is used for tests. `STORAGE=nosql` uses Oracle NoSQL; the SDK is configured from `ORACLE_NOSQL_*` variables (see `.env.example`), so the same code runs against:
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
- `backend/src/storage.rs` – `Storage` trait (SQLite and Oracle NoSQL). Monthly totals (`/api/monthly-totals`) are computed on demand from transactions, with no pre-aggregation; add a date index or rollups if a backend gets slow with very large history.
- `backend/src/nosql.rs` – Oracle NoSQL implementation of `Storage`.
- `backend/src/paamonim.rs` – Paamonim client (form login + `/rishumit/*` endpoints). Unverified against a real account yet.
- `backend/src/money.rs` – agorot to shekel rounding for export.

## Status
Scaffold only: manual transaction entry works. Paamonim import/mirroring and Google Sheets export are not finished.
