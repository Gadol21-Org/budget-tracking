//! Foreign-currency transactions (USD, JPY). The shekel is always the stored/reporting
//! currency: a foreign entry keeps its original amount and the rate used (`TxFx`), and
//! `amount_agorot` is the shekel value, so totals, Paamonim mirroring and the Sheets backup
//! keep working on shekels only.
//!
//! The rate is the one for the transaction's day. A day that has closed gets that day's
//! published rate, which is final. Today's (or a future) entry gets the newest rate and is
//! marked *estimated*; it is replaced by the day's real rate once the day has passed (see
//! `refresh_estimates`), mirroring a card charge that settles a day or more after the swipe.
//! Rates come from free public APIs, tried in turn, and are cached in the database; when none
//! answers, the newest cached rate is used (estimated). Nothing cached and nothing reachable
//! is an error, so a phone that was offline just keeps the entry queued until it can be sent.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::storage::{Storage, TxFx};

pub const CURRENCIES: [&str; 2] = ["USD", "JPY"];

/// Decimal places of the currency's minor unit (cents; the yen has none).
pub fn decimals(currency: &str) -> u32 {
    if currency == "JPY" {
        0
    } else {
        2
    }
}

pub fn check_currency(currency: &str) -> Result<()> {
    if CURRENCIES.contains(&currency) {
        Ok(())
    } else {
        bail!("unsupported currency {currency}")
    }
}

/// Shekel value in agorot of `orig_minor` at `rate` (shekels per unit), same sign.
pub fn to_agorot(currency: &str, orig_minor: i64, rate: f64) -> i64 {
    let units = orig_minor as f64 / 10f64.powi(decimals(currency) as i32);
    (units * rate * 100.0).round() as i64
}

/// The rate implied by a shekel amount the user typed (what the card really charged).
pub fn implied_rate(currency: &str, orig_minor: i64, agorot: i64) -> f64 {
    let units = orig_minor as f64 / 10f64.powi(decimals(currency) as i32);
    (agorot as f64 / 100.0) / units
}

#[derive(Debug, PartialEq)]
pub struct Rate {
    /// Shekels per unit.
    pub rate: f64,
    /// The day the rate is for.
    pub date: String,
    /// A stand-in until the day's rate is known.
    pub estimated: bool,
}

fn parse_frankfurter(v: &Value) -> Option<f64> {
    v["rates"]["ILS"].as_f64()
}

fn parse_fawaz(v: &Value, currency: &str) -> Option<f64> {
    v[currency.to_lowercase()]["ils"].as_f64()
}

fn parse_er_api(v: &Value) -> Option<f64> {
    v["rates"]["ILS"].as_f64()
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value> {
    let resp = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url}: {}", resp.status());
    }
    Ok(resp.json().await?)
}

/// Fetch a rate from the public APIs: `date` is a past day, or `None` for the newest.
/// Providers: Frankfurter (ECB), then the fawazahmed0 currency API, then (newest only)
/// open.er-api.com. `FX_FAKE_RATES=USD=3.6,JPY=0.024` replaces them (development, tests).
async fn fetch(currency: &str, date: Option<&str>) -> Result<f64> {
    if let Ok(fake) = std::env::var("FX_FAKE_RATES") {
        return fake
            .split(',')
            .filter_map(|p| p.split_once('='))
            .find(|(c, _)| c.trim() == currency)
            .and_then(|(_, r)| r.trim().parse().ok())
            .ok_or_else(|| anyhow!("no fake rate for {currency}"));
    }
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .build()?;
    let day = date.unwrap_or("latest");
    let mut errors = Vec::new();

    let url = format!("https://api.frankfurter.dev/v1/{day}?base={currency}&symbols=ILS");
    match get_json(&http, &url).await {
        Ok(v) => match parse_frankfurter(&v) {
            Some(r) if r > 0.0 => return Ok(r),
            _ => errors.push(format!("frankfurter: unexpected answer {v}")),
        },
        Err(e) => errors.push(e.to_string()),
    }

    let url = format!(
        "https://cdn.jsdelivr.net/npm/@fawazahmed0/currency-api@{day}/v1/currencies/{}.json",
        currency.to_lowercase()
    );
    match get_json(&http, &url).await {
        Ok(v) => match parse_fawaz(&v, currency) {
            Some(r) if r > 0.0 => return Ok(r),
            _ => errors.push("fawazahmed0: unexpected answer".into()),
        },
        Err(e) => errors.push(e.to_string()),
    }

    if date.is_none() {
        let url = format!("https://open.er-api.com/v6/latest/{currency}");
        match get_json(&http, &url).await {
            Ok(v) => match parse_er_api(&v) {
                Some(r) if r > 0.0 => return Ok(r),
                _ => errors.push("open.er-api: unexpected answer".into()),
            },
            Err(e) => errors.push(e.to_string()),
        }
    }
    bail!("no exchange-rate service answered: {}", errors.join("; "))
}

