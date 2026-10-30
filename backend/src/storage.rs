//! Storage boundary. The app talks only to [`Storage`], so SQLite (local/tests)
//! can be swapped for Postgres on Oracle Cloud without touching the API.

use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct Transaction {
    pub id: i64,
    pub date: String,
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub note: String,
    pub sync_state: String,
}

#[derive(Deserialize)]
pub struct NewTransaction {
    pub date: String,
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    #[serde(default)]
    pub note: String,
}

#[derive(Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub is_income: bool,
}

/// One row of the pre-aggregated monthly rollup that dashboards read.
#[derive(Serialize)]
pub struct MonthlyTotal {
    pub month: String, // YYYY-MM
    pub category_id: Option<i64>,
    pub total_agorot: i64,
    pub count: i64,
}

/// One Paamonim import run. Keys are Paamonim ids (items are `e:<id>` / `i:<id>` since
/// expense and income ids are separate spaces). Amounts are signed agorot: expenses negative.
pub struct ImportCategory {
    pub key: String,
    pub name: String,
    pub is_income: bool,
}

pub struct ImportPaymentMethod {
    pub key: String,
    pub name: String,
}

pub struct ImportTransaction {
    pub line_id: String,
    pub date: String,
    pub amount_agorot: i64,
    pub category_key: Option<String>,
    pub payment_key: Option<String>,
    pub note: String,
}

#[derive(Default)]
pub struct ImportBatch {
    pub categories: Vec<ImportCategory>,
    pub payment_methods: Vec<ImportPaymentMethod>,
    pub transactions: Vec<ImportTransaction>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ImportSummary {
    pub categories_added: usize,
    pub payment_methods_added: usize,
    pub transactions_added: usize,
    /// Already imported (same Paamonim line id); import is idempotent.
    pub transactions_skipped: usize,
}

#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    /// Idempotent: lookups are upserted by key, transactions skipped if their line id exists.
    async fn import_paamonim(&self, batch: &ImportBatch) -> anyhow::Result<ImportSummary>;
    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>>;
    async fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64>;
    async fn delete_transaction(&self, id: i64) -> anyhow::Result<()>;
    async fn list_categories(&self) -> anyhow::Result<Vec<Category>>;
    /// Dashboards read this instead of scanning transactions.
    async fn monthly_totals(&self) -> anyhow::Result<Vec<MonthlyTotal>>;
}

pub struct SqliteStorage(Mutex<Connection>);

impl SqliteStorage {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        Ok(Self(Mutex::new(crate::db::open(path)?)))
    }
}

/// Rollup rows use category_id = 0 for "uncategorised" so the key is never NULL.
fn adjust_rollup(conn: &Connection, date: &str, category_id: Option<i64>, delta: i64, dcount: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO monthly_rollups (month, category_id, total_agorot, count) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(month, category_id) DO UPDATE SET
           total_agorot = total_agorot + ?3, count = count + ?4",
        params![&date[..7], category_id.unwrap_or(0), delta, dcount],
    )?;
    Ok(())
}

