//! Storage boundary. The app talks only to [`Storage`], so SQLite (local/tests)
//! can be swapped for Postgres on Oracle Cloud without touching the API.

use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::recurring::{NewRecurring, Occurrence, RecurringSegment};

#[derive(Serialize, Clone)]
pub struct Transaction {
    pub id: i64,
    pub date: String,
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub note: String,
    pub sync_state: String,
    /// Set on a settled occurrence of a repeating transaction.
    pub recurring_series_id: Option<i64>,
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

#[derive(Serialize, Clone)]
pub struct PaymentMethod {
    pub id: i64,
    pub name: String,
}

#[derive(Serialize, Clone)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub is_income: bool,
    /// Top-level categories have none; transactions are normally on the children.
    pub parent_id: Option<i64>,
}

/// Per-month, per-category totals, computed from transactions on demand.
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
    /// Key of a parent that appears earlier in the batch.
    pub parent_key: Option<String>,
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

/// A locally created transaction waiting to be mirrored to Paamonim.
pub struct PendingMirror {
    pub id: i64,
    pub date: String,
    pub amount_agorot: i64,
    pub note: String,
    /// `e:<id>` / `i:<id>` of the Paamonim budget item; None for categories made locally.
    pub item_key: Option<String>,
    pub payment_key: Option<String>,
}

#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    async fn list_payment_methods(&self) -> anyhow::Result<Vec<PaymentMethod>>;
    async fn pending_mirrors(&self) -> anyhow::Result<Vec<PendingMirror>>;
    /// Record that the transaction now exists in Paamonim as `line_id`.
    async fn mark_mirrored(&self, id: i64, line_id: &str) -> anyhow::Result<()>;
    async fn mark_mirror_error(&self, id: i64) -> anyhow::Result<()>;
    /// Paamonim line ids already known locally.
    async fn known_line_ids(&self) -> anyhow::Result<Vec<String>>;
    /// Idempotent: lookups are upserted by key, transactions skipped if their line id exists.
    async fn import_paamonim(&self, batch: &ImportBatch) -> anyhow::Result<ImportSummary>;
    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>>;
    async fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64>;
    async fn delete_transaction(&self, id: i64) -> anyhow::Result<()>;
    /// Local edit. A line already in Paamonim is flagged `modified`: edits are not pushed there.
    async fn update_transaction(&self, id: i64, t: &NewTransaction) -> anyhow::Result<()>;
    async fn list_categories(&self) -> anyhow::Result<Vec<Category>>;
    /// Repeating-transaction rules (see `recurring.rs`). Backends without support have none.
    async fn list_recurring(&self) -> anyhow::Result<Vec<RecurringSegment>> {
        Ok(Vec::new())
    }
    /// Add a segment and return its id; `series_id` None starts a new series, whose id is
    /// that of its first segment.
    async fn insert_recurring(
        &self,
        _series_id: Option<i64>,
        _r: &NewRecurring,
    ) -> anyhow::Result<i64> {
        anyhow::bail!("repeating transactions are not supported by this storage backend")
    }
    async fn update_recurring(&self, _id: i64, _r: &NewRecurring) -> anyhow::Result<()> {
        anyhow::bail!("repeating transactions are not supported by this storage backend")
    }
    async fn delete_recurring(&self, _id: i64) -> anyhow::Result<()> {
        anyhow::bail!("repeating transactions are not supported by this storage backend")
    }
    /// Turn occurrences that have come due into real transactions (sync state `local`, so
    /// they are not mirrored). Each (series, month) is settled once, even if the transaction
    /// is later deleted. Returns how many were added.
    async fn settle_recurring(&self, _due: &[Occurrence]) -> anyhow::Result<usize> {
        Ok(0)
    }
    /// Computed on demand from transactions. Fast enough at this data size; add
    /// pre-aggregation (or a date index) if a backend gets slow with a lot of history.
    async fn monthly_totals(&self) -> anyhow::Result<Vec<MonthlyTotal>> {
        let mut by_key: std::collections::BTreeMap<(String, Option<i64>), (i64, i64)> =
            Default::default();
        for t in self.list_transactions().await? {
            let e = by_key
                .entry((t.date[..7].to_string(), t.category_id))
                .or_default();
            e.0 += t.amount_agorot;
            e.1 += 1;
        }
        let mut totals: Vec<MonthlyTotal> = by_key
            .into_iter()
            .map(
                |((month, category_id), (total_agorot, count))| MonthlyTotal {
                    month,
                    category_id,
                    total_agorot,
                    count,
                },
            )
            .collect();
        totals.sort_by(|a, b| {
            b.month
                .cmp(&a.month)
                .then(a.category_id.cmp(&b.category_id))
        });
        Ok(totals)
    }
}