/// The rate for `currency` on `date` (YYYY-MM-DD), given today's date.
pub async fn rate_for(db: &dyn Storage, currency: &str, date: &str, today: &str) -> Result<Rate> {
    check_currency(currency)?;
    let closed = date < today;
    if closed {
        if let Some(rate) = db.get_fx_rate(currency, date).await? {
            return Ok(Rate {
                rate,
                date: date.into(),
                estimated: false,
            });
        }
        if let Ok(rate) = fetch(currency, Some(date)).await {
            let _ = db.put_fx_rate(currency, date, rate).await;
            return Ok(Rate {
                rate,
                date: date.into(),
                estimated: false,
            });
        }
    } else if let Ok(rate) = fetch(currency, None).await {
        let _ = db.put_fx_rate(currency, "latest", rate).await;
        return Ok(Rate {
            rate,
            date: today.into(),
            estimated: true,
        });
    }
    // Nothing answered: the newest rate we have seen, as a stand-in.
    match db.get_fx_rate(currency, "latest").await? {
        Some(rate) => Ok(Rate {
            rate,
            date: today.into(),
            estimated: true,
        }),
        None => bail!("exchange rate for {currency} is unavailable right now"),
    }
}

/// Build the stored details for a new or changed foreign amount.
pub fn build_fx(id: i64, currency: &str, orig_minor: i64, r: &Rate) -> TxFx {
    TxFx {
        transaction_id: id,
        currency: currency.into(),
        orig_minor,
        rate: r.rate,
        rate_date: r.date.clone(),
        estimated: r.estimated,
    }
}

/// Re-price transactions whose rate was a stand-in and whose day has since closed.
/// Best effort: returns how many were updated; a failed lookup leaves the stand-in.
pub async fn refresh_estimates(db: &dyn Storage, today: &str, limit: usize) -> usize {
    let Ok(all) = db.list_tx_fx().await else {
        return 0;
    };
    let Ok(txs) = db.list_transactions().await else {
        return 0;
    };
    let mut done = 0;
    for fx in all.into_iter().filter(|f| f.estimated) {
        if done >= limit {
            break;
        }
        let Some(t) = txs.iter().find(|t| t.id == fx.transaction_id) else {
            continue;
        };
        if t.date.as_str() >= today {
            continue;
        }
        if reprice(db, t, &fx, today).await.is_ok() {
            done += 1;
        }
    }
    done
}

