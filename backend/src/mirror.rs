//! Mirror local transactions to Paamonim: new ones, edits and deletes. The app keeps exact
//! amounts; Paamonim only takes whole shekels, so amounts are rounded here, on export.
//!
//! Paamonim's save endpoint returns no line id, so after saving we re-read that day's history
//! and take the one new line that has the values we sent. Safeguards against corrupting or
//! duplicating data there:
//! - Mirroring only starts from when it was turned on (see `Storage::baseline_mirror`).
//! - A row is flagged `verifying` *before* a request is sent and only becomes `synced` once the
//!   new line has been found and checked. Anything unclear leaves it `verifying`/`error`, never
//!   retried blindly, and the run stops at the first problem.
//! - An edit posts the new line first and queues the old one for deletion afterwards; a
//!   delete is only reported done once the line is gone from the history.
//! - A line we posted but could not link is adopted by a later import (despite rounding) instead
//!   of being imported as a second transaction.
//! - `PAAMONIM_MIRROR_DRY_RUN=1` lists what would be sent and changes nothing.
//!
//! NOTE: field names come from the site's JS and forms; no write has been run against
//! a real account yet beyond one manual test line (saved and deleted by hand).

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use scraper::{Html, Selector};
use serde::Serialize;

use crate::money::to_paamonim_shekels;
use crate::paamonim::{self, Client, HistoryEntry, Kind};
use crate::storage::{PendingMirror, Storage};

/// Paamonim's remark field is limited to 35 characters.
const REMARK_MAX: usize = 35;

#[derive(Debug, Clone)]
pub struct Options {
    /// Log what would be sent; send and change nothing.
    pub dry_run: bool,
    /// Most requests that change Paamonim in one run.
    pub max_ops: usize,
    /// `(path, id parameter)` of the endpoint that deletes a history line (a POST). `None`
    /// leaves edits and deletes waiting.
    pub delete: Option<(String, String)>,
}

impl Options {
    /// `PAAMONIM_MIRROR_DRY_RUN`, `PAAMONIM_MIRROR_MAX_OPS` (default 50), and
    /// `PAAMONIM_DELETE_PATH` / `PAAMONIM_DELETE_PARAM` to override the delete endpoint
    /// (default: POST `/rishumit/delete_history_line` with `line_id`, as the site's own JS does).
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            dry_run: matches!(
                var("PAAMONIM_MIRROR_DRY_RUN").as_deref(),
                Some("1" | "true")
            ),
            max_ops: var("PAAMONIM_MIRROR_MAX_OPS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(50),
            delete: Some((
                var("PAAMONIM_DELETE_PATH")
                    .unwrap_or_else(|| "/rishumit/delete_history_line".into()),
                var("PAAMONIM_DELETE_PARAM").unwrap_or_else(|| "line_id".into()),
            )),
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct MirrorSummary {
    /// New lines posted.
    pub mirrored: usize,
    /// Edited transactions: new line posted, old one queued for deletion.
    pub replaced: usize,
    /// Edited, but Paamonim already holds the same values (e.g. rounding hides the change).
    pub unchanged: usize,
    /// Lines deleted in Paamonim.
    pub deleted: usize,
    /// Not mirrored by design: no Paamonim budget item, a zero amount, or the wrong sign.
    pub skipped: usize,
    /// Edits/deletes waiting because no delete endpoint is configured.
    pub unsupported: usize,
    /// Marked `error`; check them and `POST /api/mirror/paamonim/retry/:id`.
    pub failed: usize,
    /// Why the run ended early, if it did.
    pub stopped: Option<String>,
    /// Dry run only: what would have been done.
    pub planned: Vec<String>,
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

/// Payment method ids offered by the edit form's `payment_method_id` select.
pub fn parse_payment_methods(html: &str) -> HashSet<String> {
    let doc = Html::parse_fragment(html);
    let sel =
        Selector::parse("select[name=\"balance_history[payment_method_id]\"] option").unwrap();
    doc.select(&sel)
        .filter_map(|o| o.value().attr("value"))
        .filter(|v| !v.is_empty() && *v != "-1")
        .map(str::to_string)
        .collect()
}

/// Query fields for `save_item_line`, in the order the site's form serializes them.
/// The payment method is sent only if it is one the form offers: imported "payment methods"
/// come from installment groups in the history and are not real method ids.
pub fn save_fields(
    p: &PendingMirror,
    item_id: &str,
    family_id: &str,
    user_id: &str,
    methods: &HashSet<String>,
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
            p.payment_key
                .clone()
                .filter(|k| methods.contains(k))
                .unwrap_or_default(),
        ),
        ("balance_history[total_payments]", "1".to_string()),
        ("balance_history[family_id]", family_id.to_string()),
        ("balance_history[user_id]", user_id.to_string()),
        ("balance_history[budget_item_id]", item_id.to_string()),
    ])
}

