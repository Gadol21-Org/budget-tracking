//! Oracle NoSQL backend. The handle is configured from `ORACLE_NOSQL_*`
//! environment variables, so the same code talks to the cloud service
//! (`ORACLE_NOSQL_AUTH=user`) or a local Cloud Simulator
//! (`ORACLE_NOSQL_AUTH=cloudsim`, `ORACLE_NOSQL_ENDPOINT=http://localhost:8080`).
//!
//! Three tables only (the Always Free tier allows three): `transactions`,
//! `lookups` (categories and payment methods, keyed by kind + id) and
//! `monthly_rollups`.
//!
//! NOTE: written against the SDK docs; not yet run against a live service.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use oracle_nosql_rust_sdk::types::{MapValue, TableLimits};
use oracle_nosql_rust_sdk::{
    DeleteRequest, GetRequest, Handle, PutRequest, QueryRequest, TableRequest,
};

use std::collections::{HashMap, HashSet};

use crate::storage::{Category, ImportBatch, PaymentMethod, PendingMirror, ImportSummary, MonthlyTotal, NewTransaction, Storage, Transaction};

const TX: &str = "transactions";
const LOOKUPS: &str = "lookups";
const ROLLUPS: &str = "monthly_rollups";
const MAX_ROLLUP_RETRIES: usize = 8;

pub struct NoSqlStorage {
    handle: Handle,
    seq: AtomicU64,
}

impl NoSqlStorage {
    pub async fn connect() -> Result<Self> {
        let handle = Handle::builder().from_environment()?.build().await?;
        let s = Self { handle, seq: AtomicU64::new(0) };
        s.create_tables().await?;
        Ok(s)
    }

    async fn ddl(&self, statement: &str, limits: Option<TableLimits>) -> Result<()> {
        let mut req = TableRequest::new(table_name(statement)).statement(statement);
        if let Some(l) = limits {
            req = req.limits(&l);
        }
        req.execute(&self.handle).await?.wait_for_completion_ms(&self.handle, 30000, 500).await?;
        Ok(())
    }

    async fn create_tables(&self) -> Result<()> {
        // Small provisioned limits: ~200 operations a month is far below any cap.
        let limits = || Some(TableLimits::provisioned(10, 10, 1));
        self.ddl(
            "create table if not exists transactions (id long, date string, amount_agorot long, \
             category_id long, payment_method_id long, note string, sync_state string, \
             paamonim_line_id string, primary key(id))",
            limits(),
        )
        .await?;
        self.ddl(
            "create table if not exists lookups (kind string, id long, name string, \
             is_income boolean, paamonim_id string, primary key(kind, id))",
            limits(),
        )
        .await?;
        self.ddl(
            "create table if not exists monthly_rollups (month string, category_id long, \
             total_agorot long, count long, primary key(month, category_id))",
            limits(),
        )
        .await?;
        Ok(())
    }

    /// Time-ordered unique id: milliseconds since epoch in the high bits, a counter below.
    fn next_id(&self) -> i64 {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let n = self.seq.fetch_add(1, Ordering::Relaxed) & 0x3ff;
        ((ms << 10) | n) as i64
    }

    async fn query_all(&self, statement: &str) -> Result<Vec<MapValue>> {
        let mut req = QueryRequest::new(statement);
        let mut rows = Vec::new();
        loop {
            let mut res = req.execute(&self.handle).await?;
            rows.extend(res.take_rows());
            if req.is_done() {
                return Ok(rows);
            }
        }
    }

    async fn set_sync_state(&self, id: i64, state: &str, line_id: Option<&str>) -> Result<()> {
        let got = GetRequest::new(TX).key(MapValue::new().i64("id", id)).execute(&self.handle).await?;
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
        match line_id.map(str::to_string).or_else(|| r.get_string("paamonim_line_id")) {
            Some(l) => row = row.str("paamonim_line_id", &l),
            None => {}
        }
        PutRequest::new(TX).value(row).execute(&self.handle).await?;
        Ok(())
    }

    /// Optimistic read-modify-write; retries when another writer got in between.
    async fn adjust_rollup(&self, date: &str, category_id: Option<i64>, delta: i64, dcount: i64) -> Result<()> {
        let month = date.get(..7).ok_or_else(|| anyhow!("bad date {date}"))?;
        let cat = category_id.unwrap_or(0);
        for _ in 0..MAX_ROLLUP_RETRIES {
            let key = MapValue::new().str("month", month).i64("category_id", cat);
            let got = GetRequest::new(ROLLUPS).key(key).execute(&self.handle).await?;
            let (total, count) = got
                .row()
                .map(|r| (r.get_i64("total_agorot").unwrap_or(0), r.get_i64("count").unwrap_or(0)))
                .unwrap_or((0, 0));
            let row = MapValue::new()
                .str("month", month)
                .i64("category_id", cat)
                .i64("total_agorot", total + delta)
                .i64("count", count + dcount);
            let mut put = PutRequest::new(ROLLUPS).value(row);
            put = match got.version() {
                Some(v) => put.if_version(v),
                None => put.if_absent(),
            };
            // No version back means the condition failed: someone else wrote first.
            if put.execute(&self.handle).await?.version().is_some() {
                return Ok(());
            }
        }
        bail!("could not update monthly rollup for {month} after {MAX_ROLLUP_RETRIES} attempts")
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
        sync_state: r.get_string("sync_state").unwrap_or_else(|| "pending".into()),
    })
}