/// Look up the rate for the transaction's own day and update its shekel amount.
pub async fn reprice(
    db: &dyn Storage,
    t: &crate::storage::Transaction,
    fx: &TxFx,
    today: &str,
) -> Result<TxFx> {
    let r = rate_for(db, &fx.currency, &t.date, today).await?;
    let new = build_fx(t.id, &fx.currency, fx.orig_minor, &r);
    let agorot = to_agorot(&fx.currency, fx.orig_minor, r.rate);
    if agorot != 0 && agorot != t.amount_agorot {
        db.update_transaction(
            t.id,
            &crate::storage::NewTransaction {
                date: t.date.clone(),
                amount_agorot: agorot,
                category_id: t.category_id,
                payment_method_id: t.payment_method_id,
                note: t.note.clone(),
            },
        )
        .await?;
    }
    db.set_tx_fx(t.id, Some(&new)).await?;
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{NewTransaction, SqliteStorage};
    use serde_json::json;
    use std::sync::Mutex;

    /// The tests set a process-wide variable, so they take turns.
    static ENV: Mutex<()> = Mutex::new(());

    #[test]
    fn converts_with_the_currencys_minor_unit() {
        assert_eq!(to_agorot("USD", -1250, 3.6), -4500);
        // 3000 yen at 0.0245 shekels = 73.50
        assert_eq!(to_agorot("JPY", -3000, 0.0245), -7350);
        assert_eq!(to_agorot("JPY", 1, 0.0245), 2);
        assert!((implied_rate("JPY", -3000, -7350) - 0.0245).abs() < 1e-9);
        assert!((implied_rate("USD", 1250, 4500) - 3.6).abs() < 1e-9);
    }

    #[test]
    fn parses_provider_answers() {
        assert_eq!(
            parse_frankfurter(&json!({"rates": {"ILS": 3.1284}})),
            Some(3.1284)
        );
        assert_eq!(
            parse_fawaz(&json!({"date": "x", "usd": {"ils": 3.2}}), "USD"),
            Some(3.2)
        );
        assert_eq!(parse_er_api(&json!({"rates": {"ILS": 3.3}})), Some(3.3));
        assert_eq!(parse_frankfurter(&json!({})), None);
    }

    #[tokio::test]
    async fn closed_days_are_final_and_cached_today_is_an_estimate() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("FX_FAKE_RATES", "USD=3.5,JPY=0.02");
        let s = SqliteStorage::open(":memory:").unwrap();
        let past = rate_for(&s, "USD", "2026-10-01", "2026-10-05")
            .await
            .unwrap();
        assert_eq!(past.rate, 3.5);
        assert!(!past.estimated);
        // Cached now: a changed source no longer matters for that day.
        std::env::set_var("FX_FAKE_RATES", "USD=9.9");
        assert_eq!(s.get_fx_rate("USD", "2026-10-01").await.unwrap(), Some(3.5));
        let again = rate_for(&s, "USD", "2026-10-01", "2026-10-05")
            .await
            .unwrap();
        assert_eq!(again.rate, 3.5);
        let today = rate_for(&s, "USD", "2026-10-05", "2026-10-05")
            .await
            .unwrap();
        assert!(today.estimated);
        assert_eq!(today.rate, 9.9);
        // Source gone: the newest rate seen is used, still an estimate.
        std::env::set_var("FX_FAKE_RATES", "EUR=1");
        let off = rate_for(&s, "USD", "2026-10-09", "2026-10-05")
            .await
            .unwrap();
        assert_eq!((off.rate, off.estimated), (9.9, true));
        assert!(rate_for(&s, "JPY", "2026-10-09", "2026-10-05")
            .await
            .is_err());
        assert!(rate_for(&s, "EUR", "2026-10-01", "2026-10-05")
            .await
            .is_err());
        std::env::remove_var("FX_FAKE_RATES");
    }

    #[tokio::test]
    async fn estimates_are_repriced_once_the_day_has_closed() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("FX_FAKE_RATES", "USD=3.0");
        let s = SqliteStorage::open(":memory:").unwrap();
        let id = s
            .create_transaction(&NewTransaction {
                date: "2026-10-05".into(),
                amount_agorot: -3000,
                category_id: None,
                payment_method_id: None,
                note: String::new(),
            })
            .await
            .unwrap();
        let est = Rate {
            rate: 3.0,
            date: "2026-10-05".into(),
            estimated: true,
        };
        s.set_tx_fx(id, Some(&build_fx(id, "USD", -1000, &est)))
            .await
            .unwrap();
        // Same day: untouched.
        assert_eq!(refresh_estimates(&s, "2026-10-05", 10).await, 0);
        std::env::set_var("FX_FAKE_RATES", "USD=3.2");
        assert_eq!(refresh_estimates(&s, "2026-10-06", 10).await, 1);
        let t = &s.list_transactions().await.unwrap()[0];
        assert_eq!(t.amount_agorot, -3200);
        let fx = &s.list_tx_fx().await.unwrap()[0];
        assert!(!fx.estimated);
        assert_eq!(fx.rate_date, "2026-10-05");
        // Deleting the transaction drops its details.
        s.delete_transaction(id).await.unwrap();
        assert!(s.list_tx_fx().await.unwrap().is_empty());
        std::env::remove_var("FX_FAKE_RATES");
    }
}
