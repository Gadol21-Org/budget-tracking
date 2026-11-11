//! A forwarded email, reduced to what the parser needs.

use anyhow::{anyhow, Result};
use mail_parser::{MessageParser, MimeHeaders};
use std::hash::{Hash, Hasher};

/// Biggest piece of a body passed on to the AI (and kept while it waits for a retry).
const MAX_TEXT: usize = 12_000;
/// Biggest HTML body stored for viewing.
const MAX_HTML: usize = 300_000;
/// PDF attachments handed to the AI (the first ones; any more count as unread).
const MAX_PDFS: usize = 3;

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
    /// PDF attachments (at most [`MAX_PDFS`]); the AI reads them with the body.
    pub pdfs: Vec<Vec<u8>>,
    /// Attachments the AI does not read (other file types, PDFs beyond the limit), by name or type.
    pub unread: Vec<String>,
    /// The raw message, kept in case it has to wait for a retry.
    pub raw: std::sync::Arc<[u8]>,
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
    let (mut pdfs, mut unread) = (Vec::new(), Vec::new());
    for part in msg.attachments() {
        let kind = part.content_type().map_or_else(String::new, |c| {
            format!("{}/{}", c.ctype(), c.subtype().unwrap_or_default()).to_lowercase()
        });
        let name = part.attachment_name().unwrap_or_default();
        if kind == "application/pdf" || name.to_lowercase().ends_with(".pdf") {
            if pdfs.len() < MAX_PDFS && !part.contents().is_empty() {
                pdfs.push(part.contents().to_vec());
            } else {
                unread.push(if name.is_empty() {
                    kind
                } else {
                    name.to_string()
                });
            }
        } else if part.content_id().is_some() && kind.starts_with("image/") {
            // A picture shown inside the message (a logo), not a document.
        } else {
            unread.push(if name.is_empty() {
                kind
            } else {
                name.to_string()
            });
        }
    }
    Ok(Mail {
        id,
        from,
        subject,
        received,
        html,
        text,
        pdfs,
        unread,
        raw: raw.into(),
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

#[allow(dead_code)] // for handlers
impl Mail {
    /// The `http(s)` links in the HTML body, in order, without repeats (for handlers that follow them).
    pub fn links(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for part in self.html.split("href=").skip(1) {
            let Some(q) = part.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let url = part[1..]
                .split(q)
                .next()
                .unwrap_or("")
                .replace("&amp;", "&");
            if (url.starts_with("https://") || url.starts_with("http://")) && !out.contains(&url) {
                out.push(url);
            }
        }
        out
    }
}
