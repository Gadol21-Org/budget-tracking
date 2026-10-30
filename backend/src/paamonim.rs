//! Client for Paamonim (Rishumit). There is no public API: we log in through the
//! HTML form and call the cookie-authenticated `/rishumit/*` endpoints.
//!
//! NOTE: written from the site's public JS only; response shapes and the
//! `save_item_line` field names still need verifying against a real account.

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;

pub struct Client {
    http: reqwest::Client,
    base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Expense,
    Income,
}

/// One `.historyLine` of the activity-history fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub line_id: String,
    pub date: NaiveDate,
    /// Budget item name; matches a [`BudgetItem`] name (history lines carry no item id).
    pub item_name: String,
    /// Whole shekels, always positive; direction is in `kind`.
    pub amount_shekels: i64,
    pub kind: Kind,
    /// Free-text note (first `<small>`), possibly empty.
    pub note: String,
    /// Second `<small>`; takes only a handful of distinct values, possibly empty.
    pub label: String,
    /// Id from the `payment-<id>` class and the text beside it, when the line has one.
    pub payment: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetItem {
    pub item_id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct HtmlEnvelope {
    html: String,
}

impl Client {
    pub async fn login(base: &str, username: &str, password: &str) -> Result<Self> {
        let http = reqwest::Client::builder().cookie_store(true).build()?;
        let base = base.trim_end_matches('/').to_string();

        let page = http.get(format!("{base}/")).send().await?.text().await?;
        let token = {
            let doc = Html::parse_document(&page);
            let sel = Selector::parse("input[name=authenticity_token]").unwrap();
            doc.select(&sel)
                .next()
                .and_then(|e| e.value().attr("value"))
                .context("authenticity_token not found on login page")?
                .to_string()
        };

        let resp = http
            .post(format!("{base}/login"))
            .form(&[
                ("utf8", "\u{2713}"),
                ("authenticity_token", token.as_str()),
                ("login", username),
                ("password", password),
                ("save[login]", "0"),
            ])
            .send()
            .await?;
        let body = resp.text().await?;
        // Still on the login form => credentials rejected.
        if body.contains("id=\"frm_login\"") {
            bail!("Paamonim login failed (still on login form)");
        }
        Ok(Self { http, base })
    }

    /// Raw GET returning (status, body); used by the `probe` command to record real responses.
    pub async fn get_raw(&self, path: &str, query: &[(&str, &str)]) -> Result<(u16, String)> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(query)
            .send()
            .await?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await?))
    }

    /// GET an endpoint and return its `html` fragment (JSON envelope unwrapped).
    pub async fn fragment(&self, path: &str, query: &[(&str, &str)]) -> Result<String> {
        let (status, body) = self.get_raw(path, query).await?;
        if status != 200 {
            bail!("{path} returned HTTP {status}");
        }
        Ok(match serde_json::from_str::<HtmlEnvelope>(&body) {
            Ok(env) => env.html,
            Err(_) => body,
        })
    }

    /// Fetch raw activity-history HTML for a date range (YYYY-MM-DD).
    pub async fn activity_history_html(&self, from: &str, to: &str) -> Result<String> {
        // Same request as the recorded probe, which Paamonim answered with 200. An extra
        // `Accept: application/json` header made it respond with HTTP 500.
        self.fragment(
            "/rishumit/activity_history",
            &[("start_date", from), ("end_date", to)],
        )
        .await
    }

    /// The category Paamonim files each standard item under (expense items, income items).
    /// `family_id` is the account id from the edit-item form.
    pub async fn item_categories(
        &self,
        family_id: &str,
    ) -> Result<(Vec<ItemCategory>, Vec<ItemCategory>)> {
        let (status, body) = self
            .get_raw(
                "/budgets/budget_and_balances",
                &[
                    ("budget_id", ""),
                    ("balance_from", "0"),
                    ("balance_to", "0"),
                    ("family_id", family_id),
                ],
            )
            .await?;
        if status != 200 {
            bail!("budget_and_balances returned HTTP {status}");
        }
        parse_item_categories(&body)
    }

    /// Mirror a line to Paamonim. `amount_shekels` must already be rounded.
    pub async fn save_item_line(&self, fields: &[(&str, String)]) -> Result<()> {
        self.http
            .get(format!("{}/rishumit/save_item_line", self.base))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(fields)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

const HEBREW_MONTHS: [&str; 12] = [
    "ינואר",
    "פברואר",
    "מרץ",
    "אפריל",
    "מאי",
    "יוני",
    "יולי",
    "אוגוסט",
    "ספטמבר",
    "אוקטובר",
    "נובמבר",
    "דצמבר",
];

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses a date row such as `שלישי, 3 בפברואר 2026`. Relative labels (`היום`, `אתמול`)
/// replace the weekday but still carry the full date. The month has a `ב` prefix.
fn parse_date_row(text: &str) -> Option<NaiveDate> {
    let (_, rest) = text.split_once(',')?;
    let mut parts = rest.split_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let month_word = parts.next()?;
    let year: i32 = parts.next()?.parse().ok()?;
    let month_word = month_word.strip_prefix('ב').unwrap_or(month_word);
    let month = HEBREW_MONTHS.iter().position(|m| *m == month_word)? as u32 + 1;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Parses `1,234` (whole shekels).
fn parse_amount(text: &str) -> Option<i64> {
    let digits: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',')
        .collect();
    digits.parse().ok()
}

/// Parses the `html` of the `activity_history` response. Date rows only appear when the date
/// changes, so a line inherits the most recent one; lines before any date row are dropped, as
/// are lines whose amount cannot be parsed.
pub fn parse_history(html: &str) -> Vec<HistoryEntry> {
    let doc = Html::parse_fragment(html);
    let rows = Selector::parse(".date-row, .historyLine").unwrap();
    let amount_sel = Selector::parse(".budgetMinus, .budgetPlus").unwrap();
    let small_sel = Selector::parse("small").unwrap();
    let mut date = None;
    let mut out = Vec::new();
    for row in doc.select(&rows) {
        if row
            .value()
            .has_class("date-row", scraper::CaseSensitivity::CaseSensitive)
        {
            date = parse_date_row(&squash(&row.text().collect::<String>()));
            continue;
        }
        let (Some(date), Some(line_id)) = (date, row.value().attr("line_id")) else {
            continue;
        };
        let Some(amount_el) = row.select(&amount_sel).next() else {
            continue;
        };
        let Some(amount_shekels) = parse_amount(&amount_el.text().collect::<String>()) else {
            continue;
        };
        let kind = if amount_el
            .value()
            .has_class("budgetPlus", scraper::CaseSensitivity::CaseSensitive)
        {
            Kind::Income
        } else {
            Kind::Expense
        };
        // The item name is the text of the first child div, minus the amount span.
        let item_name = row
            .children()
            .filter_map(ElementRef::wrap)
            .next()
            .map(|d| {
                squash(
                    &d.children()
                        .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
                        .collect::<String>(),
                )
            })
            .unwrap_or_default();
        let (mut plain, mut payment) = (Vec::new(), None);
        for small in row.select(&small_sel) {
            let text = squash(&small.text().collect::<String>());
            let pay_id = small
                .value()
                .classes()
                .find_map(|c| c.strip_prefix("payment-"));
            match pay_id {
                Some(id) => payment = Some((id.to_string(), text)),
                None => plain.push(text),
            }
        }
        let mut plain = plain.into_iter();
        out.push(HistoryEntry {
            line_id: line_id.to_string(),
            date,
            item_name,
            amount_shekels,
            kind,
            note: plain.next().unwrap_or_default(),
            label: plain.next().unwrap_or_default(),
            payment,
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub id: String,
    pub name: String,
}

/// A user-defined item from `get_custom_items`; the only place an item names its category id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomItem {
    pub item_id: String,
    pub name: String,
    pub category_id: Option<String>,
}

/// Parses the category `<select name="budget_category">` embedded in the `get_custom_items`
/// response (identical for `item_type=expense` and `income`). The list is flat.
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_fragment(html);
    let sel = Selector::parse("select[name=budget_category] option").unwrap();
    doc.select(&sel)
        .filter_map(|o| {
            let id = o.value().attr("value").filter(|v| !v.is_empty())?;
            Some(Category {
                id: id.to_string(),
                name: squash(&o.text().collect::<String>()),
            })
        })
        .collect()
}

/// Parses the `.customItem` rows of `get_custom_items`.
pub fn parse_custom_items(html: &str) -> Vec<CustomItem> {
    let doc = Html::parse_fragment(html);
    let sel = Selector::parse(".customItem").unwrap();
    doc.select(&sel)
        .filter_map(|e| {
            Some(CustomItem {
                item_id: e.value().attr("item_id")?.to_string(),
                name: squash(&e.text().collect::<String>()),
                category_id: e
                    .value()
                    .attr("budget_category_id")
                    .filter(|v| !v.is_empty())
                    .map(String::from),
            })
        })
        .collect()
}

/// A standard item with the category Paamonim files it under, from `budget_and_balances`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCategory {
    /// Same id as `BudgetItem::item_id`.
    pub item_id: String,
    pub full_name: String,
    pub category_name: String,
}

/// Parses the JSON of `GET /budgets/budget_and_balances?budget_id=&balance_from=0&balance_to=0&family_id=`.
/// The body is `[expense_rows, income_rows]`; each starts with a titles row (no `id`) that is skipped.
/// Returns `(expense, income)`. Categories are given by name only, and the two kinds share none.
pub fn parse_item_categories(json: &str) -> Result<(Vec<ItemCategory>, Vec<ItemCategory>)> {
    let parts: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(json).context("budget_and_balances is not [[..],[..]]")?;
    let rows = |part: Option<&Vec<serde_json::Value>>| -> Vec<ItemCategory> {
        part.into_iter()
            .flatten()
            .filter_map(|r| {
                Some(ItemCategory {
                    item_id: r.get("id")?.to_string(),
                    full_name: r.get("full_name")?.as_str()?.trim().to_string(),
                    category_name: r.get("BudgetCategoryName")?.as_str()?.trim().to_string(),
                })
            })
            .collect()
    };
    Ok((rows(parts.first()), rows(parts.get(1))))
}

/// Parses `income_items` / `expense_items` HTML into budget items.
pub fn parse_items(html: &str) -> Vec<BudgetItem> {
    let doc = Html::parse_fragment(html);
    let sel = Selector::parse(".rishumitBudgetItem").unwrap();
    doc.select(&sel)
        .filter_map(|e| {
            Some(BudgetItem {
                item_id: e.value().attr("item_id")?.to_string(),
                name: squash(&e.text().collect::<String>()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic fixture mirroring the structure of the real activity_history fragment.
    const HISTORY: &str = r#"<div class="row"><div class="list-group">
		<!-- date title from helper -->
			<div class="list-group-item date-row">היום, 30 באוקטובר 2026</div>
		<div class="list-group-item historyLine" line_id="1001">
			<div>סופר
				<span class="pull-right budgetMinus">
					1,234
				</span>
			</div>
			<div><small>קניות שבועיות</small></div>
			<div>
				<small>תווית</small>
			</div>
		</div>
		<!-- date title from helper -->

		<div class="list-group-item historyLine" line_id="1002">
			<div>משכורת
				<span class="pull-right budgetPlus">12,000</span>
			</div>
			<div><small></small></div>
			<div>
				<small></small>
					<small class="pull-right payment-77">5 ינו 26</small>
			</div>
		</div>
		<!-- date title from helper -->
			<div class="list-group-item date-row">אתמול, 29 במרץ 2026</div>
		<div class="list-group-item historyLine" line_id="1003">
			<div>קפה <span class="pull-right budgetMinus">9</span></div>
			<div><small></small></div><div><small></small></div>
		</div>
	</div></div>"#;

    #[test]
    fn parses_lines_with_inherited_dates() {
        let got = parse_history(HISTORY);
        assert_eq!(got.len(), 3);
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        assert_eq!(
            got[0],
            HistoryEntry {
                line_id: "1001".into(),
                date: d(10, 30),
                item_name: "סופר".into(),
                amount_shekels: 1234,
                kind: Kind::Expense,
                note: "קניות שבועיות".into(),
                label: "תווית".into(),
                payment: None,
            }
        );
        // No date row before this line: inherits the previous date.
        assert_eq!(got[1].date, d(10, 30));
        assert_eq!(got[1].kind, Kind::Income);
        assert_eq!(got[1].amount_shekels, 12000);
        assert_eq!(got[1].note, "");
        assert_eq!(got[1].payment, Some(("77".into(), "5 ינו 26".into())));
        assert_eq!(got[2].date, d(3, 29));
        assert_eq!(got[2].item_name, "קפה");
    }

    #[test]
    fn drops_lines_before_first_date_row_and_bad_amounts() {
        let html = r#"<div class="historyLine" line_id="1"><div>x<span class="budgetMinus">5</span></div></div>
            <div class="date-row">שני, 1 בינואר 2026</div>
            <div class="historyLine" line_id="2"><div>x<span class="budgetMinus">??</span></div></div>"#;
        assert!(parse_history(html).is_empty());
    }

    #[test]
    fn date_rows() {
        assert_eq!(
            parse_date_row("שבת, 3 בספטמבר 2022"),
            NaiveDate::from_ymd_opt(2022, 9, 3)
        );
        assert_eq!(parse_date_row("שבת, 31 בפברואר 2022"), None);
        assert_eq!(parse_date_row("garbage"), None);
    }

    #[test]
    fn parses_items() {
        let html = r##"<div class="list-group"><a href="#" class="list-group-item rishumitBudgetItem" item_id="12">מזון - סופר
            <span class="glyphicon"></span></a></div>"##;
        assert_eq!(
            parse_items(html),
            vec![BudgetItem {
                item_id: "12".into(),
                name: "מזון - סופר".into()
            }]
        );
    }

    /// Sanity check against a real `probe` dump: `PROBE_DIR=probe-out cargo test -- --ignored`.
    /// Prints counts only, never data.
    #[test]
    #[ignore]
    fn real_probe_dump() {
        let dir = std::env::var("PROBE_DIR").expect("PROBE_DIR");
        let html = |name: &str| {
            let raw = std::fs::read_to_string(format!("{dir}/{name}.txt")).unwrap();
            let body = raw.split_once('\n').unwrap().1;
            serde_json::from_str::<HtmlEnvelope>(body).unwrap().html
        };
        let history_html = html("activity_history");
        let entries = parse_history(&history_html);
        let raw_lines = history_html.matches("line_id=").count();
        let items: Vec<_> = parse_items(&html("expense_items"))
            .into_iter()
            .chain(parse_items(&html("income_items")))
            .collect();
        let unmatched = entries
            .iter()
            .filter(|e| !items.iter().any(|i| i.name == e.item_name))
            .count();
        println!(
            "lines={raw_lines} parsed={} items={} unmatched={unmatched}",
            entries.len(),
            items.len()
        );
        assert_eq!(raw_lines, entries.len());
        assert_eq!(unmatched, 0);
    }

    #[test]
    fn parses_categories_and_custom_items() {
        let html = r##"<div class="budgetCategoriesToCut"><select name="budget_category" id="budget_category">
            <option value="" label=" "></option><option value="3">מזון</option><option value="7">רכב</option></select></div>
            <a href="#" class="list-group-item customItem" item_id="90" budget_category_id="7">דלק</a>
            <a href="#" class="list-group-item customItem" item_id="91">אחר</a>"##;
        let cats = parse_categories(html);
        assert_eq!(
            cats,
            vec![
                Category {
                    id: "3".into(),
                    name: "מזון".into()
                },
                Category {
                    id: "7".into(),
                    name: "רכב".into()
                }
            ]
        );
        let items = parse_custom_items(html);
        assert_eq!(
            items[0],
            CustomItem {
                item_id: "90".into(),
                name: "דלק".into(),
                category_id: Some("7".into())
            }
        );
        assert_eq!(items[1].category_id, None);
    }

    #[test]
    fn parses_item_categories() {
        let json = r#"[
            [{"avg_title":"a","balances_titles":[],"budget_title":"b","future_payments":"c","remaining_budget_title":"d","type_title":"e"},
             {"id":5,"full_name":"סופר ","BudgetCategoryName":"מזון","Average":10,"BAL1":3,"RemainingBudget":null,"FutureLiabilities":0},
             {"id":6,"full_name":"דלק","BudgetCategoryName":"רכב","Average":1}],
            [{"avg_title":"a"},
             {"id":9,"full_name":"משכורת","BudgetCategoryName":"הכנסות","Average":0}]]"#;
        let (exp, inc) = parse_item_categories(json).unwrap();
        assert_eq!(exp.len(), 2);
        assert_eq!(
            exp[0],
            ItemCategory {
                item_id: "5".into(),
                full_name: "סופר".into(),
                category_name: "מזון".into()
            }
        );
        assert_eq!(
            inc,
            vec![ItemCategory {
                item_id: "9".into(),
                full_name: "משכורת".into(),
                category_name: "הכנסות".into()
            }]
        );
        assert!(parse_item_categories("{}").is_err());
    }
}
