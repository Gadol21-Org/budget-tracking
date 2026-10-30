//! MySQL backend (Oracle MySQL HeatWave Always Free, or any MySQL 8 server).
//! Plain SQL, so dashboards can aggregate in the database.
//!
//! Configured with `MYSQL_URL`, e.g. `mysql://user:password@host:3306/budget`
//! (percent-encode special characters in the password).
//!
//! NOTE: not yet run against a live server.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::ConnectOptions;
use sqlx::Row;

use crate::storage::{
    Category, ImportBatch, ImportSummary, MonthlyTotal, NewTransaction, PaymentMethod,
    PendingMirror, Storage, Transaction,
};

const SCHEMA: [&str; 3] = [
    "CREATE TABLE IF NOT EXISTS categories (
         id BIGINT AUTO_INCREMENT PRIMARY KEY,
         name VARCHAR(400) NOT NULL,
         is_income BOOLEAN NOT NULL DEFAULT FALSE,
         paamonim_item_id VARCHAR(100) UNIQUE,
         parent_id BIGINT NULL,
         FOREIGN KEY (parent_id) REFERENCES categories(id)
     )",
    "CREATE TABLE IF NOT EXISTS payment_methods (
         id BIGINT AUTO_INCREMENT PRIMARY KEY,
         name VARCHAR(400) NOT NULL,
         paamonim_id VARCHAR(100) UNIQUE
     )",
    "CREATE TABLE IF NOT EXISTS transactions (
         id BIGINT AUTO_INCREMENT PRIMARY KEY,
         date VARCHAR(10) NOT NULL,              -- YYYY-MM-DD
         amount_agorot BIGINT NOT NULL,          -- exact, may be non-round
         category_id BIGINT NULL,
         payment_method_id BIGINT NULL,
         note TEXT NOT NULL,
         paamonim_line_id VARCHAR(100) UNIQUE,   -- set once mirrored/imported
         sync_state VARCHAR(20) NOT NULL DEFAULT 'pending', -- pending | synced | modified | error
         INDEX idx_tx_date (date),
         INDEX idx_tx_category_date (category_id, date),
         FOREIGN KEY (category_id) REFERENCES categories(id),
         FOREIGN KEY (payment_method_id) REFERENCES payment_methods(id)
     )",
];

pub struct MySqlStorage {
    pool: MySqlPool,
}

impl MySqlStorage {
    pub async fn connect() -> Result<Self> {
        let url = std::env::var("MYSQL_URL").context("MYSQL_URL is not set")?;
        let opts: MySqlConnectOptions = url.parse().context("parsing MYSQL_URL")?;
        // Create the database on first use (the account needs the CREATE privilege;
        // if it lacks it and the database already exists, this is skipped).
        if let Some(db) = opts.get_database() {
            let server = opts.clone().database("");
            let mut conn = server.connect().await.context("connecting to MySQL")?;
            let name = db.replace('`', "``");
            if let Err(e) = sqlx::query(&format!(
                "CREATE DATABASE IF NOT EXISTS `{name}` CHARACTER SET utf8mb4"
            ))
            .execute(&mut conn)
            .await
            {
                tracing::debug!("not creating database {db}: {e}");
            }
        }
        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .context("connecting to MySQL")?;
        for ddl in SCHEMA {
            sqlx::query(ddl).execute(&pool).await?;
        }
        Ok(Self { pool })
    }
}

fn tx_from_row(r: &MySqlRow) -> Result<Transaction> {
    Ok(Transaction {
        id: r.try_get("id")?,
        date: r.try_get("date")?,
        amount_agorot: r.try_get("amount_agorot")?,
        category_id: r.try_get("category_id")?,
        payment_method_id: r.try_get("payment_method_id")?,
        note: r.try_get("note")?,
        sync_state: r.try_get("sync_state")?,
    })
}

