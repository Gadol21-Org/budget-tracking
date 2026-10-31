//! Client for Paamonim (Rishumit). There is no public API: we log in through the
//! HTML form and call the cookie-authenticated `/rishumit/*` endpoints.
//!
//! NOTE: written from the site's public JS only; response shapes and the
//! `save_item_line` field names still need verifying against a real account.

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;

/// A logged-in session. Paamonim answers any request whose session has expired with a
/// redirect to the login page; the client then logs in again once and resends the request
/// (the expired request was not acted on), so one `Client` can be kept for many runs.
pub struct Client {
    http: reqwest::Client,
    base: String,
    /// Username and password, to log in again; `None` for a test client.
    credentials: Option<(String, String)>,
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
    /// From the `payment-<id>` class and the text beside it (`3 מתוך 10`): the line is one
    /// installment of a group whose root line is `<id>`. Not a payment method.
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
        let client = Self {
            http: reqwest::Client::builder().cookie_store(true).build()?,
            base: base.trim_end_matches('/').to_string(),
            credentials: Some((username.to_string(), password.to_string())),
        };
        client.sign_in().await?;
        Ok(client)
    }

    /// Log in through the HTML form; the session cookie lands in the client's cookie store.
    async fn sign_in(&self) -> Result<()> {
        let Some((username, password)) = &self.credentials else {
            bail!("no Paamonim credentials to log in with");
        };
        let base = &self.base;
        let page = self
            .http
            .get(format!("{base}/"))
            .send()
            .await?
            .text()
            .await?;
        let token = {
            let doc = Html::parse_document(&page);
            let sel = Selector::parse("input[name=authenticity_token]").unwrap();
            doc.select(&sel)
                .next()
                .and_then(|e| e.value().attr("value"))
                .context("authenticity_token not found on login page")?
                .to_string()
        };

        let resp = self
            .http
            .post(format!("{base}/login"))
            .form(&[
                ("utf8", "\u{2713}"),
                ("authenticity_token", token.as_str()),
                ("login", username.as_str()),
                ("password", password.as_str()),
                ("save[login]", "0"),
            ])
            .send()
            .await?;
        let body = resp.text().await?;
        // Still on the login form => credentials rejected.
        if is_login_page(&body) {
            bail!("Paamonim login failed (still on login form)");
        }
        Ok(())
    }

    /// Send a request built by `build`, returning (status, body). If the session has expired,
    /// log in again and send it once more. Redirects are followed, so an expired session shows
    /// up as the login page (or a 401), never as a 404 or a success.
    async fn send(
        &self,
        build: impl Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<(u16, String)> {
        let (status, body) = self.send_once(&build).await?;
        if !session_expired(status, &body) {
            return Ok((status, body));
        }
        if self.credentials.is_none() {
            bail!("Paamonim session expired");
        }
        tracing::info!("paamonim session expired; logging in again");
        self.sign_in().await?;
        let (status, body) = self.send_once(&build).await?;
        if session_expired(status, &body) {
            bail!("Paamonim session expired, and logging in again did not help");
        }
        Ok((status, body))
    }

    async fn send_once(
        &self,
        build: &impl Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<(u16, String)> {
        let resp = build(&self.http, &self.base)
            .header("X-Requested-With", "XMLHttpRequest")
            .send()
            .await?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await?))
    }

    /// Raw GET returning (status, body); used by the `probe` command to record real responses.
    pub async fn get_raw(&self, path: &str, query: &[(&str, &str)]) -> Result<(u16, String)> {
        self.send(|http, base| http.get(format!("{base}{path}")).query(query))
            .await
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

    /// Delete a history line: the site POSTs `line_id` to `path` (`/rishumit/delete_history_line`,
    /// answered with the text `deleted`).
    pub async fn delete_item_line(&self, path: &str, param: &str, line_id: &str) -> Result<()> {
        let (status, _) = self
            .send(|http, base| http.post(format!("{base}{path}")).form(&[(param, line_id)]))
            .await?;
        ok_status(path, status)
    }

    /// The day of history line `line_id`, read from its edit form; `None` if Paamonim
    /// answers 404 (no such line, e.g. already deleted).
    pub async fn line_date(&self, line_id: &str) -> Result<Option<String>> {
        let (status, body) = self
            .get_raw("/rishumit/edit_item_line", &[("line_id", line_id)])
            .await?;
        if status == 404 {
            return Ok(None);
        }
        if status != 200 {
            bail!("edit_item_line for line {line_id} returned HTTP {status}");
        }
        let html = match serde_json::from_str::<HtmlEnvelope>(&body) {
            Ok(env) => env.html,
            Err(_) => body,
        };
        let doc = Html::parse_fragment(&html);
        let sel = Selector::parse("input[name=\"balance_history[payment_date]\"]").unwrap();
        doc.select(&sel)
            .next()
            .and_then(|e| e.value().attr("value"))
            .map(|d| Some(d.to_string()))
            .context("payment date not found in the line's edit form")
    }

    /// The family's own items of one kind (`expense` / `income`).
    pub async fn custom_items(&self, item_type: &str) -> Result<Vec<CustomItem>> {
        let html = self
            .fragment("/rishumit/get_custom_items", &[("item_type", item_type)])
            .await?;
        Ok(parse_custom_items(&html))
    }

    /// Add an item of the family's own (`item_type` `expense` / `income`) under Paamonim
    /// category `category_id`, as the site's "add item" form does; with `existing`, move that
    /// item there instead (the form's edit mode). Returns nothing; read the items again to get
    /// its id.
    pub async fn save_custom_item(
        &self,
        name: &str,
        item_type: &str,
        category_id: &str,
        existing: Option<&crate::paamonim::CustomItem>,
    ) -> Result<()> {
        let (_, page) = self.get_raw("/rishumit", &[]).await?;
        let token = {
            let doc = Html::parse_document(&page);
            let input = Selector::parse("input[name=authenticity_token]").unwrap();
            let meta = Selector::parse("meta[name=csrf-token]").unwrap();
            doc.select(&input)
                .next()
                .and_then(|e| e.value().attr("value"))
                .or_else(|| {
                    doc.select(&meta)
                        .next()
                        .and_then(|e| e.value().attr("content"))
                })
                .unwrap_or_default()
                .to_string()
        };
        let (status, _) = self
            .send(|http, base| {
                http.post(format!("{base}/rishumit/save_family_custom_item"))
                    .form(&[
                        ("utf8", "\u{2713}"),
                        ("authenticity_token", token.as_str()),
                        // The category the site files the item under is the form's
                        // `budget_category` select; `budget_category_id` only carries the
                        // edited item's current category, to preselect it.
                        ("budget_category", category_id),
                        ("new_budget_item_name", name),
                        ("item_type", item_type),
                        (
                            "custom_item_id",
                            existing.map_or("", |i| i.item_id.as_str()),
                        ),
                        (
                            "budget_category_id",
                            existing
                                .and_then(|i| i.category_id.as_deref())
                                .unwrap_or(""),
                        ),
                    ])
            })
            .await?;
        ok_status("save_family_custom_item", status)
    }

    /// A client for a test server, skipping the login.
    #[cfg(test)]
    pub fn for_tests(base: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: base.trim_end_matches('/').to_string(),
            credentials: None,
        }
    }

    /// Mirror a line to Paamonim. `amount_shekels` must already be rounded.
    pub async fn save_item_line(&self, fields: &[(&str, String)]) -> Result<()> {
        let (status, _) = self
            .send(|http, base| {
                http.get(format!("{base}/rishumit/save_item_line"))
                    .query(fields)
            })
            .await?;
        ok_status("save_item_line", status)
    }
}

