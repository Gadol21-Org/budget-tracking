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

use crate::oplog::{LogEntry, NewLogEntry};
use crate::recurring::{NewRecurring, Occurrence, RecurringSegment};
use crate::storage::{
    rounds_to, Business, BusinessLinks, Category, ImportBatch, ImportSummary, LineDelete,
    MonthlyTotal, NewTransaction, PaymentMethod, PendingMirror, RawBackup, RawCategory,
    RawPaymentMethod, Storage, Transaction, TxFx, REQUEUE_SKIPPED_SQL,
};

const SCHEMA: [&str; 14] = [
    "CREATE TABLE IF NOT EXISTS categories (
         id BIGINT AUTO_INCREMENT PRIMARY KEY,
         name VARCHAR(400) NOT NULL,
         is_income BOOLEAN NOT NULL DEFAULT FALSE,
         paamonim_item_id VARCHAR(100) UNIQUE,
         parent_id BIGINT NULL,
         -- Deleted locally. Kept (when it has a Paamonim id) so re-imports skip it;
         -- its lines go to merged_into, if any.
         deleted BOOLEAN NOT NULL DEFAULT FALSE,
         merged_into BIGINT NULL,
         -- On a top-level category made here: the Paamonim top-level category it stands in for.
         linked_to BIGINT NULL,
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
         sync_state VARCHAR(20) NOT NULL DEFAULT 'pending', -- pending | synced | modified | verifying | error | skipped | local
         recurring_series_id BIGINT NULL,        -- set on a settled repeating transaction
         mirror_date VARCHAR(10) NULL,           -- the line's date in Paamonim, when linked
         INDEX idx_tx_date (date),
         INDEX idx_tx_category_date (category_id, date),
         FOREIGN KEY (category_id) REFERENCES categories(id),
         FOREIGN KEY (payment_method_id) REFERENCES payment_methods(id)
     )",
    "CREATE TABLE IF NOT EXISTS recurring (
         id BIGINT AUTO_INCREMENT PRIMARY KEY,
         series_id BIGINT NOT NULL,              -- id of the series' first segment
         start_month VARCHAR(7) NOT NULL,        -- YYYY-MM
         end_month VARCHAR(7) NULL,              -- inclusive; NULL = forever
         day INT NOT NULL,
         amount_agorot BIGINT NOT NULL,
         category_id BIGINT NULL,
         payment_method_id BIGINT NULL,
         note TEXT NOT NULL,
         currency VARCHAR(8) NULL,               -- foreign-currency rule: USD, EUR, JPY
         orig_minor BIGINT NULL,                 -- its signed amount in cents / whole yen
         INDEX idx_recurring_series (series_id),
         FOREIGN KEY (category_id) REFERENCES categories(id),
         FOREIGN KEY (payment_method_id) REFERENCES payment_methods(id)
     )",
    "CREATE TABLE IF NOT EXISTS recurring_settled (
         series_id BIGINT NOT NULL,
         month VARCHAR(7) NOT NULL,
         PRIMARY KEY (series_id, month)
     )",
    // Lines to delete in Paamonim: their transaction was deleted or replaced here.
    "CREATE TABLE IF NOT EXISTS mirror_deletes (
         seq BIGINT AUTO_INCREMENT PRIMARY KEY,
         line_id VARCHAR(100) NOT NULL UNIQUE,
         date VARCHAR(10) NOT NULL
     )",
    "CREATE TABLE IF NOT EXISTS mirror_meta (k VARCHAR(50) PRIMARY KEY, v VARCHAR(100) NOT NULL)",
    // What changed, with values before and after as JSON; pruned after a year.
    "CREATE TABLE IF NOT EXISTS change_log (
         id BIGINT AUTO_INCREMENT PRIMARY KEY,
         at VARCHAR(20) NOT NULL,                -- UTC, YYYY-MM-DDTHH:MM:SSZ
         kind VARCHAR(40) NOT NULL,
         entity_id BIGINT NULL,
         old_value TEXT NULL,
         new_value TEXT NULL,
         INDEX idx_change_log_at (at)
     )",
    "CREATE TABLE IF NOT EXISTS businesses (
         id BIGINT AUTO_INCREMENT PRIMARY KEY,
         name VARCHAR(400) NOT NULL,
         is_general BOOLEAN NOT NULL DEFAULT FALSE
     )",
    "CREATE TABLE IF NOT EXISTS transaction_business (
         transaction_id BIGINT PRIMARY KEY,
         business_id BIGINT NOT NULL
     )",
    "CREATE TABLE IF NOT EXISTS category_business (
         category_id BIGINT PRIMARY KEY,
         business_id BIGINT NOT NULL
     )",
    "CREATE TABLE IF NOT EXISTS transaction_fx (
         transaction_id BIGINT PRIMARY KEY,
         currency VARCHAR(8) NOT NULL,
         orig_minor BIGINT NOT NULL,
         rate DOUBLE NOT NULL,
         rate_date VARCHAR(10) NOT NULL,
         estimated BOOLEAN NOT NULL DEFAULT FALSE
     )",
    "CREATE TABLE IF NOT EXISTS fx_rates (
         currency VARCHAR(8) NOT NULL,
         date VARCHAR(10) NOT NULL,
         rate DOUBLE NOT NULL,
         PRIMARY KEY (currency, date)
     )",
    "CREATE TABLE IF NOT EXISTS category_business_required (
         category_id BIGINT PRIMARY KEY
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
        // Tables created before categories could be deleted.
        let has_deleted: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM information_schema.columns
             WHERE table_schema = DATABASE() AND table_name = 'categories' AND column_name = 'deleted'",
        )
        .fetch_one(&pool)
        .await?;
        if has_deleted == 0 {
            sqlx::query("ALTER TABLE categories ADD COLUMN deleted BOOLEAN NOT NULL DEFAULT FALSE")
                .execute(&pool)
                .await?;
            sqlx::query("ALTER TABLE categories ADD COLUMN merged_into BIGINT NULL")
                .execute(&pool)
                .await?;
        }
        // Tables created before categories could be linked; fails harmlessly if present.
        let _ = sqlx::query("ALTER TABLE categories ADD COLUMN linked_to BIGINT NULL")
            .execute(&pool)
            .await;
        // Tables created before repeating transactions existed; fails harmlessly if present.
        let _ = sqlx::query("ALTER TABLE transactions ADD COLUMN recurring_series_id BIGINT NULL")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE transactions ADD COLUMN mirror_date VARCHAR(10) NULL")
            .execute(&pool)
            .await;
        // Rules created before they could be in a foreign currency; fails harmlessly if present.
        let _ = sqlx::query("ALTER TABLE recurring ADD COLUMN currency VARCHAR(8) NULL")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE recurring ADD COLUMN orig_minor BIGINT NULL")
            .execute(&pool)
            .await;
        Ok(Self { pool })
    }
}

