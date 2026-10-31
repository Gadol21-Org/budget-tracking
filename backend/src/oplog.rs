//! Operation log: what changed in the data, with the values before and after, and when.
//!
//! [`Logged`] wraps a [`Storage`] and records every change made through it (transactions,
//! categories, repeating transactions settling, imports), so neither backend has to know
//! about it. Series-level edits of repeating rules are recorded where they are decided, in
//! `recurring.rs`, via [`record`]. Snapshots are self-contained (category and payment method
//! are stored by name), so an entry still reads correctly after the category is deleted.
//! Entries older than [`RETENTION_DAYS`] are pruned.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};

use crate::recurring::{NewRecurring, Occurrence, RecurringSegment};
use crate::storage::{
    Category, ImportBatch, ImportSummary, LineDelete, MonthlyTotal, NewCategory, NewTransaction,
    PaymentMethod, PendingMirror, Storage, Transaction,
};

/// How long entries are kept.
pub const RETENTION_DAYS: i64 = 365;

/// What happened; the UI shows these in Hebrew.
pub mod kind {
    pub const TX_CREATE: &str = "transaction_create";
    pub const TX_UPDATE: &str = "transaction_update";
    pub const TX_DELETE: &str = "transaction_delete";
    pub const CATEGORY_CREATE: &str = "category_create";
    /// Also covers the move of its transactions and rules to another category.
    pub const CATEGORY_DELETE: &str = "category_delete";
    /// A top-level category linked to another Paamonim category.
    pub const CATEGORY_LINK: &str = "category_link";
    /// A sub-category moved under another top-level category.
    pub const CATEGORY_MOVE: &str = "category_move";
    pub const RECURRING_CREATE: &str = "recurring_create";
    pub const RECURRING_UPDATE: &str = "recurring_update";
    pub const RECURRING_CANCEL: &str = "recurring_cancel";
    /// A month of a series edited on its own.
    pub const RECURRING_OVERRIDE: &str = "recurring_override";
    pub const RECURRING_SKIP: &str = "recurring_skip";
    /// A due occurrence turned into a transaction.
    pub const RECURRING_SETTLE: &str = "recurring_settle";
    pub const IMPORT: &str = "import";
}

pub struct NewLogEntry {
    /// UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub at: String,
    pub kind: String,
    /// Transaction, category or series id, when there is one.
    pub entity_id: Option<i64>,
    pub old: Option<Value>,
    pub new: Option<Value>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub id: i64,
    pub at: String,
    pub kind: String,
    pub entity_id: Option<i64>,
    pub old: Option<Value>,
    pub new: Option<Value>,
}

pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Entries before this timestamp are due for pruning.
pub fn cutoff(now: chrono::DateTime<chrono::Utc>) -> String {
    (now - chrono::Duration::days(RETENTION_DAYS))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Drop entries past retention. Returns how many.
pub async fn prune(db: &dyn Storage) -> anyhow::Result<usize> {
    db.prune_log(&cutoff(chrono::Utc::now())).await
}

/// Append an entry. A failure to log must not fail the change that was just made.
pub async fn record(
    db: &dyn Storage,
    kind: &str,
    entity_id: Option<i64>,
    old: Option<Value>,
    new: Option<Value>,
) {
    let entry = NewLogEntry {
        at: now(),
        kind: kind.into(),
        entity_id,
        old,
        new,
    };
    if let Err(e) = db.log_append(&entry).await {
        tracing::warn!("writing the change log failed: {e}");
    }
}

fn category_path(cats: &[Category], id: Option<i64>) -> Option<String> {
    let c = cats.iter().find(|c| Some(c.id) == id)?;
    Some(
        match c.parent_id.and_then(|p| cats.iter().find(|x| x.id == p)) {
            Some(p) => format!("{} › {}", p.name, c.name),
            None => c.name.clone(),
        },
    )
}

fn payment_name(pms: &[PaymentMethod], id: Option<i64>) -> Option<String> {
    pms.iter()
        .find(|p| Some(p.id) == id)
        .map(|p| p.name.clone())
}

/// Names for ids, read once per entry.
struct Names {
    cats: Vec<Category>,
    pms: Vec<PaymentMethod>,
}

impl Names {
    async fn load(db: &dyn Storage) -> anyhow::Result<Self> {
        Ok(Self {
            cats: db.list_categories().await?,
            pms: db.list_payment_methods().await?,
        })
    }

    fn tx(
        &self,
        date: &str,
        amount_agorot: i64,
        category_id: Option<i64>,
        payment_method_id: Option<i64>,
        note: &str,
    ) -> Value {
        json!({
            "date": date,
            "amount_agorot": amount_agorot,
            "category": category_path(&self.cats, category_id),
            "payment_method": payment_name(&self.pms, payment_method_id),
            "note": note,
        })
    }

    fn new_tx(&self, t: &NewTransaction) -> Value {
        self.tx(
            &t.date,
            t.amount_agorot,
            t.category_id,
            t.payment_method_id,
            &t.note,
        )
    }

    fn stored_tx(&self, t: &Transaction) -> Value {
        self.tx(
            &t.date,
            t.amount_agorot,
            t.category_id,
            t.payment_method_id,
            &t.note,
        )
    }

    fn occurrence(&self, o: &Occurrence) -> Value {
        self.tx(
            &o.date,
            o.amount_agorot,
            o.category_id,
            o.payment_method_id,
            &o.note,
        )
    }

    fn rule(&self, r: &NewRecurring) -> Value {
        json!({
            "start_month": r.start_month,
            "end_month": r.end_month,
            "day": r.day,
            "amount_agorot": r.amount_agorot,
            "category": category_path(&self.cats, r.category_id),
            "payment_method": payment_name(&self.pms, r.payment_method_id),
            "note": r.note,
        })
    }
}

/// A repeating rule as a log snapshot.
pub async fn rule_value(db: &dyn Storage, r: &NewRecurring) -> Option<Value> {
    Some(Names::load(db).await.ok()?.rule(r))
}

/// A stored segment as a log snapshot.
pub async fn segment_value(db: &dyn Storage, s: &RecurringSegment) -> Option<Value> {
    rule_value(db, &s.to_new()).await
}

/// Storage that records every change to the operation log.
pub struct Logged(pub Arc<dyn Storage>);

impl Logged {
    async fn names(&self) -> Option<Names> {
        match Names::load(self.0.as_ref()).await {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!("reading names for the change log failed: {e}");
                None
            }
        }
    }

    async fn log(&self, kind: &str, id: Option<i64>, old: Option<Value>, new: Option<Value>) {
        record(self.0.as_ref(), kind, id, old, new).await;
    }
}

#[async_trait::async_trait]
impl Storage for Logged {
    async fn list_payment_methods(&self) -> anyhow::Result<Vec<PaymentMethod>> {
        self.0.list_payment_methods().await
    }
    // Mirroring only changes sync bookkeeping, not the user's data: forwarded, not logged.
    async fn pending_mirrors(&self) -> anyhow::Result<Vec<PendingMirror>> {
        self.0.pending_mirrors().await
    }
    async fn set_mirror_state(&self, id: i64, state: &str) -> anyhow::Result<()> {
        self.0.set_mirror_state(id, state).await
    }
    async fn mark_mirrored(&self, posted: &PendingMirror, line_id: &str) -> anyhow::Result<bool> {
        self.0.mark_mirrored(posted, line_id).await
    }
    async fn link_category(&self, id: i64, key: &str) -> anyhow::Result<bool> {
        self.0.link_category(id, key).await
    }
    async fn detach_mirror(&self, id: i64) -> anyhow::Result<()> {
        self.0.detach_mirror(id).await
    }
    async fn retry_mirror(&self, id: i64) -> anyhow::Result<bool> {
        self.0.retry_mirror(id).await
    }
    async fn requeue_skipped_sub_categories(&self) -> anyhow::Result<usize> {
        self.0.requeue_skipped_sub_categories().await
    }
    async fn raw_backup(&self) -> anyhow::Result<crate::storage::RawBackup> {
        self.0.raw_backup().await
    }
    async fn pending_line_deletes(&self) -> anyhow::Result<Vec<LineDelete>> {
        self.0.pending_line_deletes().await
    }
    async fn queue_line_delete(&self, line_id: &str, date: &str) -> anyhow::Result<()> {
        self.0.queue_line_delete(line_id, date).await
    }
    async fn finish_line_delete(&self, line_id: &str) -> anyhow::Result<()> {
        self.0.finish_line_delete(line_id).await
    }
    async fn known_line_ids(&self) -> anyhow::Result<Vec<String>> {
        self.0.known_line_ids().await
    }
    async fn baseline_mirror(&self) -> anyhow::Result<usize> {
        self.0.baseline_mirror().await
    }
    async fn clear_mirror_baseline(&self) -> anyhow::Result<()> {
        self.0.clear_mirror_baseline().await
    }