#[async_trait::async_trait]
impl Storage for MySqlStorage {
    async fn list_payment_methods(&self) -> Result<Vec<PaymentMethod>> {
        let rows = sqlx::query("SELECT id, name FROM payment_methods ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(PaymentMethod {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                })
            })
            .collect()
    }

    async fn pending_mirrors(&self) -> Result<Vec<PendingMirror>> {
        let rows = sqlx::query(
            "SELECT t.id, t.date, t.amount_agorot, t.note,
                    c.paamonim_item_id AS item_key, p.paamonim_id AS payment_key
             FROM transactions t
             LEFT JOIN categories c ON c.id = t.category_id
             LEFT JOIN payment_methods p ON p.id = t.payment_method_id
             WHERE t.sync_state = 'pending' ORDER BY t.id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(PendingMirror {
                    id: r.try_get("id")?,
                    date: r.try_get("date")?,
                    amount_agorot: r.try_get("amount_agorot")?,
                    note: r.try_get("note")?,
                    item_key: r.try_get("item_key")?,
                    payment_key: r.try_get("payment_key")?,
                })
            })
            .collect()
    }

    async fn mark_mirrored(&self, id: i64, line_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE transactions SET sync_state = 'synced', paamonim_line_id = ? WHERE id = ?",
        )
        .bind(line_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_mirror_error(&self, id: i64) -> Result<()> {
        sqlx::query("UPDATE transactions SET sync_state = 'error' WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn known_line_ids(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT paamonim_line_id FROM transactions WHERE paamonim_line_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok(r.try_get("paamonim_line_id")?))
            .collect()
    }

    async fn import_paamonim(&self, batch: &ImportBatch) -> Result<ImportSummary> {
        let mut tx = self.pool.begin().await?;
        let mut sum = ImportSummary::default();

        let mut cats: HashMap<String, i64> = HashMap::new();
        for c in &batch.categories {
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT id FROM categories WHERE paamonim_item_id = ?")
                    .bind(&c.key)
                    .fetch_optional(&mut *tx)
                    .await?;
            let parent = c.parent_key.as_ref().and_then(|k| cats.get(k)).copied();
            let id = match existing {
                Some(id) => {
                    sqlx::query("UPDATE categories SET name = ?, parent_id = ? WHERE id = ?")
                        .bind(&c.name)
                        .bind(parent)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    id
                }
                None => {
                    sum.categories_added += 1;
                    let res = sqlx::query(
                        "INSERT INTO categories (name, is_income, paamonim_item_id, parent_id)
                         VALUES (?, ?, ?, ?)",
                    )
                    .bind(&c.name)
                    .bind(c.is_income)
                    .bind(&c.key)
                    .bind(parent)
                    .execute(&mut *tx)
                    .await?;
                    res.last_insert_id() as i64
                }
            };
            cats.insert(c.key.clone(), id);
        }

        let mut pms: HashMap<String, i64> = HashMap::new();
        for p in &batch.payment_methods {
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT id FROM payment_methods WHERE paamonim_id = ?")
                    .bind(&p.key)
                    .fetch_optional(&mut *tx)
                    .await?;
            let id = match existing {
                Some(id) => {
                    sqlx::query("UPDATE payment_methods SET name = ? WHERE id = ?")
                        .bind(&p.name)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    id
                }
                None => {
                    sum.payment_methods_added += 1;
                    let res = sqlx::query(
                        "INSERT INTO payment_methods (name, paamonim_id) VALUES (?, ?)",
                    )
                    .bind(&p.name)
                    .bind(&p.key)
                    .execute(&mut *tx)
                    .await?;
                    res.last_insert_id() as i64
                }
            };
            pms.insert(p.key.clone(), id);
        }

        let known: HashSet<String> = sqlx::query_scalar(
            "SELECT paamonim_line_id FROM transactions WHERE paamonim_line_id IS NOT NULL",
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        for t in &batch.transactions {
            let cat = t.category_key.as_ref().and_then(|k| cats.get(k)).copied();
            let pm = t.payment_key.as_ref().and_then(|k| pms.get(k)).copied();
            if known.contains(&t.line_id) {
                // Only fill in links that were missing; never overwrite local edits.
                sqlx::query(
                    "UPDATE transactions SET category_id = COALESCE(category_id, ?),
                       payment_method_id = COALESCE(payment_method_id, ?)
                     WHERE paamonim_line_id = ?",
                )
                .bind(cat)
                .bind(pm)
                .bind(&t.line_id)
                .execute(&mut *tx)
                .await?;
                sum.transactions_skipped += 1;
            } else {
                sqlx::query(
                    "INSERT INTO transactions
                       (date, amount_agorot, category_id, payment_method_id, note, paamonim_line_id, sync_state)
                     VALUES (?, ?, ?, ?, ?, ?, 'synced')",
                )
                .bind(&t.date)
                .bind(t.amount_agorot)
                .bind(cat)
                .bind(pm)
                .bind(&t.note)
                .bind(&t.line_id)
                .execute(&mut *tx)
                .await?;
                sum.transactions_added += 1;
            }
        }
        tx.commit().await?;
        Ok(sum)
    }

    async fn list_transactions(&self) -> Result<Vec<Transaction>> {
        let rows = sqlx::query(
            "SELECT id, date, amount_agorot, category_id, payment_method_id, note, sync_state
             FROM transactions ORDER BY date DESC, id DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(tx_from_row).collect()
    }

    async fn create_transaction(&self, t: &NewTransaction) -> Result<i64> {
        let res = sqlx::query(
            "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id, note)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&t.date)
        .bind(t.amount_agorot)
        .bind(t.category_id)
        .bind(t.payment_method_id)
        .bind(&t.note)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_id() as i64)
    }

    async fn update_transaction(&self, id: i64, t: &NewTransaction) -> Result<()> {
        sqlx::query(
            "UPDATE transactions SET date = ?, amount_agorot = ?, category_id = ?,
               payment_method_id = ?, note = ?,
               sync_state = CASE WHEN sync_state = 'synced' THEN 'modified' ELSE sync_state END
             WHERE id = ?",
        )
        .bind(&t.date)
        .bind(t.amount_agorot)
        .bind(t.category_id)
        .bind(t.payment_method_id)
        .bind(&t.note)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_transaction(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM transactions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_categories(&self) -> Result<Vec<Category>> {
        let rows =
            sqlx::query("SELECT id, name, is_income, parent_id FROM categories ORDER BY name")
                .fetch_all(&self.pool)
                .await?;
        rows.iter()
            .map(|r| {
                Ok(Category {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    is_income: r.try_get("is_income")?,
                    parent_id: r.try_get("parent_id")?,
                })
            })
            .collect()
    }

    /// Aggregated in the database.
    async fn monthly_totals(&self) -> Result<Vec<MonthlyTotal>> {
        let rows = sqlx::query(
            "SELECT SUBSTR(date, 1, 7) AS month, category_id,
                    CAST(SUM(amount_agorot) AS SIGNED) AS total, COUNT(*) AS n
             FROM transactions GROUP BY month, category_id
             ORDER BY month DESC, category_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(MonthlyTotal {
                    month: r.try_get("month")?,
                    category_id: r.try_get("category_id")?,
                    total_agorot: r.try_get("total")?,
                    count: r.try_get("n")?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ImportCategory, ImportPaymentMethod, ImportTransaction};

    /// Needs a MySQL 8 server with an empty database: `MYSQL_URL=... cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn import_is_idempotent_and_links_parents() {
        let s = MySqlStorage::connect().await.unwrap();
        let batch = ImportBatch {
            categories: vec![
                ImportCategory {
                    key: "ce:3".into(),
                    name: "Home".into(),
                    is_income: false,
                    parent_key: None,
                },
                ImportCategory {
                    key: "e:1".into(),
                    name: "Rent".into(),
                    is_income: false,
                    parent_key: Some("ce:3".into()),
                },
            ],
            payment_methods: vec![ImportPaymentMethod {
                key: "p:1".into(),
                name: "Card".into(),
            }],
            transactions: vec![ImportTransaction {
                line_id: "1".into(),
                date: "2026-10-03".into(),
                amount_agorot: -100,
                category_key: Some("e:1".into()),
                payment_key: Some("p:1".into()),
                note: String::new(),
            }],
        };
        let first = s.import_paamonim(&batch).await.unwrap();
        assert_eq!((first.categories_added, first.transactions_added), (2, 1));
        let again = s.import_paamonim(&batch).await.unwrap();
        assert_eq!((again.categories_added, again.transactions_skipped), (0, 1));
        let cats = s.list_categories().await.unwrap();
        let parent = cats.iter().find(|c| c.name == "Home").unwrap();
        let child = cats.iter().find(|c| c.name == "Rent").unwrap();
        assert_eq!(child.parent_id, Some(parent.id));
        assert_eq!(s.list_transactions().await.unwrap().len(), 1);
    }
}
