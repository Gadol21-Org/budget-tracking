//! Initial (and repeatable) import from Paamonim into [`Storage`].

use anyhow::Result;

use crate::money::from_shekels;
use crate::paamonim::{BudgetItem, Client, HistoryEntry, Kind};
use crate::storage::{
    ImportBatch, ImportCategory, ImportPaymentMethod, ImportSummary, ImportTransaction, Storage,
};

fn item_key(kind: Kind, item_id: &str) -> String {
    match kind {
        Kind::Expense => format!("e:{item_id}"),
        Kind::Income => format!("i:{item_id}"),
    }
}

/// Pure mapping from parsed Paamonim data to an import batch.
pub fn build_batch(expense_items: &[BudgetItem], income_items: &[BudgetItem], history: &[HistoryEntry]) -> ImportBatch {
    let mut batch = ImportBatch::default();
    let mut by_name = std::collections::HashMap::new();
    for (items, kind) in [(expense_items, Kind::Expense), (income_items, Kind::Income)] {
        for it in items {
            let key = item_key(kind, &it.item_id);
            by_name.insert((kind, it.name.clone()), key.clone());
            batch.categories.push(ImportCategory { key, name: it.name.clone(), is_income: kind == Kind::Income });
        }
    }
    let mut seen_pm = std::collections::HashSet::new();
    for h in history {
        let payment_key = h.payment.as_ref().map(|(id, text)| {
            if seen_pm.insert(id.clone()) {
                batch.payment_methods.push(ImportPaymentMethod { key: id.clone(), name: text.clone() });
            }
            id.clone()
        });
        let sign = if h.kind == Kind::Income { 1 } else { -1 };
        batch.transactions.push(ImportTransaction {
            line_id: h.line_id.clone(),
            date: h.date.format("%Y-%m-%d").to_string(),
            amount_agorot: sign * from_shekels(h.amount_shekels),
            category_key: by_name.get(&(h.kind, h.item_name.clone())).cloned(),
            payment_key,
            note: h.note.clone(),
        });
    }
    batch
}

pub async fn run(client: &Client, storage: &dyn Storage, from: &str, to: &str) -> Result<ImportSummary> {
    let expense = crate::paamonim::parse_items(&client.fragment("/rishumit/get_expense_items", &[]).await?);
    let income = crate::paamonim::parse_items(&client.fragment("/rishumit/get_income_items", &[]).await?);
    let history = crate::paamonim::parse_history(&client.activity_history_html(from, to).await?);
    storage.import_paamonim(&build_batch(&expense, &income, &history)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn expenses_are_negative_agorot_and_linked_by_item_name() {
        let items = vec![BudgetItem { item_id: "5".into(), name: "סופר".into() }];
        let h = HistoryEntry {
            line_id: "1".into(),
            date: NaiveDate::from_ymd_opt(2026, 10, 3).unwrap(),
            item_name: "סופר".into(),
            amount_shekels: 123,
            kind: Kind::Expense,
            note: String::new(),
            label: String::new(),
            payment: Some(("9".into(), "visa".into())),
        };
        let b = build_batch(&items, &[], &[h]);
        assert_eq!(b.transactions[0].amount_agorot, -12300);
        assert_eq!(b.transactions[0].category_key.as_deref(), Some("e:5"));
        assert_eq!(b.payment_methods.len(), 1);
    }
}