/// Keep the foreign-currency details with a transaction that was just inserted.
async fn insert_tx_fx(
    conn: &mut sqlx::Transaction<'_, sqlx::MySql>,
    id: i64,
    f: &TxFx,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO transaction_fx
           (transaction_id, currency, orig_minor, rate, rate_date, estimated)
         VALUES (?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE currency = VALUES(currency), orig_minor = VALUES(orig_minor),
           rate = VALUES(rate), rate_date = VALUES(rate_date), estimated = VALUES(estimated)",
    )
    .bind(id)
    .bind(&f.currency)
    .bind(f.orig_minor)
    .bind(f.rate)
    .bind(&f.rate_date)
    .bind(f.estimated)
    .execute(&mut **conn)
    .await?;
    Ok(())
}

/// Where lines of category `id` belong: itself, or what it was merged into when deleted
/// (None if it was deleted without a replacement).
async fn resolve_category(conn: &mut sqlx::MySqlConnection, mut id: i64) -> Result<Option<i64>> {
    for _ in 0..16 {
        let (deleted, into): (bool, Option<i64>) =
            sqlx::query_as("SELECT deleted, merged_into FROM categories WHERE id = ?")
                .bind(id)
                .fetch_one(&mut *conn)
                .await?;
        if !deleted {
            return Ok(Some(id));
        }
        match into {
            Some(next) => id = next,
            None => return Ok(None),
        }
    }
    Ok(None)
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
        recurring_series_id: r.try_get("recurring_series_id")?,
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

    async fn list_businesses(&self) -> Result<Vec<Business>> {
        let rows = sqlx::query("SELECT id, name, is_general FROM businesses ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(Business {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    is_general: r.try_get("is_general")?,
                })
            })
            .collect()
    }

    async fn insert_business(&self, name: &str, is_general: bool) -> Result<i64> {
        let r = sqlx::query("INSERT INTO businesses (name, is_general) VALUES (?, ?)")
            .bind(name)
            .bind(is_general)
            .execute(&self.pool)
            .await?;
        Ok(r.last_insert_id() as i64)
    }

    async fn rename_business(&self, id: i64, name: &str) -> Result<()> {
        sqlx::query("UPDATE businesses SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn remove_business(&self, id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for q in [
            "DELETE FROM transaction_business WHERE business_id = ?",
            "DELETE FROM category_business WHERE business_id = ?",
            "DELETE FROM businesses WHERE id = ?",
        ] {
            sqlx::query(q).bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn business_links(&self) -> Result<BusinessLinks> {
        let t = sqlx::query(
            "SELECT b.transaction_id, b.business_id FROM transaction_business b
             JOIN transactions t ON t.id = b.transaction_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let c = sqlx::query("SELECT category_id, business_id FROM category_business")
            .fetch_all(&self.pool)
            .await?;
        let q = sqlx::query("SELECT category_id FROM category_business_required")
            .fetch_all(&self.pool)
            .await?;
        Ok(BusinessLinks {
            required: q.iter().map(|r| r.try_get(0)).collect::<Result<_, _>>()?,
            transactions: t
                .iter()
                .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
                .collect::<Result<_>>()?,
            categories: c
                .iter()
                .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
                .collect::<Result<_>>()?,
        })
    }

    async fn set_category_business(&self, category: i64, business: Option<i64>) -> Result<()> {
        match business {
            Some(b) => {
                sqlx::query(
                    "INSERT INTO category_business (category_id, business_id) VALUES (?, ?)
                     ON DUPLICATE KEY UPDATE business_id = VALUES(business_id)",
                )
                .bind(category)
                .bind(b)
                .execute(&self.pool)
                .await?
            }
            None => {
                sqlx::query("DELETE FROM category_business WHERE category_id = ?")
                    .bind(category)
                    .execute(&self.pool)
                    .await?
            }
        };
        Ok(())
    }

    async fn set_category_business_required(&self, category: i64, required: bool) -> Result<()> {
        if required {
            sqlx::query("INSERT IGNORE INTO category_business_required (category_id) VALUES (?)")
                .bind(category)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query("DELETE FROM category_business_required WHERE category_id = ?")
                .bind(category)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn set_tx_fx(&self, id: i64, fx: Option<&TxFx>) -> Result<()> {
        match fx {
            Some(f) => {
                sqlx::query(
                    "INSERT INTO transaction_fx
                       (transaction_id, currency, orig_minor, rate, rate_date, estimated)
                     VALUES (?, ?, ?, ?, ?, ?)
                     ON DUPLICATE KEY UPDATE currency = VALUES(currency),
                       orig_minor = VALUES(orig_minor), rate = VALUES(rate),
                       rate_date = VALUES(rate_date), estimated = VALUES(estimated)",
                )
                .bind(id)
                .bind(&f.currency)
                .bind(f.orig_minor)
                .bind(f.rate)
                .bind(&f.rate_date)
                .bind(f.estimated)
                .execute(&self.pool)
                .await?
            }
            None => {
                sqlx::query("DELETE FROM transaction_fx WHERE transaction_id = ?")
                    .bind(id)
                    .execute(&self.pool)
                    .await?
            }
        };
        Ok(())
    }

    async fn list_tx_fx(&self) -> Result<Vec<TxFx>> {
        let rows = sqlx::query(
            "SELECT f.transaction_id, f.currency, f.orig_minor, f.rate, f.rate_date,
                    CAST(f.estimated AS SIGNED) AS estimated
             FROM transaction_fx f JOIN transactions t ON t.id = f.transaction_id
             ORDER BY f.transaction_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(TxFx {
                    transaction_id: r.try_get("transaction_id")?,
                    currency: r.try_get("currency")?,
                    orig_minor: r.try_get("orig_minor")?,
                    rate: r.try_get("rate")?,
                    rate_date: r.try_get("rate_date")?,
                    estimated: r.try_get::<i64, _>("estimated")? != 0,
                })
            })
            .collect()
    }

    async fn get_fx_rate(&self, currency: &str, date: &str) -> Result<Option<f64>> {
        let row = sqlx::query("SELECT rate FROM fx_rates WHERE currency = ? AND date = ?")
            .bind(currency)
            .bind(date)
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => Some(r.try_get("rate")?),
            None => None,
        })
    }

    async fn put_fx_rate(&self, currency: &str, date: &str, rate: f64) -> Result<()> {
        sqlx::query(
            "INSERT INTO fx_rates (currency, date, rate) VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE rate = VALUES(rate)",
        )
        .bind(currency)
        .bind(date)
        .bind(rate)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn set_transactions_business(&self, ids: &[i64], business: Option<i64>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for id in ids {
            match business {
                Some(b) => {
                    sqlx::query(
                        "INSERT INTO transaction_business (transaction_id, business_id)
                         VALUES (?, ?)
                         ON DUPLICATE KEY UPDATE business_id = VALUES(business_id)",
                    )
                    .bind(id)
                    .bind(b)
                    .execute(&mut *tx)
                    .await?
                }
                None => {
                    sqlx::query("DELETE FROM transaction_business WHERE transaction_id = ?")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?
                }
            };
        }
        tx.commit().await?;
        Ok(())
    }

    async fn pending_mirrors(&self) -> Result<Vec<PendingMirror>> {
        let rows = sqlx::query(
            "SELECT t.id, t.sync_state, t.date, t.amount_agorot, t.note,
                    COALESCE(c.paamonim_item_id, (SELECT s.paamonim_item_id FROM categories s
                       JOIN categories sp ON sp.id = s.parent_id AND NOT sp.deleted
                       WHERE NOT s.deleted AND s.id <> c.id AND s.name = c.name
                         AND s.is_income = c.is_income
                         AND (s.paamonim_item_id LIKE 'e:%' OR s.paamonim_item_id LIKE 'i:%')
                         AND COALESCE(sp.linked_to, sp.id) = COALESCE(pc.linked_to, pc.id)
                       ORDER BY s.id LIMIT 1)) AS item_key,
                    c.name AS item_name,
                    p.paamonim_id AS payment_key, t.paamonim_line_id AS line_id,
                    t.category_id, COALESCE(pc.paamonim_item_id, lc.paamonim_item_id) AS parent_key,
                    CAST(COALESCE(c.is_income, 0) AS SIGNED) AS is_income
             FROM transactions t
             LEFT JOIN categories c ON c.id = t.category_id
             LEFT JOIN categories pc ON pc.id = c.parent_id AND NOT pc.deleted
             LEFT JOIN categories lc ON lc.id = pc.linked_to AND NOT lc.deleted
             LEFT JOIN payment_methods p ON p.id = t.payment_method_id
             WHERE t.sync_state IN ('pending', 'modified', 'verifying') ORDER BY t.id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(PendingMirror {
                    id: r.try_get("id")?,
                    state: r.try_get("sync_state")?,
                    date: r.try_get("date")?,
                    amount_agorot: r.try_get("amount_agorot")?,
                    note: r.try_get("note")?,
                    item_key: r.try_get("item_key")?,
                    item_name: r.try_get("item_name")?,
                    payment_key: r.try_get("payment_key")?,
                    line_id: r.try_get("line_id")?,
                    category_id: r.try_get("category_id")?,
                    parent_key: r.try_get("parent_key")?,
                    is_income: r.try_get::<i64, _>("is_income")? != 0,
                })
            })
            .collect()
    }

    async fn set_mirror_state(&self, id: i64, state: &str) -> Result<()> {
        sqlx::query("UPDATE transactions SET sync_state = ? WHERE id = ?")
            .bind(state)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn mark_mirrored(&self, p: &PendingMirror, line_id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT IGNORE INTO mirror_deletes (line_id, date)
             SELECT paamonim_line_id, COALESCE(mirror_date, date) FROM transactions
             WHERE id = ? AND paamonim_line_id IS NOT NULL AND paamonim_line_id <> ?",
        )
        .bind(p.id)
        .bind(line_id)
        .execute(&mut *tx)
        .await?;
        let res = sqlx::query(
            "UPDATE transactions SET paamonim_line_id = ?, mirror_date = ?,
               sync_state = CASE WHEN date = ? AND amount_agorot = ? AND note = ?
                            THEN 'synced' ELSE 'modified' END
             WHERE id = ?",
        )
        .bind(line_id)
        .bind(&p.date)
        .bind(&p.date)
        .bind(p.amount_agorot)
        .bind(&p.note)
        .bind(p.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        // MySQL counts only changed rows; the line id always changes here.
        Ok(res.rows_affected() == 1)
    }

    async fn link_category(&self, id: i64, key: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        // An item another category already holds stays with it; lines are still sent there.
        let taken: Option<i64> =
            sqlx::query_scalar("SELECT id FROM categories WHERE paamonim_item_id = ?")
                .bind(key)
                .fetch_optional(&mut *tx)
                .await?;
        let n = if taken.is_some() {
            0
        } else {
            sqlx::query(
                "UPDATE categories SET paamonim_item_id = ? WHERE id = ? AND paamonim_item_id IS NULL",
            )
            .bind(key)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
        };
        if n == 1 {
            sqlx::query(
                "UPDATE transactions SET sync_state = 'pending'
                 WHERE category_id = ? AND sync_state = 'skipped'",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(n == 1)
    }

    async fn detach_mirror(&self, id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT IGNORE INTO mirror_deletes (line_id, date)
             SELECT paamonim_line_id, COALESCE(mirror_date, date) FROM transactions
             WHERE id = ? AND paamonim_line_id IS NOT NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE transactions SET paamonim_line_id = NULL, mirror_date = NULL,
               sync_state = 'skipped' WHERE id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn retry_mirror(&self, id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE transactions SET sync_state =
               CASE WHEN paamonim_line_id IS NULL THEN 'pending' ELSE 'modified' END
             WHERE id = ? AND sync_state = 'error'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    async fn requeue_skipped_sub_categories(&self) -> Result<usize> {
        let res = sqlx::query(REQUEUE_SKIPPED_SQL).execute(&self.pool).await?;
        Ok(res.rows_affected() as usize)
    }

    async fn raw_backup(&self) -> Result<RawBackup> {
        let categories = sqlx::query(
            "SELECT id, name, CAST(is_income AS SIGNED) AS is_income, paamonim_item_id, parent_id,
                    CAST(deleted AS SIGNED) AS deleted, merged_into, linked_to
             FROM categories ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| {
            Ok(RawCategory {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                is_income: r.try_get::<i64, _>("is_income")? != 0,
                paamonim_item_id: r.try_get("paamonim_item_id")?,
                parent_id: r.try_get("parent_id")?,
                deleted: r.try_get::<i64, _>("deleted")? != 0,
                merged_into: r.try_get("merged_into")?,
                linked_to: r.try_get("linked_to")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        let payment_methods =
            sqlx::query("SELECT id, name, paamonim_id FROM payment_methods ORDER BY id")
                .fetch_all(&self.pool)
                .await?
                .iter()
                .map(|r| {
                    Ok(RawPaymentMethod {
                        id: r.try_get("id")?,
                        name: r.try_get("name")?,
                        paamonim_id: r.try_get("paamonim_id")?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
        let tx_links =
            sqlx::query("SELECT id, paamonim_line_id, mirror_date FROM transactions ORDER BY id")
                .fetch_all(&self.pool)
                .await?
                .iter()
                .map(|r| {
                    Ok((
                        r.try_get("id")?,
                        r.try_get("paamonim_line_id")?,
                        r.try_get("mirror_date")?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
        let meta = sqlx::query("SELECT k, v FROM mirror_meta ORDER BY k")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| Ok((r.try_get("k")?, r.try_get("v")?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(RawBackup {
            categories,
            payment_methods,
            tx_links,
            meta,
        })
    }

    async fn pending_line_deletes(&self) -> Result<Vec<LineDelete>> {
        let rows = sqlx::query("SELECT line_id, date FROM mirror_deletes ORDER BY seq")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(LineDelete {
                    line_id: r.try_get("line_id")?,
                    date: r.try_get("date")?,
                })
            })
            .collect()
    }

    async fn queue_line_delete(&self, line_id: &str, date: &str) -> Result<()> {
        sqlx::query("INSERT IGNORE INTO mirror_deletes (line_id, date) VALUES (?, ?)")
            .bind(line_id)
            .bind(date)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn finish_line_delete(&self, line_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM mirror_deletes WHERE line_id = ?")
            .bind(line_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn known_line_ids(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT paamonim_line_id AS id FROM transactions WHERE paamonim_line_id IS NOT NULL
             UNION SELECT line_id AS id FROM mirror_deletes",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(|r| Ok(r.try_get("id")?)).collect()
    }

    async fn baseline_mirror(&self) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        let done: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mirror_meta WHERE k = 'baseline'")
            .fetch_one(&mut *tx)
            .await?;
        let mut n = 0;
        if done == 0 {
            // Imported lines keep their Paamonim day from here on.
            sqlx::query(
                "UPDATE transactions SET mirror_date = date
                 WHERE paamonim_line_id IS NOT NULL AND mirror_date IS NULL AND sync_state = 'synced'",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE transactions SET sync_state = 'synced'
                 WHERE sync_state = 'modified' AND paamonim_line_id IS NOT NULL",
            )
            .execute(&mut *tx)
            .await?;
            n = sqlx::query(
                "UPDATE transactions SET sync_state = 'local'
                 WHERE sync_state IN ('pending', 'modified', 'verifying', 'error', 'skipped')",
            )
            .execute(&mut *tx)
            .await?
            .rows_affected() as usize;
            sqlx::query("DELETE FROM mirror_deletes")
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO mirror_meta (k, v) VALUES ('baseline', '1')")
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(n)
    }

    async fn clear_mirror_baseline(&self) -> Result<()> {
        sqlx::query("DELETE FROM mirror_meta WHERE k = 'baseline'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn import_paamonim(&self, batch: &ImportBatch) -> Result<ImportSummary> {
        let mut tx = self.pool.begin().await?;
        let mut sum = ImportSummary::default();

        // `cats`: key -> row id, for linking parents. `live`: key -> where its lines go, which
        // differs for categories deleted locally (their replacement, or none).
        let mut cats: HashMap<String, i64> = HashMap::new();
        let mut live: HashMap<String, Option<i64>> = HashMap::new();
        for c in &batch.categories {
            let existing: Option<(i64, bool)> =
                sqlx::query_as("SELECT id, deleted FROM categories WHERE paamonim_item_id = ?")
                    .bind(&c.key)
                    .fetch_optional(&mut *tx)
                    .await?;
            let parent = c.parent_key.as_ref().and_then(|k| cats.get(k)).copied();
            let id = match existing {
                // Deleted here: stays deleted, and its name/parent are not refreshed.
                Some((id, true)) => id,
                Some((id, false)) => {
                    // A sub-category moved here under a category linked to its Paamonim
                    // category keeps its place.
                    let kept: Option<i64> = match parent {
                        Some(p) => sqlx::query_scalar(
                            "SELECT c.parent_id FROM categories c
                             JOIN categories pc ON pc.id = c.parent_id
                             WHERE c.id = ? AND pc.linked_to = ?",
                        )
                        .bind(id)
                        .bind(p)
                        .fetch_optional(&mut *tx)
                        .await?
                        .flatten(),
                        None => None,
                    };
                    sqlx::query("UPDATE categories SET name = ?, parent_id = ? WHERE id = ?")
                        .bind(&c.name)
                        .bind(kept.or(parent))
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    id
                }
                None => {
                    sum.categories_added += 1;
                    // New under a parent that was deleted here: deleted too, same destination.
                    let into = match parent {
                        Some(p) => Some(resolve_category(&mut tx, p).await?),
                        None => None,
                    };
                    let parent_gone = parent.is_some() && into != Some(parent);
                    let into = into.flatten().filter(|_| parent_gone);
                    let res = sqlx::query(
                        "INSERT INTO categories (name, is_income, paamonim_item_id, parent_id, deleted, merged_into)
                         VALUES (?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&c.name)
                    .bind(c.is_income)
                    .bind(&c.key)
                    .bind(parent)
                    .bind(parent_gone)
                    .bind(into)
                    .execute(&mut *tx)
                    .await?;
                    res.last_insert_id() as i64
                }
            };
            live.insert(c.key.clone(), resolve_category(&mut tx, id).await?);
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
        let queued: HashSet<String> = sqlx::query_scalar("SELECT line_id FROM mirror_deletes")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
        for t in &batch.transactions {
            let cat = t
                .category_key
                .as_ref()
                .and_then(|k| live.get(k))
                .copied()
                .flatten();
            let pm = t.payment_key.as_ref().and_then(|k| pms.get(k)).copied();
            // Deleted here and waiting to be deleted there: don't bring it back.
            if queued.contains(&t.line_id) {
                sum.transactions_skipped += 1;
                continue;
            }
            if !known.contains(&t.line_id) {
                // A line we posted whose id we never recorded: link it, keeping the exact
                // local amount, rather than adding it a second time.
                let candidates: Vec<(i64, i64)> = sqlx::query_as(
                    "SELECT id, amount_agorot FROM transactions
                     WHERE paamonim_line_id IS NULL AND sync_state IN ('verifying', 'error')
                       AND date = ? AND category_id <=> ? ORDER BY id",
                )
                .bind(&t.date)
                .bind(cat)
                .fetch_all(&mut *tx)
                .await?;
                if let Some((id, _)) = candidates
                    .into_iter()
                    .find(|(_, a)| rounds_to(*a, t.amount_agorot))
                {
                    sqlx::query(
                        "UPDATE transactions SET paamonim_line_id = ?, mirror_date = date,
                           sync_state = 'synced', payment_method_id = COALESCE(payment_method_id, ?)
                         WHERE id = ?",
                    )
                    .bind(&t.line_id)
                    .bind(pm)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    sum.transactions_adopted += 1;
                    continue;
                }
            }
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
                       (date, amount_agorot, category_id, payment_method_id, note, paamonim_line_id,
                        mirror_date, sync_state)
                     VALUES (?, ?, ?, ?, ?, ?, ?, 'synced')",
                )
                .bind(&t.date)
                .bind(t.amount_agorot)
                .bind(cat)
                .bind(pm)
                .bind(&t.note)
                .bind(&t.line_id)
                .bind(&t.date)
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
            "SELECT id, date, amount_agorot, category_id, payment_method_id, note, sync_state,
                    recurring_series_id
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
            // mirror_date first: MySQL applies SET left to right, so it still sees the old date.
            "UPDATE transactions SET mirror_date = CASE WHEN paamonim_line_id IS NULL THEN NULL
                                                   ELSE COALESCE(mirror_date, date) END,
               date = ?, amount_agorot = ?, category_id = ?,
               payment_method_id = ?, note = ?,
               sync_state = CASE WHEN sync_state = 'synced' THEN 'modified'
                                 WHEN sync_state = 'skipped' THEN 'pending' ELSE sync_state END
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
        let mut tx = self.pool.begin().await?;
        // Queue its Paamonim line (if any) for deletion there.
        sqlx::query(
            "INSERT IGNORE INTO mirror_deletes (line_id, date)
             SELECT paamonim_line_id, COALESCE(mirror_date, date) FROM transactions
             WHERE id = ? AND paamonim_line_id IS NOT NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM transactions WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM transaction_fx WHERE transaction_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn list_categories(&self) -> Result<Vec<Category>> {
        let rows = sqlx::query(
            "SELECT id, name, is_income, parent_id,
                    CAST(paamonim_item_id IS NOT NULL AS SIGNED) AS from_paamonim, linked_to
             FROM categories WHERE NOT deleted ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Category {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    is_income: r.try_get("is_income")?,
                    parent_id: r.try_get("parent_id")?,
                    from_paamonim: r.try_get::<i64, _>("from_paamonim")? != 0,
                    linked_to: r.try_get("linked_to")?,
                })
            })
            .collect()
    }

    async fn set_category_link(&self, id: i64, linked_to: Option<i64>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE categories SET linked_to = ? WHERE id = ?")
            .bind(linked_to)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(REQUEUE_SKIPPED_SQL).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_category_parent(&self, id: i64, parent_id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE categories SET parent_id = ? WHERE id = ?")
            .bind(parent_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(REQUEUE_SKIPPED_SQL).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn insert_category(
        &self,
        name: &str,
        is_income: bool,
        parent_id: Option<i64>,
    ) -> Result<i64> {
        let res =
            sqlx::query("INSERT INTO categories (name, is_income, parent_id) VALUES (?, ?, ?)")
                .bind(name)
                .bind(is_income)
                .bind(parent_id)
                .execute(&self.pool)
                .await?;
        Ok(res.last_insert_id() as i64)
    }

    async fn remove_categories(&self, ids: &[i64], move_to: Option<i64>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for id in ids {
            if let Some(to) = move_to {
                // Like any local edit: a line already in Paamonim now differs from it.
                sqlx::query(
                    "UPDATE transactions SET category_id = ?,
                       sync_state = CASE WHEN sync_state = 'synced' THEN 'modified'
                                 WHEN sync_state = 'skipped' THEN 'pending' ELSE sync_state END
                     WHERE category_id = ?",
                )
                .bind(to)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE recurring SET category_id = ? WHERE category_id = ?")
                    .bind(to)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            let from_paamonim: bool = sqlx::query_scalar(
                "SELECT paamonim_item_id IS NOT NULL FROM categories WHERE id = ?",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if from_paamonim {
                sqlx::query("UPDATE categories SET deleted = TRUE, merged_into = ? WHERE id = ?")
                    .bind(move_to)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            } else {
                sqlx::query("DELETE FROM categories WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_recurring(&self) -> Result<Vec<RecurringSegment>> {
        let rows = sqlx::query(
            "SELECT id, series_id, start_month, end_month, day, amount_agorot, category_id,
                    payment_method_id, note, currency, orig_minor
             FROM recurring ORDER BY series_id, start_month",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(RecurringSegment {
                    id: r.try_get("id")?,
                    series_id: r.try_get("series_id")?,
                    start_month: r.try_get("start_month")?,
                    end_month: r.try_get("end_month")?,
                    day: r.try_get::<i32, _>("day")? as u32,
                    amount_agorot: r.try_get("amount_agorot")?,
                    category_id: r.try_get("category_id")?,
                    payment_method_id: r.try_get("payment_method_id")?,
                    note: r.try_get("note")?,
                    currency: r.try_get("currency")?,
                    orig_minor: r.try_get("orig_minor")?,
                })
            })
            .collect()
    }

    async fn insert_recurring(&self, series_id: Option<i64>, r: &NewRecurring) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let res = sqlx::query(
            "INSERT INTO recurring (series_id, start_month, end_month, day, amount_agorot,
                                    category_id, payment_method_id, note, currency, orig_minor)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(series_id.unwrap_or(0))
        .bind(&r.start_month)
        .bind(&r.end_month)
        .bind(r.day)
        .bind(r.amount_agorot)
        .bind(r.category_id)
        .bind(r.payment_method_id)
        .bind(&r.note)
        .bind(&r.currency)
        .bind(r.orig_minor)
        .execute(&mut *tx)
        .await?;
        let id = res.last_insert_id() as i64;
        if series_id.is_none() {
            sqlx::query("UPDATE recurring SET series_id = id WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    async fn update_recurring(&self, id: i64, r: &NewRecurring) -> Result<()> {
        sqlx::query(
            "UPDATE recurring SET start_month = ?, end_month = ?, day = ?, amount_agorot = ?,
               category_id = ?, payment_method_id = ?, note = ?, currency = ?, orig_minor = ?
             WHERE id = ?",
        )
        .bind(&r.start_month)
        .bind(&r.end_month)
        .bind(r.day)
        .bind(r.amount_agorot)
        .bind(r.category_id)
        .bind(r.payment_method_id)
        .bind(&r.note)
        .bind(&r.currency)
        .bind(r.orig_minor)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_recurring(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM recurring WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn settled_recurring(&self) -> Result<Vec<(i64, String)>> {
        let rows = sqlx::query("SELECT series_id, month FROM recurring_settled")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| Ok((r.try_get("series_id")?, r.try_get("month")?)))
            .collect()
    }

    async fn settle_recurring_month(
        &self,
        series_id: i64,
        month: &str,
        t: Option<&NewTransaction>,
        fx: Option<&TxFx>,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let new =
            sqlx::query("INSERT IGNORE INTO recurring_settled (series_id, month) VALUES (?, ?)")
                .bind(series_id)
                .bind(month)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if let (1, Some(t)) = (new, t) {
            let res = sqlx::query(
                "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id,
                                           note, sync_state, recurring_series_id)
                 VALUES (?, ?, ?, ?, ?, 'pending', ?)",
            )
            .bind(&t.date)
            .bind(t.amount_agorot)
            .bind(t.category_id)
            .bind(t.payment_method_id)
            .bind(&t.note)
            .bind(series_id)
            .execute(&mut *tx)
            .await?;
            if let Some(f) = fx {
                insert_tx_fx(&mut tx, res.last_insert_id() as i64, f).await?;
            }
        }
        tx.commit().await?;
        Ok(new == 1)
    }

    async fn settle_recurring(&self, due: &[Occurrence]) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        let mut added = 0;
        for o in due {
            let new = sqlx::query(
                "INSERT IGNORE INTO recurring_settled (series_id, month) VALUES (?, ?)",
            )
            .bind(o.series_id)
            .bind(&o.date[..7])
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if new == 1 {
                let res = sqlx::query(
                    "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id,
                                               note, sync_state, recurring_series_id)
                     VALUES (?, ?, ?, ?, ?, 'pending', ?)",
                )
                .bind(&o.date)
                .bind(o.amount_agorot)
                .bind(o.category_id)
                .bind(o.payment_method_id)
                .bind(&o.note)
                .bind(o.series_id)
                .execute(&mut *tx)
                .await?;
                if let Some(f) = &o.fx {
                    insert_tx_fx(&mut tx, res.last_insert_id() as i64, f).await?;
                }
                added += 1;
            }
        }
        tx.commit().await?;
        Ok(added)
    }

    async fn log_append(&self, e: &NewLogEntry) -> Result<()> {
        sqlx::query(
            "INSERT INTO change_log (at, kind, entity_id, old_value, new_value)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&e.at)
        .bind(&e.kind)
        .bind(e.entity_id)
        .bind(e.old.as_ref().map(|v| v.to_string()))
        .bind(e.new.as_ref().map(|v| v.to_string()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_log(&self, limit: usize, before_id: Option<i64>) -> Result<Vec<LogEntry>> {
        let rows = sqlx::query(
            "SELECT id, at, kind, entity_id, old_value, new_value FROM change_log
             WHERE id < ? ORDER BY id DESC LIMIT ?",
        )
        .bind(before_id.unwrap_or(i64::MAX))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                let json = |col: &str| -> Result<Option<serde_json::Value>> {
                    let s: Option<String> = r.try_get(col)?;
                    Ok(s.and_then(|s| s.parse().ok()))
                };
                Ok(LogEntry {
                    id: r.try_get("id")?,
                    at: r.try_get("at")?,
                    kind: r.try_get("kind")?,
                    entity_id: r.try_get("entity_id")?,
                    old: json("old_value")?,
                    new: json("new_value")?,
                })
            })
            .collect()
    }

    async fn prune_log(&self, before: &str) -> Result<usize> {
        let res = sqlx::query("DELETE FROM change_log WHERE at < ?")
            .bind(before)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() as usize)
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
