# Budget Tracker

PWA (TypeScript, Vite) with a Rust (axum) backend. Amounts are stored as exact integer agorot, so non-round values are supported; they are rounded to whole shekels only when mirrored to Paamonim.

## Run locally
```
cp .env.example .env   # quote values containing '#', e.g. PAAMONIM_PASSWORD='pass#'
(cd frontend && npm install && npm run build)
cargo run -p budget-backend      # serves API + built PWA on :3000
# or: (cd frontend && npm run dev) for hot reload, proxying /api to :3000
```

## Layout
- `backend/src/storage.rs` – `Storage` trait (SQLite today; Postgres on Oracle Cloud planned) plus the `monthly_rollups` table maintained on every write, which dashboards read via `/api/monthly-totals`.
- `backend/src/paamonim.rs` – Paamonim client (form login + `/rishumit/*` endpoints). Unverified against a real account yet.
- `backend/src/money.rs` – agorot to shekel rounding for export.

## Status
Scaffold only: manual transaction entry works. Paamonim import/mirroring and Google Sheets export are not finished.
