//! Mirror locally created transactions to Paamonim. The app keeps exact amounts;
//! Paamonim only takes whole shekels, so amounts are rounded here, on export.
//!
//! The save endpoint returns no line id, so after saving we re-read that day's
//! history and take the one line that wasn't there before.
//!
//! NOTE: field names come from the site's JS and forms; no write has been run against
//! a real account yet.

use std::collections::HashSet;

use anyhow::{bail, Result};
use scraper::{Html, Selector};
use serde::Serialize;

use crate::money::to_paamonim_shekels;
use crate::paamonim::{self, Client};
use crate::storage::{PendingMirror, Storage};

/// Paamonim's remark field is limited to 35 characters.
const REMARK_MAX: usize = 35;

#[derive(Debug, Default, Serialize)]
pub struct MirrorSummary {
    pub mirrored: usize,
    /// No Paamonim budget item (e.g. a locally created category) or rounds to zero.
    pub skipped: usize,
    pub failed: usize,
}

/// Account constants from the hidden inputs of the edit form.
pub fn parse_account_ids(html: &str) -> Option<(String, String)> {
    let doc = Html::parse_fragment(html);
    let get = |name: &str| {
        let sel = Selector::parse(&format!("input[name=\"balance_history[{name}]\"]")).unwrap();
        doc.select(&sel)
            .next()
            .and_then(|e| e.value().attr("value"))
            .map(str::to_string)
    };
    Some((get("family_id")?, get("user_id")?))
}

/// Query fields for `save_item_line`, in the order the site's form serializes them.
pub fn save_fields(
    p: &PendingMirror,
    item_id: &str,
    family_id: &str,
    user_id: &str,
) -> Option<Vec<(&'static str, String)>> {
    let shekels = to_paamonim_shekels(p.amount_agorot.abs());
    if shekels == 0 {
        return None;
    }
    let remark: String = p.note.chars().take(REMARK_MAX).collect();
    Some(vec![
        ("utf8", "\u{2713}".to_string()),
        ("balance_history[amount]", shekels.to_string()),
        ("balance_history[payment_date]", p.date.clone()),
        ("balance_history[remark]", remark),
        (
            "balance_history[payment_method_id]",
            p.payment_key.clone().unwrap_or_default(),
        ),
        ("balance_history[total_payments]", "1".to_string()),
        ("balance_history[family_id]", family_id.to_string()),
        ("balance_history[user_id]", user_id.to_string()),
        ("balance_history[budget_item_id]", item_id.to_string()),
    ])
}

async fn history_ids(client: &Client, date: &str) -> Result<HashSet<String>> {
    let html = client.activity_history_html(date, date).await?;
    Ok(paamonim::parse_history(&html)
        .into_iter()
        .map(|h| h.line_id)
        .collect())
}

pub async fn run(client: &Client, storage: &dyn Storage) -> Result<MirrorSummary> {
    let mut sum = MirrorSummary::default();
    for p in storage.pending_mirrors().await? {
        // Only Paamonim budget items (`e:<id>` / `i:<id>`) can be written; parent categories can't.
        let Some(item_id) = p
            .item_key
            .as_deref()
            .and_then(|k| k.strip_prefix("e:").or_else(|| k.strip_prefix("i:")))
        else {
            sum.skipped += 1;
            continue;
        };
        let form = client
            .fragment("/rishumit/edit_item_line", &[("id", item_id)])
            .await?;
        let Some((family, user)) = parse_account_ids(&form) else {
            bail!("account ids not found in edit form")
        };
        let Some(fields) = save_fields(&p, item_id, &family, &user) else {
            storage.mark_mirror_error(p.id).await?;
            sum.skipped += 1;
            continue;
        };
        let before = history_ids(client, &p.date).await?;
        if client.save_item_line(&fields).await.is_err() {
            storage.mark_mirror_error(p.id).await?;
            sum.failed += 1;
            continue;
        }
        let known: HashSet<String> = storage.known_line_ids().await?.into_iter().collect();
        let new: Vec<String> = history_ids(client, &p.date)
            .await?
            .into_iter()
            .filter(|id| !before.contains(id) && !known.contains(id))
            .collect();
        match new.as_slice() {
            [line_id] => {
                storage.mark_mirrored(p.id, line_id).await?;
                sum.mirrored += 1;
            }
            // Saved but the new line can't be told apart: stop retries so it isn't posted twice.
            _ => {
                storage.mark_mirror_error(p.id).await?;
                sum.failed += 1;
            }
        }
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(amount: i64, note: &str) -> PendingMirror {
        PendingMirror {
            id: 1,
            date: "2026-10-03".into(),
            amount_agorot: amount,
            note: note.into(),
            item_key: Some("e:5".into()),
            payment_key: None,
        }
    }

    #[test]
    fn rounds_to_whole_shekels_and_truncates_remark() {
        let f = save_fields(&pending(-1050, &"x".repeat(50)), "5", "F", "U").unwrap();
        assert_eq!(f[1], ("balance_history[amount]", "11".to_string()));
        assert_eq!(f[3].1.chars().count(), 35);
        assert_eq!(f[4].1, "");
    }

    #[test]
    fn amounts_rounding_to_zero_are_not_sent() {
        assert!(save_fields(&pending(30, ""), "5", "F", "U").is_none());
    }

    #[test]
    fn finds_account_ids() {
        let html = r#"<form id="saveItemForm"><input type="hidden" name="balance_history[family_id]" value="12"><input type="hidden" name="balance_history[user_id]" value="34"></form>"#;
        assert_eq!(parse_account_ids(html), Some(("12".into(), "34".into())));
    }
}
