//! Oracle NoSQL backend. The handle is configured from `ORACLE_NOSQL_*`
//! environment variables, so the same code talks to the cloud service
//! (`ORACLE_NOSQL_AUTH=user`) or a local Cloud Simulator
//! (`ORACLE_NOSQL_AUTH=cloudsim`, `ORACLE_NOSQL_ENDPOINT=http://localhost:8080`).
//!
//! Two tables (the Always Free tier allows three): `transactions` and
//! `lookups` (categories and payment methods, keyed by kind + id).
//!
//! NOTE: written against the SDK docs; not yet run against a live service.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use oracle_nosql_rust_sdk::types::{MapValue, TableLimits};
use oracle_nosql_rust_sdk::{
    DeleteRequest, GetRequest, Handle, PutRequest, QueryRequest, TableRequest,
};
use oracle_nosql_rust_sdk::{NoSQLError, NoSQLErrorCode};

use std::collections::{HashMap, HashSet};

use crate::storage::{
    Category, ImportBatch, ImportSummary, NewTransaction, PaymentMethod, PendingMirror, Storage,
    Transaction,
};

const TX: &str = "transactions";
const LOOKUPS: &str = "lookups";
const MAX_ATTEMPTS: u32 = 12;
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(8);
/// Always Free allows 25 read and 25 write units per table (and 3 tables).
const UNITS: i32 = 25;

pub struct NoSqlStorage {
    handle: Handle,
    seq: AtomicU64,
}

impl NoSqlStorage {
    pub async fn connect() -> Result<Self> {
        // The SDK paces requests to the table's limits instead of failing them.
        let handle = Handle::builder()
            .from_environment()?
            .rate_limiting_enabled(true)?
            .build()
            .await?;
        let s = Self {
            handle,
            seq: AtomicU64::new(0),
        };
        s.create_tables().await?;
        Ok(s)
    }

    async fn ddl(&self, statement: &str, limits: Option<TableLimits>) -> Result<()> {
        let mut req = TableRequest::new(table_name(statement)).statement(statement);
        if let Some(l) = limits {
            req = req.limits(&l);
        }
        req.execute(&self.handle)
            .await?
            .wait_for_completion_ms(&self.handle, 30000, 500)
            .await?;
        Ok(())
    }

    async fn create_tables(&self) -> Result<()> {
        // Small provisioned limits: ~200 operations a month is far below any cap.
        let limits = || Some(TableLimits::provisioned(UNITS, UNITS, 1));
        self.ddl(
            "create table if not exists transactions (id long, date string, amount_agorot long, \
             category_id long, payment_method_id long, note string, sync_state string, \
             paamonim_line_id string, primary key(id))",
            limits(),
        )
        .await?;
        self.ddl(
            "create table if not exists lookups (kind string, id long, name string, \
             is_income boolean, paamonim_id string, parent_id long, primary key(kind, id))",
            limits(),
        )
        .await?;
        // Tables created by an earlier version keep their old limits and lack
        // `parent_id`; bring them up to date (both are harmless no-ops otherwise).
        for table in [TX, LOOKUPS] {
            let l = TableLimits::provisioned(UNITS, UNITS, 1);
            let done = match TableRequest::new(table)
                .limits(&l)
                .execute(&self.handle)
                .await
            {
                Ok(mut r) => r.wait_for_completion_ms(&self.handle, 30000, 500).await,
                Err(e) => Err(e),
            };
            if let Err(e) = done {
                tracing::debug!("not changing limits of {table}: {e}");
            }
        }
        let alter = "alter table lookups (add parent_id long)";
        if let Err(e) = self.ddl(alter, None).await {
            tracing::debug!("lookups.parent_id: {e}");
        }
        Ok(())
    }

    /// Time-ordered unique id: milliseconds since epoch in the high bits, a counter below.
    fn next_id(&self) -> i64 {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let n = self.seq.fetch_add(1, Ordering::Relaxed) & 0x3ff;
        ((ms << 10) | n) as i64
    }

