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
`STORAGE=sqlite` (default) needs nothing and is used for tests. `STORAGE=nosql` uses Oracle NoSQL; the SDK is configured from `ORACLE_NOSQL_*` variables (see `.env.example`), so the same code runs against:
- **Local:** the [Oracle NoSQL Cloud Simulator](https://www.oracle.com/downloads/cloud/nosql-cloud-sdk-downloads.html) with `ORACLE_NOSQL_AUTH=cloudsim` and `ORACLE_NOSQL_ENDPOINT=http://localhost:8080`.
- **Cloud:** `ORACLE_NOSQL_AUTH=user` with an OCI config file, region and compartment.

Tables are created on startup (three, to stay within the Always Free limit). The NoSQL backend has not yet been run against the simulator or the cloud service.

## Paamonim import
`cargo run -p budget-backend -- probe probe-out` logs in (needs `PAAMONIM_USERNAME`/`PAAMONIM_PASSWORD`) and saves raw responses from the `/rishumit/*` endpoints to `probe-out/` (git-ignored, contains your financial data). The import parser is written from those samples.

## Layout
- `backend/src/storage.rs` – `Storage` trait (SQLite and Oracle NoSQL) plus the `monthly_rollups` table maintained on every write, which dashboards read via `/api/monthly-totals`.
- `backend/src/nosql.rs` – Oracle NoSQL implementation of `Storage`.
- `backend/src/paamonim.rs` – Paamonim client (form login + `/rishumit/*` endpoints). Unverified against a real account yet.
- `backend/src/money.rs` – agorot to shekel rounding for export.

## Status
Scaffold only: manual transaction entry works. Paamonim import/mirroring and Google Sheets export are not finished.
