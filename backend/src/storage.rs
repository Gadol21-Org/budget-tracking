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

pub trait Storage: Send + Sync {
    fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>>;
    fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64>;
    fn delete_transaction(&self, id: i64) -> anyhow::Result<()>;
    fn list_categories(&self) -> anyhow::Result<Vec<Category>>;
    /// Dashboards read this instead of scanning transactions.
    fn monthly_totals(&self) -> anyhow::Result<Vec<MonthlyTotal>>;
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

impl Storage for SqliteStorage {
    fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
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

    fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64> {
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

    fn delete_transaction(&self, id: i64) -> anyhow::Result<()> {
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

    fn list_categories(&self) -> anyhow::Result<Vec<Category>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name, is_income FROM categories ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Category { id: r.get(0)?, name: r.get(1)?, is_income: r.get::<_, i64>(2)? != 0 })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn monthly_totals(&self) -> anyhow::Result<Vec<MonthlyTotal>> {
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

    #[test]
    fn rollup_tracks_inserts_and_deletes() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let a = s.create_transaction(&tx("2026-10-03", 1050, None)).unwrap();
        s.create_transaction(&tx("2026-10-20", 2025, None)).unwrap();
        let totals = s.monthly_totals().unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].total_agorot, 3075); // non-round amounts stay exact
        s.delete_transaction(a).unwrap();
        assert_eq!(s.monthly_totals().unwrap()[0].total_agorot, 2025);
    }
}