pub struct SqliteStorage(Mutex<Connection>);

impl SqliteStorage {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        Ok(Self(Mutex::new(crate::db::open(path)?)))
    }
}

#[async_trait::async_trait]
impl Storage for SqliteStorage {
    async fn list_payment_methods(&self) -> anyhow::Result<Vec<PaymentMethod>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM payment_methods ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PaymentMethod {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn pending_mirrors(&self) -> anyhow::Result<Vec<PendingMirror>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.date, t.amount_agorot, t.note, c.paamonim_item_id, p.paamonim_id
             FROM transactions t
             LEFT JOIN categories c ON c.id = t.category_id
             LEFT JOIN payment_methods p ON p.id = t.payment_method_id
             WHERE t.sync_state = 'pending' ORDER BY t.id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PendingMirror {
                    id: r.get(0)?,
                    date: r.get(1)?,
                    amount_agorot: r.get(2)?,
                    note: r.get(3)?,
                    item_key: r.get(4)?,
                    payment_key: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn mark_mirrored(&self, id: i64, line_id: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE transactions SET sync_state = 'synced', paamonim_line_id = ?2 WHERE id = ?1",
            params![id, line_id],
        )?;
        Ok(())
    }

    async fn mark_mirror_error(&self, id: i64) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE transactions SET sync_state = 'error' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    async fn known_line_ids(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT paamonim_line_id FROM transactions WHERE paamonim_line_id IS NOT NULL",
        )?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    async fn import_paamonim(&self, batch: &ImportBatch) -> anyhow::Result<ImportSummary> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let mut sum = ImportSummary::default();
        let mut cats = std::collections::HashMap::new();
        for c in &batch.categories {
            let existed: Option<i64> = tx
                .query_row(
                    "SELECT id FROM categories WHERE paamonim_item_id = ?1",
                    params![c.key],
                    |r| r.get(0),
                )
                .ok();
            let parent = c.parent_key.as_ref().and_then(|k| cats.get(k)).copied();
            tx.execute(
                "INSERT INTO categories (name, is_income, paamonim_item_id, parent_id) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(paamonim_item_id) DO UPDATE SET name = excluded.name, parent_id = excluded.parent_id",
                params![c.name, c.is_income, c.key, parent],
            )?;
            let id: i64 = tx.query_row(
                "SELECT id FROM categories WHERE paamonim_item_id = ?1",
                params![c.key],
                |r| r.get(0),
            )?;
            sum.categories_added += existed.is_none() as usize;
            cats.insert(c.key.clone(), id);
        }
        let mut pms = std::collections::HashMap::new();
        for p in &batch.payment_methods {
            let existed: Option<i64> = tx
                .query_row(
                    "SELECT id FROM payment_methods WHERE paamonim_id = ?1",
                    params![p.key],
                    |r| r.get(0),
                )
                .ok();
            tx.execute(
                "INSERT INTO payment_methods (name, paamonim_id) VALUES (?1, ?2)
                 ON CONFLICT(paamonim_id) DO UPDATE SET name = excluded.name",
                params![p.name, p.key],
            )?;
            let id: i64 = tx.query_row(
                "SELECT id FROM payment_methods WHERE paamonim_id = ?1",
                params![p.key],
                |r| r.get(0),
            )?;
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
                sum.transactions_added += 1;
            } else {
                // Already imported: only fill in links that were missing (e.g. before custom
                // items or sub-categories were understood); never overwrite local edits.
                tx.execute(
                    "UPDATE transactions SET category_id = COALESCE(category_id, ?2),
                       payment_method_id = COALESCE(payment_method_id, ?3)
                     WHERE paamonim_line_id = ?1",
                    params![t.line_id, cat, pm],
                )?;
                sum.transactions_skipped += 1;
            }
        }
        tx.commit()?;
        Ok(sum)
    }

    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, date, amount_agorot, category_id, payment_method_id, note, sync_state,
                    recurring_series_id
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
                    recurring_series_id: r.get(7)?,
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
            params![
                t.date,
                t.amount_agorot,
                t.category_id,
                t.payment_method_id,
                t.note
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    async fn update_transaction(&self, id: i64, t: &NewTransaction) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE transactions SET date = ?2, amount_agorot = ?3, category_id = ?4, payment_method_id = ?5, note = ?6,
               sync_state = CASE WHEN sync_state = 'synced' THEN 'modified' ELSE sync_state END
             WHERE id = ?1",
            params![id, t.date, t.amount_agorot, t.category_id, t.payment_method_id, t.note],
        )?;
        Ok(())
    }

    async fn delete_transaction(&self, id: i64) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM transactions WHERE id = ?1", params![id])?;
        Ok(())
    }

    async fn list_categories(&self) -> anyhow::Result<Vec<Category>> {
        let conn = self.0.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, name, is_income, parent_id FROM categories ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Category {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    is_income: r.get::<_, i64>(2)? != 0,
                    parent_id: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn list_recurring(&self) -> anyhow::Result<Vec<RecurringSegment>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, series_id, start_month, end_month, day, amount_agorot, category_id,
                    payment_method_id, note
             FROM recurring ORDER BY series_id, start_month",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(RecurringSegment {
                    id: r.get(0)?,
                    series_id: r.get(1)?,
                    start_month: r.get(2)?,
                    end_month: r.get(3)?,
                    day: r.get(4)?,
                    amount_agorot: r.get(5)?,
                    category_id: r.get(6)?,
                    payment_method_id: r.get(7)?,
                    note: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn insert_recurring(
        &self,
        series_id: Option<i64>,
        r: &NewRecurring,
    ) -> anyhow::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO recurring (series_id, start_month, end_month, day, amount_agorot,
                                    category_id, payment_method_id, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                series_id.unwrap_or(0),
                r.start_month,
                r.end_month,
                r.day,
                r.amount_agorot,
                r.category_id,
                r.payment_method_id,
                r.note
            ],
        )?;
        let id = tx.last_insert_rowid();
        if series_id.is_none() {
            tx.execute(
                "UPDATE recurring SET series_id = ?1 WHERE id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    async fn update_recurring(&self, id: i64, r: &NewRecurring) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE recurring SET start_month = ?2, end_month = ?3, day = ?4, amount_agorot = ?5,
               category_id = ?6, payment_method_id = ?7, note = ?8
             WHERE id = ?1",
            params![
                id,
                r.start_month,
                r.end_month,
                r.day,
                r.amount_agorot,
                r.category_id,
                r.payment_method_id,
                r.note
            ],
        )?;
        Ok(())
    }

    async fn delete_recurring(&self, id: i64) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM recurring WHERE id = ?1", params![id])?;
        Ok(())
    }

    async fn settle_recurring(&self, due: &[Occurrence]) -> anyhow::Result<usize> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let mut added = 0;
        for o in due {
            let new = tx.execute(
                "INSERT OR IGNORE INTO recurring_settled (series_id, month) VALUES (?1, ?2)",
                params![o.series_id, &o.date[..7]],
            )?;
            if new == 1 {
                tx.execute(
                    "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id,
                                               note, sync_state, recurring_series_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'local', ?6)",
                    params![
                        o.date,
                        o.amount_agorot,
                        o.category_id,
                        o.payment_method_id,
                        o.note,
                        o.series_id
                    ],
                )?;
                added += 1;
            }
        }
        tx.commit()?;
        Ok(added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(date: &str, amount: i64, cat: Option<i64>) -> NewTransaction {
        NewTransaction {
            date: date.into(),
            amount_agorot: amount,
            category_id: cat,
            payment_method_id: None,
            note: String::new(),
        }
    }

    #[tokio::test]
    async fn editing_a_synced_line_flags_it_modified() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let id = s
            .create_transaction(&tx("2026-10-03", -1000, None))
            .await
            .unwrap();
        s.update_transaction(id, &tx("2026-10-04", -2000, None))
            .await
            .unwrap();
        let t = &s.list_transactions().await.unwrap()[0];
        assert_eq!(
            (t.date.as_str(), t.amount_agorot, t.sync_state.as_str()),
            ("2026-10-04", -2000, "pending")
        );
        s.mark_mirrored(id, "77").await.unwrap();
        s.update_transaction(id, &tx("2026-10-05", -2500, None))
            .await
            .unwrap();
        assert_eq!(
            s.list_transactions().await.unwrap()[0].sync_state,
            "modified"
        );
    }

    #[tokio::test]
    async fn import_links_parents_and_fills_missing_links_on_rerun() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let line = |cat: Option<&str>| ImportTransaction {
            line_id: "1".into(),
            date: "2026-10-03".into(),
            amount_agorot: -100,
            category_key: cat.map(String::from),
            payment_key: None,
            note: String::new(),
        };
        let cat = |key: &str, parent: Option<&str>| ImportCategory {
            key: key.into(),
            name: key.into(),
            is_income: false,
            parent_key: parent.map(String::from),
        };
        // First run does not know the item yet, so the line has no category.
        let first = ImportBatch {
            transactions: vec![line(None)],
            ..Default::default()
        };
        s.import_paamonim(&first).await.unwrap();
        let second = ImportBatch {
            categories: vec![cat("ce:3", None), cat("e:1", Some("ce:3"))],
            transactions: vec![line(Some("e:1"))],
            ..Default::default()
        };
        s.import_paamonim(&second).await.unwrap();
        let cats = s.list_categories().await.unwrap();
        let child = cats.iter().find(|c| c.name == "e:1").unwrap();
        let parent = cats.iter().find(|c| c.name == "ce:3").unwrap();
        assert_eq!(child.parent_id, Some(parent.id));
        assert_eq!(
            s.list_transactions().await.unwrap()[0].category_id,
            Some(child.id)
        );
    }

    #[tokio::test]
    async fn import_is_idempotent() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let batch = ImportBatch {
            categories: vec![ImportCategory {
                key: "e:1".into(),
                name: "Food".into(),
                is_income: false,
                parent_key: None,
            }],
            payment_methods: vec![ImportPaymentMethod {
                key: "7".into(),
                name: "Card".into(),
            }],
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
        assert_eq!(
            (
                second.categories_added,
                second.transactions_added,
                second.transactions_skipped
            ),
            (0, 0, 1)
        );
        let totals = s.monthly_totals().await.unwrap();
        assert_eq!((totals[0].total_agorot, totals[0].count), (-12300, 1));
    }

    #[tokio::test]
    async fn monthly_totals_follow_inserts_and_deletes() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let a = s
            .create_transaction(&tx("2026-10-03", 1050, None))
            .await
            .unwrap();
        s.create_transaction(&tx("2026-10-20", 2025, None))
            .await
            .unwrap();
        let totals = s.monthly_totals().await.unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].total_agorot, 3075); // non-round amounts stay exact
        s.delete_transaction(a).await.unwrap();
        assert_eq!(s.monthly_totals().await.unwrap()[0].total_agorot, 2025);
    }
}