#[async_trait::async_trait]
impl Storage for SqliteStorage {
    async fn import_paamonim(&self, batch: &ImportBatch) -> anyhow::Result<ImportSummary> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let mut sum = ImportSummary::default();
        let mut cats = std::collections::HashMap::new();
        for c in &batch.categories {
            let existed: Option<i64> = tx
                .query_row("SELECT id FROM categories WHERE paamonim_item_id = ?1", params![c.key], |r| r.get(0))
                .ok();
            tx.execute(
                "INSERT INTO categories (name, is_income, paamonim_item_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT(paamonim_item_id) DO UPDATE SET name = excluded.name",
                params![c.name, c.is_income, c.key],
            )?;
            let id: i64 = tx.query_row("SELECT id FROM categories WHERE paamonim_item_id = ?1", params![c.key], |r| r.get(0))?;
            sum.categories_added += existed.is_none() as usize;
            cats.insert(c.key.clone(), id);
        }
        let mut pms = std::collections::HashMap::new();
        for p in &batch.payment_methods {
            let existed: Option<i64> = tx
                .query_row("SELECT id FROM payment_methods WHERE paamonim_id = ?1", params![p.key], |r| r.get(0))
                .ok();
            tx.execute(
                "INSERT INTO payment_methods (name, paamonim_id) VALUES (?1, ?2)
                 ON CONFLICT(paamonim_id) DO UPDATE SET name = excluded.name",
                params![p.name, p.key],
            )?;
            let id: i64 = tx.query_row("SELECT id FROM payment_methods WHERE paamonim_id = ?1", params![p.key], |r| r.get(0))?;
            sum.payment_methods_added += existed.is_none() as usize;
            pms.insert(p.key.clone(), id);
        }
        for t in &batch.transactions {
            let cat = t.category_key.as_ref().and_then(|k| cats.get(k)).copied();
            let pm = t.payment_key.as_ref().and_then(|k| pms.get(k)).copied();
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO transactions
                   (date, amount_agorot, category_id, payment_method_id, note, paamonim_line_id, sync_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'synced')",
                params![t.date, t.amount_agorot, cat, pm, t.note, t.line_id],
            )?;
            if inserted == 1 {
                adjust_rollup(&tx, &t.date, cat, t.amount_agorot, 1)?;
                sum.transactions_added += 1;
            } else {
                sum.transactions_skipped += 1;
            }
        }
        tx.commit()?;
        Ok(sum)
    }

    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, date, amount_agorot, category_id, payment_method_id, note, sync_state
             FROM transactions ORDER BY date DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Transaction {
                    id: r.get(0)?,
                    date: r.get(1)?,
                    amount_agorot: r.get(2)?,
                    category_id: r.get(3)?,
                    payment_method_id: r.get(4)?,
                    note: r.get(5)?,
                    sync_state: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id, note)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![t.date, t.amount_agorot, t.category_id, t.payment_method_id, t.note],
        )?;
        let id = tx.last_insert_rowid();
        adjust_rollup(&tx, &t.date, t.category_id, t.amount_agorot, 1)?;
        tx.commit()?;
        Ok(id)
    }

    async fn delete_transaction(&self, id: i64) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let row: Option<(String, i64, Option<i64>)> = tx
            .query_row(
                "SELECT date, amount_agorot, category_id FROM transactions WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        if let Some((date, amount, cat)) = row {
            tx.execute("DELETE FROM transactions WHERE id = ?1", params![id])?;
            adjust_rollup(&tx, &date, cat, -amount, -1)?;
        }
        tx.commit()?;
        Ok(())
    }

    async fn list_categories(&self) -> anyhow::Result<Vec<Category>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name, is_income FROM categories ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Category { id: r.get(0)?, name: r.get(1)?, is_income: r.get::<_, i64>(2)? != 0 })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn monthly_totals(&self) -> anyhow::Result<Vec<MonthlyTotal>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT month, category_id, total_agorot, count FROM monthly_rollups
             WHERE count > 0 ORDER BY month DESC, category_id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let cat: i64 = r.get(1)?;
                Ok(MonthlyTotal {
                    month: r.get(0)?,
                    category_id: if cat == 0 { None } else { Some(cat) },
                    total_agorot: r.get(2)?,
                    count: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(date: &str, amount: i64, cat: Option<i64>) -> NewTransaction {
        NewTransaction { date: date.into(), amount_agorot: amount, category_id: cat, payment_method_id: None, note: String::new() }
    }

    #[tokio::test]
    async fn import_is_idempotent() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let batch = ImportBatch {
            categories: vec![ImportCategory { key: "e:1".into(), name: "Food".into(), is_income: false }],
            payment_methods: vec![ImportPaymentMethod { key: "7".into(), name: "Card".into() }],
            transactions: vec![ImportTransaction {
                line_id: "100".into(),
                date: "2026-10-03".into(),
                amount_agorot: -12300,
                category_key: Some("e:1".into()),
                payment_key: Some("7".into()),
                note: String::new(),
            }],
        };
        let first = s.import_paamonim(&batch).await.unwrap();
        assert_eq!((first.categories_added, first.transactions_added), (1, 1));
        let second = s.import_paamonim(&batch).await.unwrap();
        assert_eq!((second.categories_added, second.transactions_added, second.transactions_skipped), (0, 0, 1));
        let totals = s.monthly_totals().await.unwrap();
        assert_eq!((totals[0].total_agorot, totals[0].count), (-12300, 1));
    }

    #[tokio::test]
    async fn rollup_tracks_inserts_and_deletes() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let a = s.create_transaction(&tx("2026-10-03", 1050, None)).await.unwrap();
        s.create_transaction(&tx("2026-10-20", 2025, None)).await.unwrap();
        let totals = s.monthly_totals().await.unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].total_agorot, 3075); // non-round amounts stay exact
        s.delete_transaction(a).await.unwrap();
        assert_eq!(s.monthly_totals().await.unwrap()[0].total_agorot, 2025);
    }
}
