//! Initial (and repeatable) import from Paamonim into [`Storage`].

use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate};

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

/// Calendar-year windows covering `from..=to`, newest first. A single request over many
/// years is rejected by Paamonim with HTTP 500, so history is fetched a year at a time.
pub fn year_chunks(from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
    let mut out = Vec::new();
    for year in (from.year()..=to.year()).rev() {
        let start = NaiveDate::from_ymd_opt(year, 1, 1).unwrap().max(from);
        let end = NaiveDate::from_ymd_opt(year, 12, 31).unwrap().min(to);
        out.push((start, end));
    }
    out
}

/// Years with no lines in a row, after data was seen, that end the backwards walk.
const EMPTY_YEARS_TO_STOP: usize = 5;

async fn fetch_history(client: &Client, from: &str, to: &str) -> Result<Vec<HistoryEntry>> {
    let (from, to) = (NaiveDate::parse_from_str(from, "%Y-%m-%d")?, NaiveDate::parse_from_str(to, "%Y-%m-%d")?);
    let mut all = Vec::new();
    let mut empty_run = 0;
    for (start, end) in year_chunks(from, to) {
        let (s, e) = (start.format("%Y-%m-%d").to_string(), end.format("%Y-%m-%d").to_string());
        let html = client
            .activity_history_html(&s, &e)
            .await
            .with_context(|| format!("fetching Paamonim history {s}..{e}"))?;
        let lines = crate::paamonim::parse_history(&html);
        tracing::info!("paamonim history {s}..{e}: {} lines", lines.len());
        if lines.is_empty() {
            empty_run += 1;
            if !all.is_empty() && empty_run >= EMPTY_YEARS_TO_STOP {
                break;
            }
        } else {
            empty_run = 0;
            all.extend(lines);
        }
    }
    Ok(all)
}

pub async fn run(client: &Client, storage: &dyn Storage, from: &str, to: &str) -> Result<ImportSummary> {
    let expense = crate::paamonim::parse_items(&client.fragment("/rishumit/get_expense_items", &[]).await?);
    let income = crate::paamonim::parse_items(&client.fragment("/rishumit/get_income_items", &[]).await?);
    let history = fetch_history(client, from, to).await?;
    storage.import_paamonim(&build_batch(&expense, &income, &history)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_chunks_are_clipped_and_newest_first() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let chunks = year_chunks(d(2024, 6, 15), d(2026, 10, 30));
        assert_eq!(chunks, vec![(d(2026, 1, 1), d(2026, 10, 30)), (d(2025, 1, 1), d(2025, 12, 31)), (d(2024, 6, 15), d(2024, 12, 31))]);
    }

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
