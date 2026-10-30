//! Initial (and repeatable) import from Paamonim into [`Storage`].

use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate};

use crate::money::from_shekels;
use crate::paamonim::{
    parse_categories, parse_custom_items, split_category_prefix, BudgetItem, Category as PCategory, Client, CustomItem,
    HistoryEntry, Kind,
};
use crate::storage::{
    ImportBatch, ImportCategory, ImportPaymentMethod, ImportSummary, ImportTransaction, Storage,
};

fn item_key(kind: Kind, item_id: &str) -> String {
    match kind {
        Kind::Expense => format!("e:{item_id}"),
        Kind::Income => format!("i:{item_id}"),
    }
}

/// Item lists and the category taxonomy as read from Paamonim.
pub struct Taxonomy<'a> {
    pub expense_items: &'a [BudgetItem],
    pub income_items: &'a [BudgetItem],
    pub categories: &'a [PCategory],
    pub custom_expense: &'a [CustomItem],
    pub custom_income: &'a [CustomItem],
}

/// Pure mapping from parsed Paamonim data to an import batch.
///
/// Paamonim has no real item-to-category tree: standard items are flat, a few are named
/// `<category> - <item>`, and only custom items carry a category id. Items with a known
/// category become children of it; the rest stay top-level.
pub fn build_batch(t: &Taxonomy, history: &[HistoryEntry]) -> ImportBatch {
    let mut batch = ImportBatch::default();
    let mut by_name = std::collections::HashMap::new();
    let mut parents = std::collections::HashSet::new();

    for kind in [Kind::Expense, Kind::Income] {
        let (items, custom) = match kind {
            Kind::Expense => (t.expense_items, t.custom_expense),
            Kind::Income => (t.income_items, t.custom_income),
        };
        let is_income = kind == Kind::Income;
        let tag = if is_income { "i" } else { "e" };
        let mut ensure_parent = |batch: &mut ImportBatch, c: &PCategory| -> String {
            let key = format!("c{tag}:{}", c.id);
            if parents.insert(key.clone()) {
                batch.categories.push(ImportCategory { key: key.clone(), name: c.name.clone(), is_income, parent_key: None });
            }
            key
        };

        for it in items {
            let (parent_key, name) = match split_category_prefix(&it.name, t.categories) {
                Some((c, rest)) => (Some(ensure_parent(&mut batch, c)), rest.to_string()),
                None => (None, it.name.clone()),
            };
            let key = item_key(kind, &it.item_id);
            by_name.insert((kind, it.name.clone()), key.clone());
            batch.categories.push(ImportCategory { key, name, is_income, parent_key });
        }
        for it in custom {
            let parent_key = it
                .category_id
                .as_ref()
                .and_then(|id| t.categories.iter().find(|c| &c.id == id))
                .map(|c| ensure_parent(&mut batch, c));
            let key = item_key(kind, &it.item_id);
            by_name.insert((kind, it.name.clone()), key.clone());
            batch.categories.push(ImportCategory { key, name: it.name.clone(), is_income, parent_key });
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
    let custom_expense_html = client.fragment("/rishumit/get_custom_items", &[("item_type", "expense")]).await?;
    let custom_income_html = client.fragment("/rishumit/get_custom_items", &[("item_type", "income")]).await?;
    let categories = parse_categories(&custom_expense_html);
    let (custom_expense, custom_income) = (parse_custom_items(&custom_expense_html), parse_custom_items(&custom_income_html));
    let history = fetch_history(client, from, to).await?;
    let taxonomy = Taxonomy {
        expense_items: &expense,
        income_items: &income,
        categories: &categories,
        custom_expense: &custom_expense,
        custom_income: &custom_income,
    };
    storage.import_paamonim(&build_batch(&taxonomy, &history)).await
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

    fn item(id: &str, name: &str) -> BudgetItem {
        BudgetItem { item_id: id.into(), name: name.into() }
    }

    fn entry(name: &str, kind: Kind) -> HistoryEntry {
        HistoryEntry {
            line_id: "1".into(),
            date: NaiveDate::from_ymd_opt(2026, 10, 3).unwrap(),
            item_name: name.into(),
            amount_shekels: 123,
            kind,
            note: String::new(),
            label: String::new(),
            payment: Some(("9".into(), "visa".into())),
        }
    }

    #[test]
    fn expenses_are_negative_agorot_and_linked_by_item_name() {
        let items = [item("5", "סופר")];
        let t = Taxonomy { expense_items: &items, income_items: &[], categories: &[], custom_expense: &[], custom_income: &[] };
        let b = build_batch(&t, &[entry("סופר", Kind::Expense)]);
        assert_eq!(b.transactions[0].amount_agorot, -12300);
        assert_eq!(b.transactions[0].category_key.as_deref(), Some("e:5"));
        assert_eq!(b.payment_methods.len(), 1);
    }

    #[test]
    fn prefixed_and_custom_items_get_parents_and_others_stay_top_level() {
        let cats = [PCategory { id: "3".into(), name: "דיור".into() }];
        let items = [item("1", "דיור - שכירות"), item("2", "סופר")];
        let custom = [CustomItem { item_id: "90".into(), name: "ארנונה".into(), category_id: Some("3".into()) }];
        let t = Taxonomy { expense_items: &items, income_items: &[], categories: &cats, custom_expense: &custom, custom_income: &[] };
        let b = build_batch(&t, &[entry("ארנונה", Kind::Expense), entry("דיור - שכירות", Kind::Expense)]);
        let get = |k: &str| b.categories.iter().find(|c| c.key == k).unwrap();
        assert_eq!(get("e:1").name, "שכירות");
        assert_eq!(get("e:1").parent_key.as_deref(), Some("ce:3"));
        assert_eq!(get("e:90").parent_key.as_deref(), Some("ce:3"));
        assert_eq!(get("e:2").parent_key, None);
        assert_eq!(b.categories.iter().filter(|c| c.key == "ce:3").count(), 1);
        // a parent is always listed before its children
        let pos = |k: &str| b.categories.iter().position(|c| c.key == k).unwrap();
        assert!(pos("ce:3") < pos("e:1"));
        assert_eq!(b.transactions[0].category_key.as_deref(), Some("e:90"));
        assert_eq!(b.transactions[1].category_key.as_deref(), Some("e:1"));
    }
}