    async fn query_all(&self, statement: &str) -> Result<Vec<MapValue>> {
        let mut req = QueryRequest::new(statement);
        let mut rows = Vec::new();
        loop {
            let mut attempt = 1;
            let mut delay = BACKOFF_START;
            let mut res = loop {
                match req.execute(&self.handle).await {
                    Err(e) if attempt < MAX_ATTEMPTS && throttled(&e) => {
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        delay = (delay * 2).min(BACKOFF_MAX);
                    }
                    other => break other?,
                }
            };
            rows.extend(res.take_rows());
            if req.is_done() {
                return Ok(rows);
            }
        }
    }

    async fn put(&self, table: &str, row: MapValue) -> Result<()> {
        let req = PutRequest::new(table).value(row);
        retry(|| req.execute(&self.handle)).await?;
        Ok(())
    }

    async fn set_sync_state(&self, id: i64, state: &str, line_id: Option<&str>) -> Result<()> {
        let req = GetRequest::new(TX).key(MapValue::new().i64("id", id));
        let got = retry(|| req.execute(&self.handle)).await?;
        let Some(r) = got.row() else { return Ok(()) };
        let t = tx_from_row(r)?;
        let mut row = MapValue::new()
            .i64("id", t.id)
            .str("date", &t.date)
            .i64("amount_agorot", t.amount_agorot)
            .str("note", &t.note)
            .str("sync_state", state);
        if let Some(c) = t.category_id {
            row = row.i64("category_id", c);
        }
        if let Some(p) = t.payment_method_id {
            row = row.i64("payment_method_id", p);
        }
        if let Some(l) = line_id
            .map(str::to_string)
            .or_else(|| r.get_string("paamonim_line_id"))
        {
            row = row.str("paamonim_line_id", &l);
        }
        self.put(TX, row).await?;
        Ok(())
    }
}

fn throttled(e: &NoSQLError) -> bool {
    matches!(
        e.code,
        NoSQLErrorCode::WriteLimitExceeded | NoSQLErrorCode::ReadLimitExceeded
    )
}