async fn history(client: &Client, date: &str) -> Result<Vec<HistoryEntry>> {
    let html = client.activity_history_html(date, date).await?;
    let lines = paamonim::parse_history(&html);
    // Fail closed: if some lines weren't understood we can't tell what is new.
    if html.matches("line_id=").count() != lines.len() {
        bail!("Paamonim history for {date} has lines that could not be read");
    }
    Ok(lines)
}

fn item_id(p: &PendingMirror) -> Option<&str> {
    p.item_key
        .as_deref()
        .and_then(|k| k.strip_prefix("e:").or_else(|| k.strip_prefix("i:")))
}

fn kind_of(p: &PendingMirror) -> Option<Kind> {
    match p.item_key.as_deref()? {
        k if k.starts_with("e:") => Some(Kind::Expense),
        k if k.starts_with("i:") => Some(Kind::Income),
        _ => None,
    }
}

fn remark(p: &PendingMirror) -> String {
    p.note.chars().take(REMARK_MAX).collect()
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The line has the date, amount, direction and item we sent.
fn same_values(e: &HistoryEntry, p: &PendingMirror, shekels: i64) -> bool {
    e.date.format("%Y-%m-%d").to_string() == p.date
        && e.amount_shekels == shekels
        && Some(e.kind) == kind_of(p)
        && p.item_name.as_deref().is_none_or(|n| e.item_name == n)
}

/// Same, and the note too, as a single payment (the history doesn't show payment methods).
fn same_line(e: &HistoryEntry, p: &PendingMirror, shekels: i64) -> bool {
    same_values(e, p, shekels) && e.note == squash(&remark(p)) && e.payment.is_none()
}

/// The history entry of `line_id`, found by id (its edit form gives its day) so a wrong or
/// stale local date can't hide it. `None` if Paamonim no longer has it.
async fn find_line(client: &Client, line_id: &str) -> Result<Option<HistoryEntry>> {
    let Some(day) = client.line_date(line_id).await? else {
        return Ok(None);
    };
    let found = history(client, &day)
        .await?
        .into_iter()
        .find(|e| e.line_id == line_id);
    match found {
        Some(e) => Ok(Some(e)),
        None => bail!("line {line_id} has an edit form but is not in the history of {day}"),
    }
}

/// Rounded amount to send, or why the row can't be mirrored.
fn shekels_for(p: &PendingMirror) -> Option<i64> {
    let kind = kind_of(p)?;
    // An income item can't hold a negative amount, nor an expense item a refund.
    if (p.amount_agorot < 0) != (kind == Kind::Expense) {
        return None;
    }
    let s = to_paamonim_shekels(p.amount_agorot.abs());
    (s > 0).then_some(s)
}

enum Step {
    Continue,
    /// Stop the run: something needs a look before anything else is sent.
    Stop(String),
}

pub async fn run(client: &Client, storage: &dyn Storage, opts: &Options) -> Result<MirrorSummary> {
    let mut sum = MirrorSummary::default();
    let mut ops = 0usize;
    let pending = storage.pending_mirrors().await?;
    let verifying_per_sig = {
        let mut m: HashMap<String, usize> = HashMap::new();
        for p in pending.iter().filter(|p| p.state == "verifying") {
            *m.entry(sig(p)).or_default() += 1;
        }
        m
    };
    let mut claimed: HashMap<String, usize> = HashMap::new();

    for p in &pending {
        if ops >= opts.max_ops {
            sum.stopped = Some("operation limit reached; run again to continue".into());
            return Ok(sum);
        }
        let step = if p.state == "verifying" {
            let n = verifying_per_sig[&sig(p)];
            let rank = *claimed.entry(sig(p)).and_modify(|c| *c += 1).or_insert(1) - 1;
            resolve(client, storage, p, n, rank, &mut sum).await?
        } else {
            push(client, storage, opts, p, &mut ops, &mut sum).await?
        };
        if let Step::Stop(why) = step {
            sum.stopped = Some(why);
            return Ok(sum);
        }
    }

    for d in storage.pending_line_deletes().await? {
        if ops >= opts.max_ops {
            sum.stopped = Some("operation limit reached; run again to continue".into());
            return Ok(sum);
        }
        let Some((path, param)) = &opts.delete else {
            sum.unsupported += 1;
            continue;
        };
        let Some(line) = find_line(client, &d.line_id).await? else {
            if !opts.dry_run {
                storage.finish_line_delete(&d.line_id).await?;
            }
            continue;
        };
        if line.payment.is_some() {
            // One installment of a group: the site deletes the whole group at once.
            sum.unsupported += 1;
            continue;
        }
        if opts.dry_run {
            sum.planned
                .push(format!("delete line {} ({})", d.line_id, d.date));
            continue;
        }
        ops += 1;
        client.delete_item_line(path, param, &d.line_id).await?;
        if client.line_date(&d.line_id).await?.is_some() {
            sum.stopped = Some(format!(
                "line {} is still in Paamonim after deleting it",
                d.line_id
            ));
            return Ok(sum);
        }
        storage.finish_line_delete(&d.line_id).await?;
        sum.deleted += 1;
    }
    Ok(sum)
}

/// Rows with the same signature are interchangeable when matching leftovers.
fn sig(p: &PendingMirror) -> String {
    format!(
        "{}|{}|{:?}|{}",
        p.date,
        to_paamonim_shekels(p.amount_agorot.abs()),
        p.item_key,
        p.item_name.as_deref().unwrap_or("")
    )
}

/// Send a `pending` or `modified` row.
async fn push(
    client: &Client,
    storage: &dyn Storage,
    opts: &Options,
    p: &PendingMirror,
    ops: &mut usize,
    sum: &mut MirrorSummary,
) -> Result<Step> {
    let Some(item) = item_id(p) else {
        // No Paamonim budget item (a local category, or a parent). Nothing to send; an edited
        // line that used to have one is removed there.
        if p.line_id.is_some() && !opts.dry_run {
            storage.detach_mirror(p.id).await?;
        }
        sum.skipped += 1;
        return Ok(Step::Continue);
    };
    let Some(shekels) = shekels_for(p) else {
        if !opts.dry_run {
            if p.line_id.is_some() {
                storage.detach_mirror(p.id).await?;
            } else {
                storage.set_mirror_state(p.id, "local").await?;
            }
        }
        sum.skipped += 1;
        return Ok(Step::Continue);
    };

    if let Some(old) = &p.line_id {
        // Edit: the new line is posted, then the old one deleted.
        if opts.delete.is_none() {
            // The old line couldn't be removed afterwards, so don't post a second one.
            sum.unsupported += 1;
            return Ok(Step::Continue);
        }
        if let Some(e) = find_line(client, old).await? {
            if e.payment.is_some() {
                // An installment: replacing it would delete its whole group there.
                sum.unsupported += 1;
                return Ok(Step::Continue);
            }
            // Paamonim already holds these values (rounding can hide an edit).
            if same_line(&e, p, shekels) {
                if !opts.dry_run {
                    storage.mark_mirrored(p, old).await?;
                }
                sum.unchanged += 1;
                return Ok(Step::Continue);
            }
        }
    }

    if opts.dry_run {
        let what = if p.line_id.is_some() {
            "replace"
        } else {
            "create"
        };
        sum.planned.push(format!(
            "{what} #{}: {} ₪{shekels} item {item}",
            p.id, p.date
        ));
        return Ok(Step::Continue);
    }

    let form = client
        .fragment("/rishumit/edit_item_line", &[("id", item)])
        .await?;
    let Some((family, user)) = parse_account_ids(&form) else {
        bail!("account ids not found in edit form")
    };
    let methods = parse_payment_methods(&form);
    let Some(fields) = save_fields(p, item, &family, &user, &methods) else {
        return Ok(Step::Continue);
    };
    let before: HashSet<String> = history(client, &p.date)
        .await?
        .into_iter()
        .map(|e| e.line_id)
        .collect();

    // From here on the outcome may be unknown until checked: never re-send blindly.
    storage.set_mirror_state(p.id, "verifying").await?;
    *ops += 1;
    if let Err(e) = client.save_item_line(&fields).await {
        return Ok(Step::Stop(format!(
            "sending #{} failed ({e}); left as `verifying` and will be checked next run",
            p.id
        )));
    }
    let known: HashSet<String> = storage.known_line_ids().await?.into_iter().collect();
    let new: Vec<HistoryEntry> = history(client, &p.date)
        .await?
        .into_iter()
        .filter(|e| !before.contains(&e.line_id) && !known.contains(&e.line_id))
        .filter(|e| same_values(e, p, shekels))
        .collect();
    match new.as_slice() {
        [e] => {
            adopt(storage, p, e, shekels, sum).await?;
            if p.line_id.is_some() {
                sum.replaced += 1;
            } else {
                sum.mirrored += 1;
            }
            Ok(Step::Continue)
        }
        [] => {
            storage.set_mirror_state(p.id, "error").await?;
            sum.failed += 1;
            Ok(Step::Stop(format!(
                "#{} was sent but no matching new line appeared in Paamonim",
                p.id
            )))
        }
        _ => {
            storage.set_mirror_state(p.id, "error").await?;
            sum.failed += 1;
            Ok(Step::Stop(format!(
                "#{} was sent but several identical new lines appeared; check Paamonim",
                p.id
            )))
        }
    }
}

/// Link `p` to the found line `e`, checking the note and payment method too.
async fn adopt(
    storage: &dyn Storage,
    p: &PendingMirror,
    e: &HistoryEntry,
    shekels: i64,
    sum: &mut MirrorSummary,
) -> Result<()> {
    if !storage.mark_mirrored(p, &e.line_id).await? {
        // Deleted here while it was being sent.
        storage.queue_line_delete(&e.line_id, &p.date).await?;
    } else if !same_line(e, p, shekels) {
        storage.set_mirror_state(p.id, "error").await?;
        sum.failed += 1;
    }
    Ok(())
}

/// A row left `verifying` by an earlier run: look for the line it may have created.
async fn resolve(
    client: &Client,
    storage: &dyn Storage,
    p: &PendingMirror,
    same_sig_rows: usize,
    rank: usize,
    sum: &mut MirrorSummary,
) -> Result<Step> {
    let Some(shekels) = shekels_for(p) else {
        storage.set_mirror_state(p.id, "error").await?;
        sum.failed += 1;
        return Ok(Step::Continue);
    };
    let known: HashSet<String> = storage.known_line_ids().await?.into_iter().collect();
    let mut found: Vec<HistoryEntry> = history(client, &p.date)
        .await?
        .into_iter()
        .filter(|e| !known.contains(&e.line_id) && same_values(e, p, shekels))
        .collect();
    found.sort_by_key(|e| e.line_id.parse::<i64>().unwrap_or(i64::MAX));
    // Only trust a match when every identical candidate is accounted for by one of our rows;
    // otherwise one of them may be a line someone entered by hand.
    if found.len() == same_sig_rows {
        adopt(storage, p, &found[rank], shekels, sum).await?;
        sum.mirrored += 1;
    } else {
        storage.set_mirror_state(p.id, "error").await?;
        sum.failed += 1;
    }
    Ok(Step::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paamonim::HEBREW_MONTHS;
    use crate::storage::{
        ImportBatch, ImportCategory, ImportTransaction, NewTransaction, SqliteStorage,
    };
    use axum::{
        extract::{Query, State},
        http::StatusCode,
        routing::get,
        Router,
    };
    use chrono::NaiveDate;
    use std::sync::{Arc, Mutex};

    // ---- a pretend Paamonim -------------------------------------------------------------

    #[derive(Clone)]
    struct Line {
        id: i64,
        date: String,
        item: &'static str,
        income: bool,
        shekels: i64,
        note: String,
        /// Installment group root (`payment-<id>`), as the history shows it.
        group: Option<String>,
    }

    #[derive(Default)]
    struct Mock {
        lines: Vec<Line>,
        next_id: i64,
        saves: usize,
        deletes: usize,
        /// `payment_method_id` of the last save.
        last_method: String,
        /// Next save: answer 500 without saving.
        fail_before_save: bool,
        /// Next save: save, then answer 500.
        fail_after_save: bool,
        /// Next save: answer 200 without saving.
        silently_drop: bool,
        /// Next save: also add an identical line, as if someone else entered one.
        add_twin: bool,
    }

    type M = Arc<Mutex<Mock>>;

    const ITEMS: [(&str, &str, bool); 2] = [("5", "סופר", false), ("7", "משכורת", true)];

    async fn serve(mock: M) -> Client {
        async fn form(
            State(m): State<M>,
            Query(q): Query<HashMap<String, String>>,
        ) -> (StatusCode, String) {
            let ids = r#"<input name="balance_history[family_id]" value="1"><input name="balance_history[user_id]" value="2">"#;
            let Some(line) = q.get("line_id") else {
                let select = r#"<select name="balance_history[payment_method_id]"><option value=""></option><option value="1896">cash</option><option value="-1">new</option></select>"#;
                return (StatusCode::OK, format!("{ids}{select}"));
            };
            let m = m.lock().unwrap();
            match m.lines.iter().find(|l| l.id.to_string() == *line) {
                Some(l) => (
                    StatusCode::OK,
                    format!(
                        r#"{ids}<input type="date" value="{}" name="balance_history[payment_date]">"#,
                        l.date
                    ),
                ),
                None => (StatusCode::NOT_FOUND, String::new()),
            }
        }
        async fn save(State(m): State<M>, Query(q): Query<HashMap<String, String>>) -> StatusCode {
            let mut m = m.lock().unwrap();
            m.saves += 1;
            if std::mem::take(&mut m.fail_before_save) {
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            if std::mem::take(&mut m.silently_drop) {
                return StatusCode::OK;
            }
            m.last_method = q["balance_history[payment_method_id]"].clone();
            let item = ITEMS
                .iter()
                .find(|i| i.0 == q["balance_history[budget_item_id]"])
                .unwrap();
            let add = |m: &mut Mock| {
                m.next_id += 1;
                let id = 1000 + m.next_id;
                m.lines.push(Line {
                    id,
                    date: q["balance_history[payment_date]"].clone(),
                    item: item.1,
                    income: item.2,
                    shekels: q["balance_history[amount]"].parse().unwrap(),
                    note: q["balance_history[remark]"].clone(),
                    group: None,
                });
            };
            add(&mut m);
            if std::mem::take(&mut m.add_twin) {
                add(&mut m);
            }
            if std::mem::take(&mut m.fail_after_save) {
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            StatusCode::OK
        }
        async fn delete(
            State(m): State<M>,
            axum::extract::Form(q): axum::extract::Form<HashMap<String, String>>,
        ) -> StatusCode {
            let mut m = m.lock().unwrap();
            m.deletes += 1;
            let id: i64 = q["line_id"].parse().unwrap();
            m.lines.retain(|l| l.id != id);
            StatusCode::OK
        }
        async fn hist(State(m): State<M>, Query(q): Query<HashMap<String, String>>) -> String {
            let m = m.lock().unwrap();
            let day = &q["start_date"];
            let mut out = String::new();
            for l in m.lines.iter().filter(|l| &l.date == day) {
                if out.is_empty() {
                    let d = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
                    let month = HEBREW_MONTHS[chrono::Datelike::month0(&d) as usize];
                    out += &format!(
                        r#"<div class="date-row">שלישי, {} ב{} {}</div>"#,
                        chrono::Datelike::day(&d),
                        month,
                        chrono::Datelike::year(&d)
                    );
                }
                let pay = l.group.as_ref().map_or(String::new(), |p| {
                    format!(r#"<small class="pull-right payment-{p}">1 מתוך 2</small>"#)
                });
                out += &format!(
                    r#"<div class="historyLine" line_id="{}"><div>{} <span class="{}">{}</span></div><div><small>{}</small></div><div><small></small>{}</div></div>"#,
                    l.id,
                    l.item,
                    if l.income {
                        "budgetPlus"
                    } else {
                        "budgetMinus"
                    },
                    l.shekels,
                    l.note,
                    pay
                );
            }
            out
        }
        let app = Router::new()
            .route("/rishumit/edit_item_line", get(form))
            .route("/rishumit/save_item_line", get(save))
            .route("/rishumit/delete_history_line", axum::routing::post(delete))
            .route("/rishumit/activity_history", get(hist))
            .with_state(mock);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Client::for_tests(&base)
    }

    fn opts() -> Options {
        Options {
            dry_run: false,
            max_ops: 50,
            delete: Some(("/rishumit/delete_history_line".into(), "line_id".into())),
        }
    }

    /// Storage with Paamonim items "סופר" (expense) and "משכורת" (income) imported.
    async fn setup() -> (SqliteStorage, i64, i64) {
        let s = SqliteStorage::open(":memory:").unwrap();
        let cat = |key: &str, name: &str, income| ImportCategory {
            key: key.into(),
            name: name.into(),
            is_income: income,
            parent_key: None,
        };
        s.import_paamonim(&ImportBatch {
            categories: vec![cat("e:5", "סופר", false), cat("i:7", "משכורת", true)],
            ..Default::default()
        })
        .await
        .unwrap();
        let cats = s.list_categories().await.unwrap();
        let id = |n: &str| cats.iter().find(|c| c.name == n).unwrap().id;
        let ids = (id("סופר"), id("משכורת"));
        s.baseline_mirror().await.unwrap();
        (s, ids.0, ids.1)
    }

    fn tx(date: &str, agorot: i64, cat: i64) -> NewTransaction {
        NewTransaction {
            date: date.into(),
            amount_agorot: agorot,
            category_id: Some(cat),
            payment_method_id: None,
            note: "n".into(),
        }
    }

    async fn state_of(s: &SqliteStorage, id: i64) -> String {
        s.list_transactions()
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
            .sync_state
    }

    const D: &str = "2026-10-03";

    // ---- tests --------------------------------------------------------------------------

    #[tokio::test]
    async fn new_transaction_is_posted_rounded_and_keeps_its_exact_amount() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!((sum.mirrored, sum.failed), (1, 0));
        assert_eq!(mock.lock().unwrap().lines[0].shekels, 11);
        let t = &s.list_transactions().await.unwrap()[0];
        assert_eq!((t.amount_agorot, t.sync_state.as_str()), (-1050, "synced"));
        // Nothing left to do: a second run sends nothing.
        run(&c, &s, &opts()).await.unwrap();
        assert_eq!(mock.lock().unwrap().saves, 1);
        let _ = id;
    }

    #[tokio::test]
    async fn turning_mirroring_on_does_not_replay_earlier_entries() {
        let s = SqliteStorage::open(":memory:").unwrap();
        s.import_paamonim(&ImportBatch {
            categories: vec![ImportCategory {
                key: "e:5".into(),
                name: "סופר".into(),
                is_income: false,
                parent_key: None,
            }],
            ..Default::default()
        })
        .await
        .unwrap();
        let food = s.list_categories().await.unwrap()[0].id;
        let old = s.create_transaction(&tx(D, -1000, food)).await.unwrap();
        assert_eq!(s.baseline_mirror().await.unwrap(), 1);
        assert_eq!(s.baseline_mirror().await.unwrap(), 0);
        let mock = M::default();
        let c = serve(mock.clone()).await;
        run(&c, &s, &opts()).await.unwrap();
        assert_eq!(mock.lock().unwrap().saves, 0);
        assert_eq!(state_of(&s, old).await, "local");
        // New entries after that are mirrored.
        s.create_transaction(&tx(D, -2000, food)).await.unwrap();
        assert_eq!(run(&c, &s, &opts()).await.unwrap().mirrored, 1);
    }

    #[tokio::test]
    async fn edit_replaces_the_line_and_delete_removes_it() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        run(&c, &s, &opts()).await.unwrap();

        s.update_transaction(id, &tx("2026-10-04", -2000, food))
            .await
            .unwrap();
        assert_eq!(state_of(&s, id).await, "modified");
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!((sum.replaced, sum.deleted), (1, 1));
        {
            let m = mock.lock().unwrap();
            assert_eq!(m.lines.len(), 1);
            assert_eq!(
                (m.lines[0].date.as_str(), m.lines[0].shekels),
                ("2026-10-04", 20)
            );
        }
        assert_eq!(state_of(&s, id).await, "synced");

        s.delete_transaction(id).await.unwrap();
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!(sum.deleted, 1);
        assert!(mock.lock().unwrap().lines.is_empty());
        assert!(s.pending_line_deletes().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_edit_that_rounds_to_the_same_line_sends_nothing() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        run(&c, &s, &opts()).await.unwrap();
        s.update_transaction(id, &tx(D, -1100, food)).await.unwrap();
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!(sum.unchanged, 1);
        {
            let m = mock.lock().unwrap();
            assert_eq!((m.saves, m.deletes), (1, 0));
        }
        assert_eq!(state_of(&s, id).await, "synced");
    }

    #[tokio::test]
    async fn without_a_delete_endpoint_edits_and_deletes_wait_and_nothing_is_duplicated() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let no_delete = Options {
            delete: None,
            ..opts()
        };
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        run(&c, &s, &no_delete).await.unwrap();
        s.update_transaction(id, &tx(D, -3000, food)).await.unwrap();
        let sum = run(&c, &s, &no_delete).await.unwrap();
        assert_eq!(sum.unsupported, 1);
        assert_eq!(mock.lock().unwrap().saves, 1);
        assert_eq!(state_of(&s, id).await, "modified");
        // With the endpoint configured it goes through.
        assert_eq!(run(&c, &s, &opts()).await.unwrap().replaced, 1);
        assert_eq!(mock.lock().unwrap().lines.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_response_after_saving_does_not_post_twice() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        mock.lock().unwrap().fail_after_save = true;
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert!(sum.stopped.is_some());
        assert_eq!(state_of(&s, id).await, "verifying");
        // Next run finds the line it created and links it; no second post.
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!(sum.mirrored, 1);
        {
            let m = mock.lock().unwrap();
            assert_eq!((m.saves, m.lines.len()), (1, 1));
        }
        assert_eq!(state_of(&s, id).await, "synced");
    }

    #[tokio::test]
    async fn a_request_that_never_saved_is_flagged_not_retried() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        mock.lock().unwrap().fail_before_save = true;
        run(&c, &s, &opts()).await.unwrap();
        run(&c, &s, &opts()).await.unwrap();
        assert_eq!(state_of(&s, id).await, "error");
        assert_eq!(mock.lock().unwrap().saves, 1);
        // After a look, a manual retry puts it back in the queue.
        assert!(s.retry_mirror(id).await.unwrap());
        assert_eq!(run(&c, &s, &opts()).await.unwrap().mirrored, 1);
    }

    #[tokio::test]
    async fn an_ok_answer_without_a_new_line_is_an_error() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        mock.lock().unwrap().silently_drop = true;
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert!(sum.stopped.is_some());
        assert_eq!(state_of(&s, id).await, "error");
    }

    #[tokio::test]
    async fn lines_that_cannot_be_told_apart_are_not_guessed() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        mock.lock().unwrap().add_twin = true;
        run(&c, &s, &opts()).await.unwrap();
        assert_eq!(state_of(&s, id).await, "error");
        assert!(s.list_transactions().await.unwrap()[0].sync_state != "synced");
    }

    #[tokio::test]
    async fn a_line_entered_by_hand_the_same_day_is_left_alone() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        mock.lock().unwrap().lines.push(Line {
            id: 5,
            date: D.into(),
            item: "סופר",
            income: false,
            shekels: 11,
            note: String::new(),
            group: None,
        });
        s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        assert_eq!(run(&c, &s, &opts()).await.unwrap().mirrored, 1);
        let m = mock.lock().unwrap();
        assert_eq!(m.lines.len(), 2);
        assert_eq!(m.lines[0].id, 5);
    }

    #[tokio::test]
    async fn import_links_an_unconfirmed_line_instead_of_duplicating_it() {
        let (s, food, _) = setup().await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        s.set_mirror_state(id, "error").await.unwrap();
        // Paamonim holds the rounded amount of what we sent.
        let batch = ImportBatch {
            categories: vec![ImportCategory {
                key: "e:5".into(),
                name: "סופר".into(),
                is_income: false,
                parent_key: None,
            }],
            transactions: vec![ImportTransaction {
                line_id: "1001".into(),
                date: D.into(),
                amount_agorot: -1100,
                category_key: Some("e:5".into()),
                payment_key: None,
                note: "n".into(),
            }],
            ..Default::default()
        };
        let sum = s.import_paamonim(&batch).await.unwrap();
        assert_eq!((sum.transactions_adopted, sum.transactions_added), (1, 0));
        let all = s.list_transactions().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(
            (all[0].amount_agorot, all[0].sync_state.as_str()),
            (-1050, "synced")
        );
        // Importing again changes nothing.
        let sum = s.import_paamonim(&batch).await.unwrap();
        assert_eq!((sum.transactions_added, sum.transactions_adopted), (0, 0));
        assert_eq!(s.list_transactions().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn import_does_not_bring_back_a_line_waiting_to_be_deleted() {
        let (s, food, _) = setup().await;
        let id = s.create_transaction(&tx(D, -1000, food)).await.unwrap();
        mark_synced(&s, id, "1001").await;
        s.delete_transaction(id).await.unwrap();
        let batch = ImportBatch {
            transactions: vec![ImportTransaction {
                line_id: "1001".into(),
                date: D.into(),
                amount_agorot: -1000,
                category_key: Some("e:5".into()),
                payment_key: None,
                note: String::new(),
            }],
            ..Default::default()
        };
        s.import_paamonim(&batch).await.unwrap();
        assert!(s.list_transactions().await.unwrap().is_empty());
    }

    async fn mark_synced(s: &SqliteStorage, id: i64, line: &str) {
        let p = s
            .pending_mirrors()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap();
        s.mark_mirrored(&p, line).await.unwrap();
    }

    #[tokio::test]
    async fn dry_run_changes_nothing() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = s.create_transaction(&tx(D, -1050, food)).await.unwrap();
        let dry = Options {
            dry_run: true,
            ..opts()
        };
        let sum = run(&c, &s, &dry).await.unwrap();
        assert_eq!(sum.planned.len(), 1);
        assert_eq!(mock.lock().unwrap().saves, 0);
        assert_eq!(state_of(&s, id).await, "pending");
    }

    #[tokio::test]
    async fn rows_that_cannot_be_represented_are_kept_local() {
        let (s, food, pay) = setup().await;
        let tiny = s.create_transaction(&tx(D, -30, food)).await.unwrap();
        let refund = s.create_transaction(&tx(D, 5000, food)).await.unwrap();
        let zero = s.create_transaction(&tx(D, 0, food)).await.unwrap();
        let sum = run(&serve(M::default()).await, &s, &opts()).await.unwrap();
        assert_eq!((sum.skipped, sum.mirrored), (2, 1));
        // Under a shekel is sent as ₪1.
        assert_eq!(state_of(&s, tiny).await, "synced");
        assert_eq!(state_of(&s, refund).await, "local");
        assert_eq!(state_of(&s, zero).await, "local");
        let _ = pay;
    }

    #[tokio::test]
    async fn an_income_is_posted_to_its_income_item() {
        let (s, _, pay) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        s.create_transaction(&tx(D, 1234567, pay)).await.unwrap();
        run(&c, &s, &opts()).await.unwrap();
        let m = mock.lock().unwrap();
        assert!(m.lines[0].income);
        assert_eq!(m.lines[0].shekels, 12346);
    }

    /// An imported line 900 on day `D` in "סופר", also present in the mock.
    async fn imported(s: &SqliteStorage, mock: &M, group: Option<&str>) -> i64 {
        mock.lock().unwrap().lines.push(Line {
            id: 900,
            date: D.into(),
            item: "סופר",
            income: false,
            shekels: 10,
            note: String::new(),
            group: group.map(str::to_string),
        });
        s.import_paamonim(&ImportBatch {
            categories: vec![ImportCategory {
                key: "e:5".into(),
                name: "סופר".into(),
                is_income: false,
                parent_key: None,
            }],
            transactions: vec![ImportTransaction {
                line_id: "900".into(),
                date: D.into(),
                amount_agorot: -1000,
                category_key: Some("e:5".into()),
                payment_key: None,
                note: String::new(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();
        s.list_transactions().await.unwrap()[0].id
    }

    #[tokio::test]
    async fn editing_the_date_of_an_imported_line_replaces_it_on_its_original_day() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = imported(&s, &mock, None).await;
        s.update_transaction(id, &tx("2026-10-09", -1000, food))
            .await
            .unwrap();
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!((sum.replaced, sum.deleted), (1, 1));
        let m = mock.lock().unwrap();
        assert_eq!(m.lines.len(), 1);
        assert_eq!(m.lines[0].date, "2026-10-09");
        assert_ne!(m.lines[0].id, 900);
    }

    #[tokio::test]
    async fn a_delete_finds_the_line_even_if_the_queued_day_is_wrong() {
        let (s, _, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        imported(&s, &mock, None).await;
        s.queue_line_delete("900", "2020-01-01").await.unwrap();
        assert_eq!(run(&c, &s, &opts()).await.unwrap().deleted, 1);
        assert!(mock.lock().unwrap().lines.is_empty());
    }

    #[tokio::test]
    async fn installment_lines_are_never_deleted_or_replaced() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        let id = imported(&s, &mock, Some("1000")).await;
        s.update_transaction(id, &tx(D, -5000, food)).await.unwrap();
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!(sum.unsupported, 1);
        s.delete_transaction(id).await.unwrap();
        let sum = run(&c, &s, &opts()).await.unwrap();
        assert_eq!(sum.unsupported, 1);
        let m = mock.lock().unwrap();
        assert_eq!((m.lines.len(), m.saves, m.deletes), (1, 0, 0));
    }

    #[tokio::test]
    async fn only_payment_methods_the_site_offers_are_sent() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        s.import_paamonim(&ImportBatch {
            payment_methods: vec![
                crate::storage::ImportPaymentMethod {
                    key: "1896".into(),
                    name: "cash".into(),
                },
                crate::storage::ImportPaymentMethod {
                    key: "13000000".into(),
                    name: "1 מתוך 10".into(),
                },
            ],
            ..Default::default()
        })
        .await
        .unwrap();
        let pms = s.list_payment_methods().await.unwrap();
        for (name, sent) in [("cash", "1896"), ("1 מתוך 10", "")] {
            let pm = pms.iter().find(|p| p.name == name).unwrap().id;
            s.create_transaction(&NewTransaction {
                payment_method_id: Some(pm),
                ..tx(D, -1000, food)
            })
            .await
            .unwrap();
            assert_eq!(run(&c, &s, &opts()).await.unwrap().mirrored, 1);
            assert_eq!(mock.lock().unwrap().last_method, sent);
        }
    }

    #[tokio::test]
    async fn the_run_stops_at_the_operation_limit() {
        let (s, food, _) = setup().await;
        let mock = M::default();
        let c = serve(mock.clone()).await;
        for _ in 0..3 {
            s.create_transaction(&tx(D, -1000, food)).await.unwrap();
        }
        let limited = Options {
            max_ops: 2,
            ..opts()
        };
        let sum = run(&c, &s, &limited).await.unwrap();
        assert_eq!(sum.mirrored, 2);
        assert!(sum.stopped.is_some());
    }

    fn pending(amount: i64, note: &str) -> PendingMirror {
        PendingMirror {
            id: 1,
            date: "2026-10-03".into(),
            amount_agorot: amount,
            note: note.into(),
            item_key: Some("e:5".into()),
            payment_key: None,
            state: "pending".into(),
            item_name: Some("סופר".into()),
            line_id: None,
        }
    }

    #[test]
    fn rounds_to_whole_shekels_and_truncates_remark() {
        let f = save_fields(
            &pending(-1050, &"x".repeat(50)),
            "5",
            "F",
            "U",
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(f[1], ("balance_history[amount]", "11".to_string()));
        assert_eq!(f[3].1.chars().count(), 35);
        assert_eq!(f[4].1, "");
    }

    #[test]
    fn amounts_under_a_shekel_are_sent_as_one() {
        let f = save_fields(&pending(-30, ""), "5", "F", "U", &HashSet::new()).unwrap();
        assert_eq!(f[1], ("balance_history[amount]", "1".to_string()));
        assert!(save_fields(&pending(0, ""), "5", "F", "U", &HashSet::new()).is_none());
    }

    #[test]
    fn finds_account_ids() {
        let html = r#"<form id="saveItemForm"><input type="hidden" name="balance_history[family_id]" value="12"><input type="hidden" name="balance_history[user_id]" value="34"></form>"#;
        assert_eq!(parse_account_ids(html), Some(("12".into(), "34".into())));
    }
}