#[async_trait::async_trait]
impl Storage for NoSqlStorage {
    async fn list_payment_methods(&self) -> Result<Vec<PaymentMethod>> {
        let rows = self.query_all(&format!("select * from {LOOKUPS} where kind = 'payment_method'")).await?;
        let mut out: Vec<PaymentMethod> = rows
            .iter()
            .filter_map(|r| Some(PaymentMethod { id: r.get_i64("id")?, name: r.get_string("name")? }))
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
        let rows = self.query_all(&format!("select * from {TX} where sync_state = 'pending'")).await?;
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
                    payment_key: t.payment_method_id.and_then(|p| paamonim_of.get(&p).cloned()),
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
        let rows = self.query_all(&format!("select paamonim_line_id from {TX}")).await?;
        Ok(rows.iter().filter_map(|r| r.get_string("paamonim_line_id")).collect())
    }

    async fn import_paamonim(&self, batch: &ImportBatch) -> Result<ImportSummary> {
        let mut sum = ImportSummary::default();

        // Existing lookups: (kind, paamonim_id) -> id, so reruns reuse ids.
        let mut ids: HashMap<(String, String), i64> = HashMap::new();
        for r in self.query_all(&format!("select * from {LOOKUPS}")).await? {
            if let (Some(kind), Some(pid), Some(id)) = (r.get_string("kind"), r.get_string("paamonim_id"), r.get_i64("id")) {
                ids.insert((kind, pid), id);
            }
        }
        let mut cats: HashMap<String, i64> = HashMap::new();
        for c in &batch.categories {
            let k = ("category".to_string(), c.key.clone());
            let new = !ids.contains_key(&k);
            let id = *ids.entry(k).or_insert_with(|| self.next_id());
            let row = MapValue::new()
                .str("kind", "category")
                .i64("id", id)
                .str("name", &c.name)
                .column("is_income", c.is_income)
                .str("paamonim_id", &c.key);
            PutRequest::new(LOOKUPS).value(row).execute(&self.handle).await?;
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
            PutRequest::new(LOOKUPS).value(row).execute(&self.handle).await?;
            sum.payment_methods_added += new as usize;
            pms.insert(p.key.clone(), id);
        }

        let existing: HashSet<String> = self
            .query_all(&format!("select paamonim_line_id from {TX}"))
            .await?
            .iter()
            .filter_map(|r| r.get_string("paamonim_line_id"))
            .collect();
        // Aggregate rollup deltas first so each (month, category) is written once.
        let mut deltas: HashMap<(String, Option<i64>), (i64, i64)> = HashMap::new();
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
            PutRequest::new(TX).value(row).execute(&self.handle).await?;
            let d = deltas.entry((t.date[..7].to_string(), cat)).or_default();
            d.0 += t.amount_agorot;
            d.1 += 1;
            sum.transactions_added += 1;
        }
        for ((month, cat), (total, count)) in deltas {
            self.adjust_rollup(&format!("{month}-01"), cat, total, count).await?;
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
        PutRequest::new(TX).value(row).execute(&self.handle).await?;
        self.adjust_rollup(&t.date, t.category_id, t.amount_agorot, 1).await?;
        Ok(id)
    }

    async fn delete_transaction(&self, id: i64) -> Result<()> {
        let got = GetRequest::new(TX).key(MapValue::new().i64("id", id)).execute(&self.handle).await?;
        let Some(row) = got.row() else { return Ok(()) };
        let tx = tx_from_row(row)?;
        if DeleteRequest::new(TX, MapValue::new().i64("id", id)).execute(&self.handle).await?.success() {
            self.adjust_rollup(&tx.date, tx.category_id, -tx.amount_agorot, -1).await?;
        }
        Ok(())
    }

    async fn list_categories(&self) -> Result<Vec<Category>> {
        let rows = self.query_all(&format!("select * from {LOOKUPS} where kind = 'category'")).await?;
        let mut cats: Vec<Category> = rows
            .iter()
            .filter_map(|r| {
                Some(Category {
                    id: r.get_i64("id")?,
                    name: r.get_string("name")?,
                    is_income: r.get_bool("is_income").unwrap_or(false),
                })
            })
            .collect();
        cats.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(cats)
    }

    async fn monthly_totals(&self) -> Result<Vec<MonthlyTotal>> {
        let rows = self.query_all(&format!("select * from {ROLLUPS}")).await?;
        let mut totals: Vec<MonthlyTotal> = rows
            .iter()
            .filter_map(|r| {
                let count = r.get_i64("count")?;
                let cat = r.get_i64("category_id")?;
                (count > 0).then(|| MonthlyTotal {
                    month: r.get_string("month").unwrap_or_default(),
                    category_id: (cat != 0).then_some(cat),
                    total_agorot: r.get_i64("total_agorot").unwrap_or(0),
                    count,
                })
            })
            .collect();
        totals.sort_by(|a, b| b.month.cmp(&a.month).then(a.category_id.cmp(&b.category_id)));
        Ok(totals)
    }
}

#[cfg(test)]
mod tests {
    use super::table_name;

    #[test]
    fn extracts_table_name() {
        assert_eq!(table_name("create table if not exists lookups (kind string)"), "lookups");
    }
}