    async fn import_paamonim(&self, batch: &ImportBatch) -> anyhow::Result<ImportSummary> {
        let sum = self.0.import_paamonim(batch).await?;
        if sum.categories_added
            + sum.payment_methods_added
            + sum.transactions_added
            + sum.transactions_adopted
            > 0
        {
            self.log(kind::IMPORT, None, None, serde_json::to_value(&sum).ok())
                .await;
        }
        Ok(sum)
    }

    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
        self.0.list_transactions().await
    }

    async fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64> {
        let id = self.0.create_transaction(t).await?;
        if let Some(n) = self.names().await {
            self.log(kind::TX_CREATE, Some(id), None, Some(n.new_tx(t)))
                .await;
        }
        Ok(id)
    }

    async fn delete_transaction(&self, id: i64) -> anyhow::Result<()> {
        let old = self
            .0
            .list_transactions()
            .await?
            .into_iter()
            .find(|t| t.id == id);
        let names = self.names().await;
        self.0.delete_transaction(id).await?;
        if let (Some(old), Some(n)) = (old, names) {
            self.log(kind::TX_DELETE, Some(id), Some(n.stored_tx(&old)), None)
                .await;
        }
        Ok(())
    }

    async fn update_transaction(&self, id: i64, t: &NewTransaction) -> anyhow::Result<()> {
        let old = self
            .0
            .list_transactions()
            .await?
            .into_iter()
            .find(|x| x.id == id);
        self.0.update_transaction(id, t).await?;
        if let (Some(old), Some(n)) = (old, self.names().await) {
            let (old, new) = (n.stored_tx(&old), n.new_tx(t));
            if old != new {
                self.log(kind::TX_UPDATE, Some(id), Some(old), Some(new))
                    .await;
            }
        }
        Ok(())
    }

    async fn list_categories(&self) -> anyhow::Result<Vec<Category>> {
        self.0.list_categories().await
    }

    async fn insert_category(
        &self,
        name: &str,
        is_income: bool,
        parent_id: Option<i64>,
    ) -> anyhow::Result<i64> {
        self.0.insert_category(name, is_income, parent_id).await
    }

    async fn remove_categories(&self, ids: &[i64], move_to: Option<i64>) -> anyhow::Result<()> {
        self.0.remove_categories(ids, move_to).await
    }

    async fn set_category_link(&self, id: i64, linked_to: Option<i64>) -> anyhow::Result<()> {
        self.0.set_category_link(id, linked_to).await
    }

    async fn set_category_parent(&self, id: i64, parent_id: i64) -> anyhow::Result<()> {
        self.0.set_category_parent(id, parent_id).await
    }

    async fn link_category_to(&self, id: i64, linked_to: Option<i64>) -> anyhow::Result<()> {
        let cats = self.0.list_categories().await?;
        self.0.link_category_to(id, linked_to).await?;
        if let Some(cat) = cats.iter().find(|c| c.id == id) {
            if cat.linked_to != linked_to {
                self.log(
                    kind::CATEGORY_LINK,
                    Some(id),
                    Some(json!({ "name": cat.name, "linked_to": category_path(&cats, cat.linked_to) })),
                    Some(json!({ "name": cat.name, "linked_to": category_path(&cats, linked_to) })),
                )
                .await;
            }
        }
        Ok(())
    }

    async fn move_category(&self, id: i64, to: i64) -> anyhow::Result<()> {
        let cats = self.0.list_categories().await?;
        self.0.move_category(id, to).await?;
        if let Some(cat) = cats.iter().find(|c| c.id == id) {
            if cat.parent_id != Some(to) {
                self.log(
                    kind::CATEGORY_MOVE,
                    Some(id),
                    Some(
                        json!({ "name": cat.name, "parent": category_path(&cats, cat.parent_id) }),
                    ),
                    Some(json!({ "name": cat.name, "parent": category_path(&cats, Some(to)) })),
                )
                .await;
            }
        }
        Ok(())
    }

    async fn create_category(&self, c: &NewCategory) -> anyhow::Result<i64> {
        let id = self.0.create_category(c).await?;
        if let Ok(cats) = self.0.list_categories().await {
            if let Some(made) = cats.iter().find(|x| x.id == id) {
                let new = json!({
                    "name": made.name,
                    "is_income": made.is_income,
                    "parent": category_path(&cats, made.parent_id),
                    "linked_to": category_path(&cats, made.linked_to),
                });
                self.log(kind::CATEGORY_CREATE, Some(id), None, Some(new))
                    .await;
            }
        }
        Ok(id)
    }

    async fn delete_category(&self, id: i64, move_to: Option<i64>) -> anyhow::Result<usize> {
        let cats = self.0.list_categories().await?;
        let moved = self.0.delete_category(id, move_to).await?;
        if let Some(cat) = cats.iter().find(|c| c.id == id) {
            let subs: Vec<&str> = cats
                .iter()
                .filter(|c| c.parent_id == Some(id))
                .map(|c| c.name.as_str())
                .collect();
            let old = json!({
                "name": cat.name,
                "is_income": cat.is_income,
                "parent": category_path(&cats, cat.parent_id),
                "subcategories": subs,
            });
            let new = json!({
                "moved_to": category_path(&cats, move_to),
                "transactions_moved": moved,
            });
            self.log(kind::CATEGORY_DELETE, Some(id), Some(old), Some(new))
                .await;
        }
        Ok(moved)
    }

    async fn list_recurring(&self) -> anyhow::Result<Vec<RecurringSegment>> {
        self.0.list_recurring().await
    }
    async fn insert_recurring(
        &self,
        series_id: Option<i64>,
        r: &NewRecurring,
    ) -> anyhow::Result<i64> {
        self.0.insert_recurring(series_id, r).await
    }
    async fn update_recurring(&self, id: i64, r: &NewRecurring) -> anyhow::Result<()> {
        self.0.update_recurring(id, r).await
    }
    async fn delete_recurring(&self, id: i64) -> anyhow::Result<()> {
        self.0.delete_recurring(id).await
    }

    async fn settle_recurring(&self, due: &[Occurrence]) -> anyhow::Result<usize> {
        let before = self.0.settled_recurring().await?;
        let added = self.0.settle_recurring(due).await?;
        if added > 0 {
            if let Some(n) = self.names().await {
                for o in due {
                    if !before.contains(&(o.series_id, o.date[..7].to_string())) {
                        self.log(
                            kind::RECURRING_SETTLE,
                            Some(o.series_id),
                            None,
                            Some(n.occurrence(o)),
                        )
                        .await;
                    }
                }
            }
        }
        Ok(added)
    }

    async fn settled_recurring(&self) -> anyhow::Result<Vec<(i64, String)>> {
        self.0.settled_recurring().await
    }

    async fn settle_recurring_month(
        &self,
        series_id: i64,
        month: &str,
        t: Option<&NewTransaction>,
    ) -> anyhow::Result<bool> {
        let scheduled = self
            .0
            .list_recurring()
            .await?
            .iter()
            .filter(|s| s.series_id == series_id)
            .flat_map(|s| crate::recurring::occurrences(std::slice::from_ref(s), month))
            .find(|o| &o.date[..7] == month);
        let settled = self.0.settle_recurring_month(series_id, month, t).await?;
        if settled {
            if let Some(n) = self.names().await {
                let old = scheduled.as_ref().map(|o| n.occurrence(o));
                match t {
                    Some(t) => {
                        self.log(
                            kind::RECURRING_OVERRIDE,
                            Some(series_id),
                            old,
                            Some(n.new_tx(t)),
                        )
                        .await
                    }
                    None => {
                        self.log(kind::RECURRING_SKIP, Some(series_id), old, None)
                            .await
                    }
                }
            }
        }
        Ok(settled)
    }

    async fn monthly_totals(&self) -> anyhow::Result<Vec<MonthlyTotal>> {
        self.0.monthly_totals().await
    }

    async fn log_append(&self, e: &NewLogEntry) -> anyhow::Result<()> {
        self.0.log_append(e).await
    }
    async fn list_log(
        &self,
        limit: usize,
        before_id: Option<i64>,
    ) -> anyhow::Result<Vec<LogEntry>> {
        self.0.list_log(limit, before_id).await
    }
    async fn prune_log(&self, before: &str) -> anyhow::Result<usize> {
        self.0.prune_log(before).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteStorage;

    fn db() -> Logged {
        Logged(Arc::new(SqliteStorage::open(":memory:").unwrap()))
    }

    fn tx(date: &str, amount: i64, cat: Option<i64>, note: &str) -> NewTransaction {
        NewTransaction {
            date: date.into(),
            amount_agorot: amount,
            category_id: cat,
            payment_method_id: None,
            note: note.into(),
        }
    }

    fn cat(name: &str, parent: Option<i64>) -> NewCategory {
        NewCategory {
            name: name.into(),
            is_income: false,
            parent_id: parent,
            linked_to: None,
        }
    }

    #[tokio::test]
    async fn transaction_create_edit_delete_are_logged_with_values() {
        let db = db();
        let food = db.create_category(&cat("Food", None)).await.unwrap();
        let id = db
            .create_transaction(&tx("2026-10-03", -1000, Some(food), "lunch"))
            .await
            .unwrap();
        db.update_transaction(id, &tx("2026-10-04", -1500, Some(food), "lunch"))
            .await
            .unwrap();
        // Saving without changing anything is not a change.
        db.update_transaction(id, &tx("2026-10-04", -1500, Some(food), "lunch"))
            .await
            .unwrap();
        db.delete_transaction(id).await.unwrap();
        // Deleting what is not there changes nothing.
        db.delete_transaction(id).await.unwrap();

        let log = db.list_log(50, None).await.unwrap();
        let kinds: Vec<_> = log.iter().rev().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "category_create",
                "transaction_create",
                "transaction_update",
                "transaction_delete"
            ]
        );
        let update = log.iter().find(|e| e.kind == "transaction_update").unwrap();
        assert_eq!(update.entity_id, Some(id));
        assert_eq!(update.old.as_ref().unwrap()["amount_agorot"], -1000);
        assert_eq!(update.new.as_ref().unwrap()["amount_agorot"], -1500);
        assert_eq!(update.new.as_ref().unwrap()["category"], "Food");
        let deleted = log.iter().find(|e| e.kind == "transaction_delete").unwrap();
        assert_eq!(deleted.old.as_ref().unwrap()["date"], "2026-10-04");
        assert!(deleted.new.is_none());
    }

    #[tokio::test]
    async fn deleting_a_category_records_where_its_lines_went() {
        let db = db();
        let a = db.create_category(&cat("A", None)).await.unwrap();
        let a1 = db.create_category(&cat("A1", Some(a))).await.unwrap();
        let b = db.create_category(&cat("B", None)).await.unwrap();
        db.create_transaction(&tx("2026-10-03", -100, Some(a1), ""))
            .await
            .unwrap();
        // Refused deletes leave no trace.
        assert!(db.delete_category(a, None).await.is_err());
        db.delete_category(a, Some(b)).await.unwrap();
        let log = db.list_log(50, None).await.unwrap();
        let del = log.iter().find(|e| e.kind == "category_delete").unwrap();
        assert_eq!(del.old.as_ref().unwrap()["subcategories"][0], "A1");
        assert_eq!(del.new.as_ref().unwrap()["moved_to"], "B");
        assert_eq!(del.new.as_ref().unwrap()["transactions_moved"], 1);
        assert_eq!(
            log.iter().filter(|e| e.kind == "category_delete").count(),
            1
        );
    }

    #[tokio::test]
    async fn settling_overriding_and_skipping_are_logged() {
        use crate::recurring;
        let db = db();
        let rule = |month: &str| NewRecurring {
            start_month: month.into(),
            end_month: None,
            day: 1,
            amount_agorot: -500,
            category_id: None,
            payment_method_id: None,
            note: "rent".into(),
        };
        let series = recurring::create(&db, &rule("2026-01")).await.unwrap();
        recurring::settle_month(
            &db,
            series,
            "2026-03",
            &tx("2026-03-05", -700, None, "rent"),
        )
        .await
        .unwrap();
        recurring::skip_month(&db, series, "2026-04").await.unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 5, 15).unwrap();
        let added = recurring::settle_due(&db, today).await.unwrap();
        assert_eq!(added, 3); // Jan, Feb, May: Mar and Apr already settled

        let log = db.list_log(50, None).await.unwrap();
        let count = |k: &str| log.iter().filter(|e| e.kind == k).count();
        assert_eq!(
            (
                count("recurring_create"),
                count("recurring_override"),
                count("recurring_skip"),
                count("recurring_settle")
            ),
            (1, 1, 1, 3)
        );
        let over = log.iter().find(|e| e.kind == "recurring_override").unwrap();
        assert_eq!(over.old.as_ref().unwrap()["amount_agorot"], -500);
        assert_eq!(over.new.as_ref().unwrap()["amount_agorot"], -700);
    }

    #[tokio::test]
    async fn series_changes_and_cancellations_are_logged() {
        use crate::recurring;
        let db = db();
        let rule = |month: &str, amount: i64| NewRecurring {
            start_month: month.into(),
            end_month: None,
            day: 1,
            amount_agorot: amount,
            category_id: None,
            payment_method_id: None,
            note: String::new(),
        };
        let today = chrono::NaiveDate::from_ymd_opt(2026, 5, 15).unwrap();
        let series = recurring::create(&db, &rule("2026-01", -500))
            .await
            .unwrap();
        recurring::change_from(&db, series, &rule("2026-08", -900), today)
            .await
            .unwrap();
        recurring::cancel_from(&db, series, "2026-10", today)
            .await
            .unwrap();
        let log = db.list_log(50, None).await.unwrap();
        let upd = log.iter().find(|e| e.kind == "recurring_update").unwrap();
        assert_eq!(upd.old.as_ref().unwrap()["amount_agorot"], -500);
        assert_eq!(upd.new.as_ref().unwrap()["amount_agorot"], -900);
        assert_eq!(upd.new.as_ref().unwrap()["start_month"], "2026-08");
        let cancel = log.iter().find(|e| e.kind == "recurring_cancel").unwrap();
        assert_eq!(cancel.new.as_ref().unwrap()["from_month"], "2026-10");
    }

    #[tokio::test]
    async fn import_is_logged_only_when_it_adds_something() {
        let db = db();
        let batch = ImportBatch {
            categories: vec![crate::storage::ImportCategory {
                key: "e:1".into(),
                name: "Food".into(),
                is_income: false,
                parent_key: None,
            }],
            ..Default::default()
        };
        db.import_paamonim(&batch).await.unwrap();
        db.import_paamonim(&batch).await.unwrap();
        let log = db.list_log(50, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].new.as_ref().unwrap()["categories_added"], 1);
    }

    #[tokio::test]
    async fn pruning_drops_only_entries_older_than_a_year() {
        let db = db();
        let at = |s: &str| NewLogEntry {
            at: s.into(),
            kind: "transaction_create".into(),
            entity_id: None,
            old: None,
            new: None,
        };
        db.log_append(&at("2025-01-01T00:00:00Z")).await.unwrap();
        db.log_append(&at("2025-11-02T00:00:00Z")).await.unwrap();
        db.log_append(&at("2026-10-30T00:00:00Z")).await.unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-30T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(cutoff(now), "2025-10-30T12:00:00Z");
        assert_eq!(db.prune_log(&cutoff(now)).await.unwrap(), 1);
        let left: Vec<_> = db
            .list_log(50, None)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.at)
            .collect();
        assert_eq!(left, ["2026-10-30T00:00:00Z", "2025-11-02T00:00:00Z"]);
    }

    #[tokio::test]
    async fn list_log_pages_newest_first() {
        let db = db();
        for i in 0..5 {
            db.create_transaction(&tx("2026-10-03", i, None, ""))
                .await
                .unwrap();
        }
        let first = db.list_log(2, None).await.unwrap();
        assert_eq!(first.len(), 2);
        let next = db.list_log(10, Some(first[1].id)).await.unwrap();
        assert_eq!(next.len(), 3);
        assert!(next[0].id < first[1].id);
    }
}
