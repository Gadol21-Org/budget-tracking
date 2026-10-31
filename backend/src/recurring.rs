//! Repeating monthly transactions.
//!
//! A series is stored as one or more *segments*, each a rule ("from `start_month` to
//! `end_month`, this amount on this day"). Occurrences are generated from the rules when
//! read and are not stored as transactions, so changing a series "from this month on" only
//! rewrites the rules: the segment in force is cut off the month before and a new one
//! starts, and earlier months keep their old values.
//!
//! An occurrence is *unsettled* until its date has passed. Unsettled ones exist only as
//! generated previews (shown as upcoming, counted in totals) and are the only ones a series
//! change may touch. When the date passes, [`settle_due`] turns the occurrence into an
//! ordinary transaction (sync state `pending`: mirrored to Paamonim like any other), which flows into the
//! totals, the Google Sheets export and so on like any other and is never rewritten by a
//! later change to the series.

use std::collections::BTreeMap;

use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::oplog::{self, kind};
use crate::storage::{MonthlyTotal, NewTransaction, Storage};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RecurringSegment {
    pub id: i64,
    /// Id of the series' first segment; every later segment of the series repeats it.
    pub series_id: i64,
    pub start_month: String, // YYYY-MM
    /// Last month, inclusive; none means it repeats forever.
    pub end_month: Option<String>,
    /// Day of month; months that are shorter use their last day.
    pub day: u32,
    /// Signed like transactions: expenses are negative.
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub note: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct NewRecurring {
    /// When creating: the first month. When changing a series: the month the change starts.
    pub start_month: String,
    pub end_month: Option<String>,
    #[serde(default = "first_day")]
    pub day: u32,
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    #[serde(default)]
    pub note: String,
}

fn first_day() -> u32 {
    1
}

impl RecurringSegment {
    pub(crate) fn to_new(&self) -> NewRecurring {
        NewRecurring {
            start_month: self.start_month.clone(),
            end_month: self.end_month.clone(),
            day: self.day,
            amount_agorot: self.amount_agorot,
            category_id: self.category_id,
            payment_method_id: self.payment_method_id,
            note: self.note.clone(),
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub series_id: i64,
    pub segment_id: i64,
    pub date: String, // YYYY-MM-DD
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub note: String,
}

#[derive(Debug)]
pub enum RecurringError {
    /// The request does not make sense (HTTP 400).
    Invalid(String),
    Storage(anyhow::Error),
}

impl From<anyhow::Error> for RecurringError {
    fn from(e: anyhow::Error) -> Self {
        Self::Storage(e)
    }
}

fn invalid<T>(msg: &str) -> Result<T, RecurringError> {
    Err(RecurringError::Invalid(msg.into()))
}

fn parse_month(m: &str) -> Option<NaiveDate> {
    if m.len() != 7 {
        return None;
    }
    NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").ok()
}

fn fmt_month(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

pub fn is_month(m: &str) -> bool {
    parse_month(m).is_some()
}

/// The month before `m` (`m` must be a valid YYYY-MM).
fn prev_month(m: &str) -> String {
    fmt_month(parse_month(m).unwrap() - Months::new(1))
}

/// The day of `month` (its first day) the occurrence falls on; short months use their last day.
fn occurrence_date(month: NaiveDate, day: u32) -> String {
    let last = (month + Months::new(1)).pred_opt().unwrap().day();
    format!("{}-{:02}", fmt_month(month), day.clamp(1, last))
}

/// Every occurrence of the segments whose month is `<= through_month`, oldest first.
pub fn occurrences(segments: &[RecurringSegment], through_month: &str) -> Vec<Occurrence> {
    let mut out = Vec::new();
    for s in segments {
        let (Some(start), Some(through)) =
            (parse_month(&s.start_month), parse_month(through_month))
        else {
            continue;
        };
        let end = match s.end_month.as_deref().and_then(parse_month) {
            Some(e) => e.min(through),
            None => through,
        };
        let mut month = start;
        while month <= end {
            out.push(Occurrence {
                series_id: s.series_id,
                segment_id: s.id,
                date: occurrence_date(month, s.day),
                amount_agorot: s.amount_agorot,
                category_id: s.category_id,
                payment_method_id: s.payment_method_id,
                note: s.note.clone(),
            });
            month = month + Months::new(1);
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then(a.segment_id.cmp(&b.segment_id)));
    out
}

/// How far ahead occurrences are generated unless asked otherwise (series that never end
/// need some horizon).
pub const DEFAULT_HORIZON_MONTHS: u32 = 24;

pub fn default_through(today: NaiveDate) -> String {
    fmt_month(today + Months::new(DEFAULT_HORIZON_MONTHS))
}

/// Occurrences still ahead of `today` (the unsettled ones), through `through_month`.
/// `settled` are months already turned into transactions (e.g. edited on their own).
pub fn upcoming(
    segments: &[RecurringSegment],
    through_month: &str,
    today: NaiveDate,
    settled: &[(i64, String)],
) -> Vec<Occurrence> {
    let today = today.format("%Y-%m-%d").to_string();
    let mut occ = occurrences(segments, through_month);
    occ.retain(|o| o.date > today && !settled.contains(&(o.series_id, o.date[..7].to_string())));
    occ
}

/// [`upcoming`] over what is stored.
pub async fn upcoming_stored(
    db: &dyn Storage,
    through_month: &str,
    today: NaiveDate,
) -> anyhow::Result<Vec<Occurrence>> {
    let segments = db.list_recurring().await?;
    let settled = db.settled_recurring().await?;
    Ok(upcoming(&segments, through_month, today, &settled))
}

/// Edit a single month on its own: it becomes a transaction with these values (any date,
/// e.g. a salary that arrived early) and the series carries on unchanged around it. Deleting
/// that transaction afterwards skips the month.
pub async fn settle_month(
    db: &dyn Storage,
    series_id: i64,
    month: &str,
    t: &NewTransaction,
) -> Result<(), RecurringError> {
    if NaiveDate::parse_from_str(&t.date, "%Y-%m-%d").is_err() {
        return invalid("date must be YYYY-MM-DD");
    }
    if t.amount_agorot == 0 {
        return invalid("amount must not be zero");
    }
    settle_or_skip(db, series_id, month, Some(t)).await
}

/// Skip a single month: no transaction is made and the series carries on around it.
pub async fn skip_month(
    db: &dyn Storage,
    series_id: i64,
    month: &str,
) -> Result<(), RecurringError> {
    settle_or_skip(db, series_id, month, None).await
}

async fn settle_or_skip(
    db: &dyn Storage,
    series_id: i64,
    month: &str,
    t: Option<&NewTransaction>,
) -> Result<(), RecurringError> {
    if !is_month(month) {
        return invalid("month must be YYYY-MM");
    }
    let segs = series_segments(db, series_id).await?;
    if occurrences(&segs, month)
        .iter()
        .all(|o| &o.date[..7] != month)
    {
        return invalid("the series has no occurrence in that month");
    }
    if !db.settle_recurring_month(series_id, month, t).await? {
        return invalid("that month has already settled");
    }
    Ok(())
}

/// Whether a series change may start in `month`: its occurrence must still be ahead.
fn is_unsettled(month: &str, day: u32, today: NaiveDate) -> bool {
    occurrence_date(parse_month(month).unwrap(), day) > today.format("%Y-%m-%d").to_string()
}

/// Settle (see the module docs) every occurrence whose date is on or before `today`.
/// Cheap to call often: it does nothing unless the day or the rules changed since last time.
pub async fn settle_due(db: &dyn Storage, today: NaiveDate) -> anyhow::Result<usize> {
    static LAST: tokio::sync::Mutex<Option<(NaiveDate, Vec<RecurringSegment>)>> =
        tokio::sync::Mutex::const_new(None);
    let mut last = LAST.lock().await;
    let segments = db.list_recurring().await?;
    if last
        .as_ref()
        .is_some_and(|(d, s)| *d == today && *s == segments)
    {
        return Ok(0);
    }
    let today_str = today.format("%Y-%m-%d").to_string();
    let mut due = occurrences(&segments, &fmt_month(today));
    due.retain(|o| o.date <= today_str);
    let added = db.settle_recurring(&due).await?;
    *last = Some((today, segments));
    Ok(added)
}

/// Add occurrences to monthly totals computed from real transactions.
pub fn add_to_totals(totals: &mut Vec<MonthlyTotal>, occ: &[Occurrence]) {
    let mut by_key: BTreeMap<(String, Option<i64>), (i64, i64)> = totals
        .drain(..)
        .map(|t| ((t.month, t.category_id), (t.total_agorot, t.count)))
        .collect();
    for o in occ {
        let e = by_key
            .entry((o.date[..7].to_string(), o.category_id))
            .or_default();
        e.0 += o.amount_agorot;
        e.1 += 1;
    }
    totals.extend(
        by_key.into_iter().map(
            |((month, category_id), (total_agorot, count))| MonthlyTotal {
                month,
                category_id,
                total_agorot,
                count,
            },
        ),
    );
    totals.sort_by(|a, b| {
        b.month
            .cmp(&a.month)
            .then(a.category_id.cmp(&b.category_id))
    });
}

fn validate(r: &NewRecurring) -> Result<(), RecurringError> {
    if !is_month(&r.start_month) {
        return invalid("start_month must be YYYY-MM");
    }
    if let Some(end) = &r.end_month {
        if !is_month(end) {
            return invalid("end_month must be YYYY-MM");
        }
        if *end < r.start_month {
            return invalid("end_month is before start_month");
        }
    }
    if !(1..=31).contains(&r.day) {
        return invalid("day must be 1-31");
    }
    if r.amount_agorot == 0 {
        return invalid("amount must not be zero");
    }
    Ok(())
}

/// Start a new series; returns its id.
pub async fn create(db: &dyn Storage, r: &NewRecurring) -> Result<i64, RecurringError> {
    validate(r)?;
    let id = db.insert_recurring(None, r).await?;
    oplog::record(
        db,
        kind::RECURRING_CREATE,
        Some(id),
        None,
        oplog::rule_value(db, r).await,
    )
    .await;
    Ok(id)
}

async fn series_segments(
    db: &dyn Storage,
    series_id: i64,
) -> Result<Vec<RecurringSegment>, RecurringError> {
    let mut segs: Vec<_> = db
        .list_recurring()
        .await?
        .into_iter()
        .filter(|s| s.series_id == series_id)
        .collect();
    if segs.is_empty() {
        return invalid("no such recurring series");
    }
    segs.sort_by(|a, b| a.start_month.cmp(&b.start_month));
    Ok(segs)
}

/// Apply `r` from `r.start_month` onward. Months before it keep their current values; any
/// later segments of the series are replaced by the new rule (which carries its own end).
pub async fn change_from(
    db: &dyn Storage,
    series_id: i64,
    r: &NewRecurring,
    today: NaiveDate,
) -> Result<(), RecurringError> {
    validate(r)?;
    let segs = series_segments(db, series_id).await?;
    let from = r.start_month.as_str();
    let Some(current) = segs.iter().rev().find(|s| s.start_month.as_str() <= from) else {
        return invalid("the change starts before the series does");
    };
    if current.end_month.as_deref().is_some_and(|e| e < from) {
        return invalid("the series has already ended in that month");
    }
    if !is_unsettled(from, current.day, today) {
        return invalid("that month has already settled; edit its transaction instead");
    }
    let old = oplog::segment_value(db, current).await;
    if current.start_month == from {
        db.update_recurring(current.id, r).await?;
    } else {
        // Insert first, so a failure half way leaves the old rule intact.
        db.insert_recurring(Some(series_id), r).await?;
        let mut cut = current.to_new();
        cut.end_month = Some(prev_month(from));
        db.update_recurring(current.id, &cut).await?;
    }
    for later in segs.iter().filter(|s| s.start_month.as_str() > from) {
        db.delete_recurring(later.id).await?;
    }
    let new = oplog::rule_value(db, r).await;
    oplog::record(db, kind::RECURRING_UPDATE, Some(series_id), old, new).await;
    Ok(())
}

/// Stop the series from `from_month` on (that month has no occurrence). Cancelling from the
/// first month removes the series.
pub async fn cancel_from(
    db: &dyn Storage,
    series_id: i64,
    from_month: &str,
    today: NaiveDate,
) -> Result<(), RecurringError> {
    if !is_month(from_month) {
        return invalid("month must be YYYY-MM");
    }
    let segs = series_segments(db, series_id).await?;
    let Some(current) = segs
        .iter()
        .rev()
        .find(|s| s.start_month.as_str() <= from_month)
    else {
        return invalid("the cancellation starts before the series does");
    };
    if !is_unsettled(from_month, current.day, today) {
        return invalid("that month has already settled; delete its transaction instead");
    }
    let old = oplog::segment_value(db, current).await;
    for s in segs {
        if s.start_month.as_str() >= from_month {
            db.delete_recurring(s.id).await?;
        } else if s.end_month.as_deref().is_none_or(|e| e >= from_month) {
            let mut cut = s.to_new();
            cut.end_month = Some(prev_month(from_month));
            db.update_recurring(s.id, &cut).await?;
        }
    }
    let new = Some(serde_json::json!({ "from_month": from_month }));
    oplog::record(db, kind::RECURRING_CANCEL, Some(series_id), old, new).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mysql::MySqlStorage;

    fn day(d: &str) -> NaiveDate {
        d.parse().unwrap()
    }

    /// The tests run on 2026-01-01 unless they say otherwise.
    async fn change(s: &MySqlStorage, id: i64, r: &NewRecurring) -> Result<(), RecurringError> {
        change_from(s, id, r, day("2026-01-01")).await
    }

    async fn cancel(s: &MySqlStorage, id: i64, month: &str) -> Result<(), RecurringError> {
        cancel_from(s, id, month, day("2026-01-01")).await
    }

    fn new(start: &str, end: Option<&str>, amount: i64) -> NewRecurring {
        NewRecurring {
            start_month: start.into(),
            end_month: end.map(String::from),
            day: 31,
            amount_agorot: amount,
            category_id: None,
            payment_method_id: None,
            note: "netflix".into(),
        }
    }

    /// (date, amount) of all occurrences through `through`.
    async fn all(s: &MySqlStorage, through: &str) -> Vec<(String, i64)> {
        occurrences(&s.list_recurring().await.unwrap(), through)
            .into_iter()
            .map(|o| (o.date, o.amount_agorot))
            .collect()
    }

    #[tokio::test]
    async fn fixed_length_and_open_ended_series() {
        let s = crate::testdb::fresh().await;
        create(&s, &new("2026-01", Some("2026-03"), -1000))
            .await
            .unwrap();
        // Short months use their last day.
        assert_eq!(
            all(&s, "2026-12").await,
            [
                ("2026-01-31".to_string(), -1000),
                ("2026-02-28".to_string(), -1000),
                ("2026-03-31".to_string(), -1000)
            ]
        );
        let s = crate::testdb::fresh().await;
        create(&s, &new("2026-11", None, -500)).await.unwrap();
        assert_eq!(all(&s, "2027-02").await.len(), 4);
    }

    #[tokio::test]
    async fn price_change_keeps_the_past() {
        let s = crate::testdb::fresh().await;
        let id = create(&s, &new("2026-01", None, -1000)).await.unwrap();
        change(&s, id, &new("2026-03", None, -1500)).await.unwrap();
        let got: Vec<i64> = all(&s, "2026-04").await.iter().map(|o| o.1).collect();
        assert_eq!(got, [-1000, -1000, -1500, -1500]);
        // A second change in a later month, then one in the very first month.
        change(&s, id, &new("2026-02", Some("2026-03"), -700))
            .await
            .unwrap();
        let got: Vec<i64> = all(&s, "2026-06").await.iter().map(|o| o.1).collect();
        assert_eq!(got, [-1000, -700, -700]); // later segments were replaced
        change(&s, id, &new("2026-01", None, -100)).await.unwrap();
        assert_eq!(all(&s, "2026-02").await.len(), 2);
        assert!(all(&s, "2026-02").await.iter().all(|o| o.1 == -100));
    }

    #[tokio::test]
    async fn cancel_from_a_month_and_entirely() {
        let s = crate::testdb::fresh().await;
        let id = create(&s, &new("2026-01", None, -1000)).await.unwrap();
        change(&s, id, &new("2026-04", None, -1200)).await.unwrap();
        cancel(&s, id, "2026-06").await.unwrap();
        let got = all(&s, "2027-01").await;
        assert_eq!(got.len(), 5);
        assert_eq!(got.last().unwrap().1, -1200);
        cancel(&s, id, "2026-01").await.unwrap();
        assert!(all(&s, "2027-01").await.is_empty());
    }

    #[tokio::test]
    async fn rejects_bad_requests() {
        let s = crate::testdb::fresh().await;
        assert!(matches!(
            create(&s, &new("2026-13", None, -1)).await,
            Err(RecurringError::Invalid(_))
        ));
        assert!(matches!(
            create(&s, &new("2026-05", Some("2026-04"), -1)).await,
            Err(RecurringError::Invalid(_))
        ));
        let id = create(&s, &new("2026-05", Some("2026-06"), -1))
            .await
            .unwrap();
        assert!(change(&s, id, &new("2026-04", None, -1)).await.is_err());
        assert!(change(&s, id, &new("2026-08", None, -1)).await.is_err());
        assert!(change(&s, 999, &new("2026-05", None, -1)).await.is_err());
    }

    #[tokio::test]
    async fn due_occurrences_settle_once_and_stay_settled() {
        let s = crate::testdb::fresh().await;
        let id = create(&s, &new("2026-01", None, -1000)).await.unwrap();
        // 2026-03-31 is not reached yet: January and February (28th) are due.
        assert_eq!(settle_due(&s, day("2026-03-15")).await.unwrap(), 3 - 1);
        assert_eq!(settle_due(&s, day("2026-03-15")).await.unwrap(), 0);
        let txs = s.list_transactions().await.unwrap();
        assert_eq!(txs.len(), 2);
        assert!(txs
            .iter()
            .all(|t| t.recurring_series_id == Some(id) && t.sync_state == "pending"));
        // A deleted settled transaction is not brought back.
        s.delete_transaction(txs[0].id).await.unwrap();
        assert_eq!(settle_due(&s, day("2026-03-16")).await.unwrap(), 0);
        assert_eq!(s.list_transactions().await.unwrap().len(), 1);
        // Later months settle as they come due.
        assert_eq!(settle_due(&s, day("2026-04-01")).await.unwrap(), 1);
        // The totals see settled ones as normal transactions; only the rest is upcoming.
        let ahead = upcoming(
            &s.list_recurring().await.unwrap(),
            "2026-05",
            day("2026-04-01"),
            &[],
        );
        assert_eq!(ahead.len(), 2);
        assert_eq!(ahead[0].date, "2026-04-30");
    }

    #[tokio::test]
    async fn one_month_can_be_edited_alone() {
        let s = crate::testdb::fresh().await;
        let id = create(&s, &new("2026-01", None, 1_200_000)).await.unwrap();
        let early = NewTransaction {
            date: "2026-03-03".into(),
            amount_agorot: 1_185_000,
            category_id: None,
            payment_method_id: None,
            note: "early".into(),
        };
        settle_month(&s, id, "2026-03", &early).await.unwrap();
        // Doing it twice, or for a month the series doesn't have, is refused.
        assert!(settle_month(&s, id, "2026-03", &early).await.is_err());
        assert!(settle_month(&s, id, "2025-12", &early).await.is_err());
        let ahead = upcoming_stored(&s, "2026-05", day("2026-02-20"))
            .await
            .unwrap();
        // March is now a transaction; February and April/May are still generated.
        assert_eq!(
            ahead.iter().map(|o| o.date.as_str()).collect::<Vec<_>>(),
            ["2026-02-28", "2026-04-30", "2026-05-31"]
        );
        let txs = s.list_transactions().await.unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(
            (txs[0].amount_agorot, txs[0].recurring_series_id),
            (1_185_000, Some(id))
        );
        // A skipped month neither makes a transaction nor is generated.
        skip_month(&s, id, "2026-04").await.unwrap();
        assert!(skip_month(&s, id, "2026-04").await.is_err());
        let ahead = upcoming_stored(&s, "2026-05", day("2026-02-20"))
            .await
            .unwrap();
        assert_eq!(
            ahead.iter().map(|o| o.date.as_str()).collect::<Vec<_>>(),
            ["2026-02-28", "2026-05-31"]
        );
        // Settling the rest later does not add a second March.
        settle_due(&s, day("2026-04-01")).await.unwrap();
        let all = s.list_transactions().await.unwrap();
        assert_eq!(
            all.iter().filter(|t| t.date.starts_with("2026-03")).count(),
            1
        );
        assert!(all.iter().all(|t| !t.date.starts_with("2026-04")));
    }

    #[tokio::test]
    async fn settled_months_cannot_be_changed() {
        let s = crate::testdb::fresh().await;
        let id = create(&s, &new("2026-01", None, -1000)).await.unwrap();
        let on = day("2026-03-10"); // January and February have settled
        assert!(matches!(
            change_from(&s, id, &new("2026-02", None, -1), on).await,
            Err(RecurringError::Invalid(_))
        ));
        assert!(matches!(
            cancel_from(&s, id, "2026-02", on).await,
            Err(RecurringError::Invalid(_))
        ));
        // The current month is still ahead until its charge date.
        change_from(&s, id, &new("2026-03", None, -1500), on)
            .await
            .unwrap();
        cancel_from(&s, id, "2026-04", on).await.unwrap();
        let got = all(&s, "2026-12").await;
        assert_eq!(got.len(), 3);
        assert_eq!(got.last().unwrap().1, -1500);
    }

    #[test]
    fn totals_include_occurrences() {
        let seg = RecurringSegment {
            id: 1,
            series_id: 1,
            start_month: "2026-01".into(),
            end_month: Some("2026-02".into()),
            day: 5,
            amount_agorot: -100,
            category_id: Some(7),
            payment_method_id: None,
            note: String::new(),
        };
        let mut totals = vec![MonthlyTotal {
            month: "2026-02".into(),
            category_id: Some(7),
            total_agorot: -50,
            count: 1,
        }];
        add_to_totals(&mut totals, &occurrences(&[seg], "2026-12"));
        assert_eq!(
            totals
                .iter()
                .map(|t| (t.month.as_str(), t.total_agorot, t.count))
                .collect::<Vec<_>>(),
            [("2026-02", -150, 2), ("2026-01", -100, 1)]
        );
    }
}
