//! Standing rules: "always file mail with this subject under this category (and business)".
//! Kept as one JSON setting; there are few of them.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::storage::Storage;

const KEY: &str = "email_rules";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rule {
    /// The subject as it was when the rule was made, for showing.
    pub subject: String,
    pub category_id: i64,
    pub business_id: Option<i64>,
}

/// The subject without "Fwd:" / "Re:" prefixes, case and extra spaces, so a forwarded copy of a
/// mail matches the rule made from the original.
pub fn normalize(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let rest = ["fwd:", "fw:", "re:", "הועבר:", "תגובה:"]
            .iter()
            .find(|p| lower.starts_with(*p))
            .map(|p| s[p.len().min(s.len())..].trim_start());
        match rest {
            // `to_lowercase` can change byte lengths; stop rather than slice inside a character.
            Some(r) if s.is_char_boundary(s.len() - r.len()) => s = r,
            _ => break,
        }
    }
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub async fn list(db: &dyn Storage) -> Result<Vec<Rule>> {
    Ok(match db.get_setting(KEY).await? {
        Some(v) if !v.is_empty() => serde_json::from_str(&v).unwrap_or_default(),
        _ => Vec::new(),
    })
}

async fn save(db: &dyn Storage, rules: &[Rule]) -> Result<()> {
    db.set_setting(KEY, &serde_json::to_string(rules)?).await
}

/// Add a rule, replacing the one for the same subject.
pub async fn put(db: &dyn Storage, rule: Rule) -> Result<()> {
    let key = normalize(&rule.subject);
    let mut rules = list(db).await?;
    rules.retain(|r| normalize(&r.subject) != key);
    rules.push(rule);
    save(db, &rules).await
}

/// Remove the rule for this subject; false if there was none.
pub async fn remove(db: &dyn Storage, subject: &str) -> Result<bool> {
    let key = normalize(subject);
    let mut rules = list(db).await?;
    let before = rules.len();
    rules.retain(|r| normalize(&r.subject) != key);
    save(db, &rules).await?;
    Ok(rules.len() != before)
}

pub fn find<'a>(rules: &'a [Rule], subject: &str) -> Option<&'a Rule> {
    let key = normalize(subject);
    if key.is_empty() {
        return None;
    }
    rules.iter().find(|r| normalize(&r.subject) == key)
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn forwarded_copies_match() {
        assert_eq!(normalize("Fwd: Re:  Your  Receipt "), "your receipt");
        assert_eq!(normalize("FW: חשבונית מס"), "חשבונית מס");
        assert_eq!(normalize(""), "");
    }
}