/// Run a request, retrying with exponential backoff while the table's
/// provisioned throughput is exceeded (bulk imports hit this easily).
async fn retry<T, F, Fut>(mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, NoSQLError>>,
{
    let mut delay = BACKOFF_START;
    let mut attempt = 1;
    loop {
        match op().await {
            Err(e) if attempt < MAX_ATTEMPTS && throttled(&e) => {
                tokio::time::sleep(delay).await;
                attempt += 1;
                delay = (delay * 2).min(BACKOFF_MAX);
            }
            other => return other.map_err(Into::into),
        }
    }
}

/// Table name from a `create table if not exists <name> (...)` statement.
fn table_name(statement: &str) -> &str {
    statement.split_whitespace().nth(5).unwrap_or("")
}

fn tx_from_row(r: &MapValue) -> Result<Transaction> {
    Ok(Transaction {
        id: r.get_i64("id").ok_or_else(|| anyhow!("row without id"))?,
        date: r.get_string("date").unwrap_or_default(),
        amount_agorot: r.get_i64("amount_agorot").unwrap_or(0),
        category_id: r.get_i64("category_id"),
        payment_method_id: r.get_i64("payment_method_id"),
        note: r.get_string("note").unwrap_or_default(),
        sync_state: r
            .get_string("sync_state")
            .unwrap_or_else(|| "pending".into()),
    })
}

#[async_trait::async_trait]
impl Storage for NoSqlStorage {
    async fn list_payment_methods(&self) -> Result<Vec<PaymentMethod>> {
        let rows = self
            .query_all(&format!(
                "select * from {LOOKUPS} where kind = 'payment_method'"
            ))
            .await?;
        let mut out: Vec<PaymentMethod> = rows
            .iter()
            .filter_map(|r| {
                Some(PaymentMethod {
                    id: r.get_i64("id")?,
                    name: r.get_string("name")?,
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn pending_mirrors(&self) -> Result<Vec<PendingMirror>> {
        let mut paamonim_of: HashMap<i64, String> = HashMap::new();
        let mut category_ids = HashSet::new();
        for r in self.query_all(&format!("select * from {LOOKUPS}")).await? {
            if let (Some(id), Some(pid)) = (r.get_i64("id"), r.get_string("paamonim_id")) {
                if r.get_string("kind").as_deref() == Some("category") {
                    category_ids.insert(id);
                }
                paamonim_of.insert(id, pid);
            }
        }
        let rows = self
            .query_all(&format!("select * from {TX} where sync_state = 'pending'"))
            .await?;
        let mut out = rows
            .iter()
            .map(|r| {
                let t = tx_from_row(r)?;
                Ok(PendingMirror {
                    id: t.id,
                    date: t.date,
                    amount_agorot: t.amount_agorot,
                    note: t.note,
                    item_key: t.category_id.and_then(|c| paamonim_of.get(&c).cloned()),
                    payment_key: t
                        .payment_method_id
                        .and_then(|p| paamonim_of.get(&p).cloned()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        out.sort_by_key(|p| p.id);
        Ok(out)
    }

    async fn mark_mirrored(&self, id: i64, line_id: &str) -> Result<()> {
        self.set_sync_state(id, "synced", Some(line_id)).await
    }

    async fn mark_mirror_error(&self, id: i64) -> Result<()> {
        self.set_sync_state(id, "error", None).await
    }

    async fn known_line_ids(&self) -> Result<Vec<String>> {
        let rows = self
            .query_all(&format!("select paamonim_line_id from {TX}"))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.get_string("paamonim_line_id"))
            .collect())
    }

    async fn import_paamonim(&self, batch: &ImportBatch) -> Result<ImportSummary> {
        let mut sum = ImportSummary::default();

        // Existing lookups: (kind, paamonim_id) -> id, so reruns reuse ids.
        let mut ids: HashMap<(String, String), i64> = HashMap::new();
        for r in self.query_all(&format!("select * from {LOOKUPS}")).await? {
            if let (Some(kind), Some(pid), Some(id)) = (
                r.get_string("kind"),
                r.get_string("paamonim_id"),
                r.get_i64("id"),
            ) {
                ids.insert((kind, pid), id);
            }
        }
        let mut cats: HashMap<String, i64> = HashMap::new();
        for c in &batch.categories {
            let k = ("category".to_string(), c.key.clone());
            let new = !ids.contains_key(&k);
            let id = *ids.entry(k).or_insert_with(|| self.next_id());
            let mut row = MapValue::new()
                .str("kind", "category")
                .i64("id", id)
                .str("name", &c.name)
                .column("is_income", c.is_income)
                .str("paamonim_id", &c.key);
            if let Some(p) = c.parent_key.as_ref().and_then(|k| cats.get(k)) {
                row = row.i64("parent_id", *p);
            }
            self.put(LOOKUPS, row).await?;
            sum.categories_added += new as usize;
            cats.insert(c.key.clone(), id);
        }
        let mut pms: HashMap<String, i64> = HashMap::new();
        for p in &batch.payment_methods {
            let k = ("payment_method".to_string(), p.key.clone());
            let new = !ids.contains_key(&k);
            let id = *ids.entry(k).or_insert_with(|| self.next_id());
            let row = MapValue::new()
                .str("kind", "payment_method")
                .i64("id", id)
                .str("name", &p.name)
                .column("is_income", false)
                .str("paamonim_id", &p.key);
            self.put(LOOKUPS, row).await?;
            sum.payment_methods_added += new as usize;
            pms.insert(p.key.clone(), id);
        }

        let existing: HashSet<String> = self
            .query_all(&format!("select paamonim_line_id from {TX}"))
            .await?
            .iter()
            .filter_map(|r| r.get_string("paamonim_line_id"))
            .collect();
        for t in &batch.transactions {
            if existing.contains(&t.line_id) {
                sum.transactions_skipped += 1;
                continue;
            }
            let cat = t.category_key.as_ref().and_then(|k| cats.get(k)).copied();
            let pm = t.payment_key.as_ref().and_then(|k| pms.get(k)).copied();
            let mut row = MapValue::new()
                .i64("id", self.next_id())
                .str("date", &t.date)
                .i64("amount_agorot", t.amount_agorot)
                .str("note", &t.note)
                .str("sync_state", "synced")
                .str("paamonim_line_id", &t.line_id);
            if let Some(c) = cat {
                row = row.i64("category_id", c);
            }
            if let Some(p) = pm {
                row = row.i64("payment_method_id", p);
            }
            self.put(TX, row).await?;
            sum.transactions_added += 1;
        }
        Ok(sum)
    }

    async fn list_transactions(&self) -> Result<Vec<Transaction>> {
        let rows = self.query_all(&format!("select * from {TX}")).await?;
        let mut txs = rows.iter().map(tx_from_row).collect::<Result<Vec<_>>>()?;
        txs.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        Ok(txs)
    }

    async fn create_transaction(&self, t: &NewTransaction) -> Result<i64> {
        let id = self.next_id();
        let mut row = MapValue::new()
            .i64("id", id)
            .str("date", &t.date)
            .i64("amount_agorot", t.amount_agorot)
            .str("note", &t.note)
            .str("sync_state", "pending");
        if let Some(c) = t.category_id {
            row = row.i64("category_id", c);
        }
        if let Some(p) = t.payment_method_id {
            row = row.i64("payment_method_id", p);
        }
        self.put(TX, row).await?;
        Ok(id)
    }

    async fn update_transaction(&self, id: i64, t: &NewTransaction) -> Result<()> {
        let req = GetRequest::new(TX).key(MapValue::new().i64("id", id));
        let got = retry(|| req.execute(&self.handle)).await?;
        let Some(r) = got.row() else { return Ok(()) };
        let state = r
            .get_string("sync_state")
            .unwrap_or_else(|| "pending".into());
        let state = if state == "synced" {
            "modified".to_string()
        } else {
            state
        };
        let mut row = MapValue::new()
            .i64("id", id)
            .str("date", &t.date)
            .i64("amount_agorot", t.amount_agorot)
            .str("note", &t.note)
            .str("sync_state", &state);
        if let Some(c) = t.category_id {
            row = row.i64("category_id", c);
        }
        if let Some(p) = t.payment_method_id {
            row = row.i64("payment_method_id", p);
        }
        if let Some(l) = r.get_string("paamonim_line_id") {
            row = row.str("paamonim_line_id", &l);
        }
        self.put(TX, row).await?;
        Ok(())
    }

    async fn delete_transaction(&self, id: i64) -> Result<()> {
        let req = DeleteRequest::new(TX, MapValue::new().i64("id", id));
        retry(|| req.execute(&self.handle)).await?;
        Ok(())
    }

    async fn list_categories(&self) -> Result<Vec<Category>> {
        let rows = self
            .query_all(&format!("select * from {LOOKUPS} where kind = 'category'"))
            .await?;
        let mut cats: Vec<Category> = rows
            .iter()
            .filter_map(|r| {
                Some(Category {
                    id: r.get_i64("id")?,
                    name: r.get_string("name")?,
                    is_income: r.get_bool("is_income").unwrap_or(false),
                    parent_id: r.get_i64("parent_id"),
                })
            })
            .collect();
        cats.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(cats)
    }
}

#[cfg(test)]
mod tests {
    use super::table_name;

    #[test]
    fn extracts_table_name() {
        assert_eq!(
            table_name("create table if not exists lookups (kind string)"),
            "lookups"
        );
    }
}
