//! A forwarded email, reduced to what the parser needs.

use anyhow::{anyhow, Result};
use mail_parser::MessageParser;
use std::hash::{Hash, Hasher};

/// Biggest piece of a body passed on to the AI (and kept while it waits for a retry).
const MAX_TEXT: usize = 12_000;
/// Biggest HTML body stored for viewing.
const MAX_HTML: usize = 300_000;

#[derive(Debug, Clone)]
pub struct Mail {
    /// The `Message-ID` header (without brackets); a fingerprint when there is none. Used to
    /// handle each message once.
    pub id: String,
    pub from: String,
    pub subject: String,
    /// Day it was sent (YYYY-MM-DD).
    pub received: String,
    /// The body as HTML (plain-text mail is turned into HTML), cut to a size worth storing.
    pub html: String,
    /// Plain text of the body (HTML is turned into text), cut to a size the AI call can take.
    pub text: String,
}

/// Does the header block carry `X-Forwarded-For` mentioning `address`? Gmail adds that header
/// when it forwards mail automatically. It is not authenticated (anyone can write it), so it only
/// raises the bar for random mail sent to the routed address.
pub fn forwarded_by(raw: &[u8], address: &str) -> bool {
    let head = &raw[..raw.len().min(65_536)];
    let head = String::from_utf8_lossy(head).to_lowercase();
    let head = head
        .split("\r\n\r\n")
        .next()
        .unwrap_or("")
        .split("\n\n")
        .next()
        .unwrap_or("");
    head.lines()
        .any(|l| l.starts_with("x-forwarded-for:") && l.contains(&address.to_lowercase()))
}

pub fn parse_raw(raw: &[u8]) -> Result<Mail> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| anyhow!("not an email message"))?;
    let from = msg
        .from()
        .and_then(|a| a.first())
        .map(|a| match (a.name(), a.address()) {
            (Some(n), Some(e)) => format!("{n} <{e}>"),
            (None, Some(e)) => e.to_string(),
            (Some(n), None) => n.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default();
    let subject = msg.subject().unwrap_or("").to_string();
    let received = msg
        .date()
        .map(|d| format!("{:04}-{:02}-{:02}", d.year, d.month, d.day))
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let text = collapse(&msg.body_text(0).unwrap_or_default());
    let text: String = text.chars().take(MAX_TEXT).collect();
    let html: String = msg
        .body_html(0)
        .unwrap_or_default()
        .chars()
        .take(MAX_HTML)
        .collect();
    let id = match msg.message_id() {
        Some(id) if !id.trim().is_empty() => id
            .trim()
            .trim_matches(['<', '>'])
            .chars()
            .take(120)
            .collect(),
        _ => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (&from, &subject, &received, &text).hash(&mut h);
            format!("noid-{:016x}", h.finish())
        }
    };
    Ok(Mail {
        id,
        from,
        subject,
        received,
        html,
        text,
    })
}

/// Runs of blanks become one space, runs of blank lines one line break.
fn collapse(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for l in s.lines() {
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() && lines.last().is_none_or(|p| p.is_empty()) {
            continue;
        }
        lines.push(l);
    }
    lines.join("\n").trim().to_string()
}