fn is_login_page(body: &str) -> bool {
    body.contains("id=\"frm_login\"")
}

/// Paamonim redirects requests without a valid session to the login page.
fn session_expired(status: u16, body: &str) -> bool {
    status == 401 || is_login_page(body)
}

fn ok_status(what: &str, status: u16) -> Result<()> {
    if !(200..300).contains(&status) {
        bail!("{what} returned HTTP {status}");
    }
    Ok(())
}

pub(crate) const HEBREW_MONTHS: [&str; 12] = [
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

    /// A pretend Paamonim whose session can be expired: requests without a live session are
    /// redirected to the login page, as the real site does.
    async fn expiring_site() -> (String, std::sync::Arc<std::sync::Mutex<(u32, u32)>>) {
        use axum::{http::HeaderMap, response::IntoResponse, routing::get, routing::post, Router};
        use std::sync::{Arc, Mutex};
        // (logins so far, session number the server accepts)
        let state = Arc::new(Mutex::new((0u32, 0u32)));
        let live = |s: &Arc<Mutex<(u32, u32)>>, h: &HeaderMap| {
            let want = format!("s={}", s.lock().unwrap().1);
            h.get("cookie")
                .and_then(|c| c.to_str().ok())
                .is_some_and(|c| c.split("; ").any(|kv| kv == want))
        };
        let login_page =
            r#"<form id="frm_login"><input name="authenticity_token" value="t"></form>"#;
        let to_login = || {
            (
                axum::http::StatusCode::FOUND,
                [("location", "/")],
                "You are being redirected",
            )
                .into_response()
        };
        let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state.clone());
        let app = Router::new()
            .route(
                "/",
                get(move |h: HeaderMap| async move {
                    if live(&s1, &h) {
                        "home"
                    } else {
                        login_page
                    }
                }),
            )
            .route(
                "/login",
                post(move || async move {
                    let mut st = s2.lock().unwrap();
                    st.0 += 1;
                    st.1 += 1;
                    let cookie = format!("s={}; Path=/", st.1);
                    (
                        axum::http::StatusCode::FOUND,
                        [("location", "/".to_string()), ("set-cookie", cookie)],
                    )
                        .into_response()
                }),
            )
            .route(
                "/rishumit/activity_history",
                get(move |h: HeaderMap| async move {
                    if live(&s3, &h) {
                        r#"{"html":"history"}"#.into_response()
                    } else {
                        to_login()
                    }
                }),
            )
            .route(
                "/rishumit/edit_item_line",
                get(move |h: HeaderMap| async move {
                    if live(&s4, &h) {
                        (axum::http::StatusCode::NOT_FOUND, "").into_response()
                    } else {
                        to_login()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, state)
    }

    #[tokio::test]
    async fn logs_in_again_when_the_session_expires() {
        let (base, state) = expiring_site().await;
        let client = Client::login(&base, "u", "p").await.unwrap();
        assert_eq!(
            client.activity_history_html("a", "b").await.unwrap(),
            "history"
        );
        assert_eq!(state.lock().unwrap().0, 1, "the session is reused");
        assert_eq!(
            client.activity_history_html("a", "b").await.unwrap(),
            "history"
        );
        assert_eq!(state.lock().unwrap().0, 1);

        // The server forgets the session.
        state.lock().unwrap().1 += 1;
        assert_eq!(
            client.activity_history_html("a", "b").await.unwrap(),
            "history"
        );
        assert_eq!(state.lock().unwrap().0, 2, "logged in again once");

        // An expired session is never read as "no such line".
        state.lock().unwrap().1 += 1;
        assert_eq!(client.line_date("7").await.unwrap(), None);
        assert_eq!(state.lock().unwrap().0, 3);
    }

    #[tokio::test]
    async fn expired_session_without_credentials_is_an_error() {
        let (base, _) = expiring_site().await;
        let client = Client::for_tests(&base);
        let err = client.line_date("7").await.unwrap_err();
        assert!(err.to_string().contains("expired"), "{err}");
    }
}
