//! Tests against a pretend Gemini API and the real routes: what arrives is read once, nothing is
//! lost when the AI is busy, the daily cap holds, and the inbound endpoint checks its caller.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use super::*;

#[derive(Default)]
struct World {
    /// Fail the next AI calls with these statuses.
    ai_fail: Vec<u16>,
    ai_calls: usize,
    prompts: Vec<String>,
    /// How many PDF parts each AI call carried.
    pdf_parts: Vec<usize>,
}

type W = Arc<Mutex<World>>;

/// The pretend model: a 12.50 expense for a "receipt", garbage for "weird", otherwise nothing.
async fn gemini(State(w): State<W>, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut w = w.lock().unwrap();
    w.ai_calls += 1;
    let prompt = b["contents"][0]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    w.prompts.push(prompt.clone());
    let pdfs = b["contents"][0]["parts"].as_array().map_or(0, |p| {
        p.iter()
            .filter(|x| x["inlineData"]["mimeType"] == "application/pdf")
            .count()
    });
    w.pdf_parts.push(pdfs);
    if !w.ai_fail.is_empty() {
        let code = w.ai_fail.remove(0);
        return (
            StatusCode::from_u16(code).unwrap(),
            Json(json!({ "error": { "message": "busy" } })),
        );
    }
    let reply = |text: String| {
        (
            StatusCode::OK,
            Json(
                json!({ "candidates": [{ "content": { "parts": [{ "text": text }] }, "finishReason": "STOP" }], "usageMetadata": { "promptTokenCount": 1000, "candidatesTokenCount": 100, "thoughtsTokenCount": 50 } }),
            ),
        )
    };
    if prompt.contains("weird") {
        return reply("not json at all".into());
    }
    let txs = if prompt.contains("receipt") || pdfs > 0 {
        json!([{ "amount": 12.5, "currency": "ILS", "date": "2026-03-01", "note": "Corner shop", "category": "Food > Groceries", "confidence": if prompt.contains("sure") { "high" } else { "medium" } }])
    } else {
        json!([])
    };
    reply(json!({ "transactions": txs }).to_string())
}

async fn start(w: W) -> String {
    let app = Router::new()
        .route("/v1beta/models/test-model:generateContent", post(gemini))
        .with_state(w);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    base
}

fn config(base: &str) -> Config {
    Config {
        inbound_secret: "s3cret".into(),
        ai_key: Some("test-key".into()),
        model: "test-model".into(),
        max_per_day: 100,
        ai_base: base.into(),
    }
}

struct Rig {
    svc: Arc<Service>,
    w: W,
    db: Db,
    base: String,
}

async fn rig_with(f: impl FnOnce(&mut Config)) -> Rig {
    let w: W = Arc::default();
    let base = start(w.clone()).await;
    let db: Db = Arc::new(crate::testdb::fresh().await);
    let new = |name: &str, parent: Option<i64>| crate::storage::NewCategory {
        name: name.into(),
        is_income: false,
        parent_id: parent,
        linked_to: None,
    };
    let food = db.create_category(&new("Food", None)).await.unwrap();
    db.create_category(&new("Groceries", Some(food)))
        .await
        .unwrap();
    let mut cfg = config(&base);
    f(&mut cfg);
    Rig {
        svc: Service::new(db.clone(), Some(cfg)),
        w,
        db,
        base,
    }
}

async fn rig() -> Rig {
    rig_with(|_| {}).await
}

fn eml(id: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Shop <shop@example.com>\r\nTo: budget@example.com\r\nSubject: {subject}\r\nMessage-ID: <{id}@example.com>\r\nDate: Mon, 02 Mar 2026 10:00:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

async fn rows(r: &Rig, status: &str) -> Vec<crate::storage::EmailSuggestion> {
    r.db.list_email_suggestions(Some(status)).await.unwrap()
}

#[tokio::test]
async fn a_receipt_becomes_one_suggestion_and_is_handled_once() {
    let r = rig().await;
    let out = r
        .svc
        .ingest(&eml("m1", "Your receipt", "Total 12.50 receipt"))
        .await
        .unwrap();
    assert_eq!(
        out,
        Ingested {
            suggestions: 1,
            ..Default::default()
        }
    );
    let p = &rows(&r, "pending").await[0];
    assert_eq!(
        (
            p.amount_minor,
            p.currency.as_str(),
            p.date.as_str(),
            p.received.as_str()
        ),
        (-1250, "ILS", "2026-03-01", "2026-03-02")
    );
    assert_eq!(
        (p.sender.as_str(), p.subject.as_str(), p.message_id.as_str()),
        ("Shop <shop@example.com>", "Your receipt", "m1@example.com")
    );
    assert!(p.excerpt.contains("receipt") && p.category_id.is_some());
    // The same message again (a retry from the Worker, or a second forward): nothing happens.
    let again = r
        .svc
        .ingest(&eml("m1", "Your receipt", "Total 12.50 receipt"))
        .await
        .unwrap();
    assert!(again.duplicate);
    assert_eq!(r.w.lock().unwrap().ai_calls, 1);
    assert_eq!(r.db.list_email_suggestions(None).await.unwrap().len(), 1);
    let s = r.svc.status().await.unwrap();
    assert_eq!(
        (s.configured, s.ai_configured, s.pending, s.queued),
        (true, true, 1, 0)
    );
    assert!(s.last_received.is_some() && s.last_error.is_none());
}

#[tokio::test]
async fn a_message_without_a_transaction_is_kept_as_ignored_and_not_asked_twice() {
    let r = rig().await;
    let out = r
        .svc
        .ingest(&eml("n1", "Weekly deals", "50% off everything"))
        .await
        .unwrap();
    assert!(out.ignored && out.suggestions == 0);
    assert!(rows(&r, "pending").await.is_empty());
    assert!(
        r.svc
            .ingest(&eml("n1", "Weekly deals", "50% off everything"))
            .await
            .unwrap()
            .duplicate
    );
    assert_eq!(r.w.lock().unwrap().ai_calls, 1);
}

#[tokio::test]
async fn html_only_mail_is_read_as_text() {
    let r = rig().await;
    let raw = "From: a@b.example\r\nSubject: Invoice\r\nMessage-ID: <h1@x>\r\nDate: Mon, 02 Mar 2026 10:00:00 +0000\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><style>p{color:red}</style><body><p>Your <b>receipt</b>: 12.50&nbsp;ILS</p></body></html>\r\n";
    r.svc.ingest(raw.as_bytes()).await.unwrap();
    let prompt = r.w.lock().unwrap().prompts[0].clone();
    assert!(
        prompt.contains("receipt") && prompt.contains("12.50"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("<b>") && !prompt.contains("color:red"),
        "{prompt}"
    );
}

#[tokio::test]
async fn mail_without_a_message_id_is_still_recognised_the_second_time() {
    let r = rig().await;
    let raw = "From: a@b.example\r\nSubject: Receipt\r\nDate: Mon, 02 Mar 2026 10:00:00 +0000\r\n\r\nreceipt 12.50\r\n";
    assert_eq!(r.svc.ingest(raw.as_bytes()).await.unwrap().suggestions, 1);
    assert!(r.svc.ingest(raw.as_bytes()).await.unwrap().duplicate);
}

#[tokio::test]
async fn a_busy_ai_service_loses_nothing_and_is_retried() {
    let r = rig().await;
    r.w.lock().unwrap().ai_fail = vec![529];
    let out = r
        .svc
        .ingest(&eml("q1", "Receipt", "receipt 12.50"))
        .await
        .unwrap();
    assert!(out.queued);
    let q = rows(&r, "queued").await;
    assert_eq!(q.len(), 1);
    assert!(
        q[0].excerpt.contains("receipt 12.50"),
        "the text is kept for the retry"
    );
    assert!(q[0].error.contains("529"), "{}", q[0].error);
    assert!(r
        .svc
        .status()
        .await
        .unwrap()
        .last_error
        .unwrap()
        .contains("529"));
    // Still down on the retry: it stays queued and nothing is duplicated.
    r.w.lock().unwrap().ai_fail = vec![503];
    assert_eq!(r.svc.retry_queued(None).await.unwrap(), 0);
    assert_eq!(rows(&r, "queued").await.len(), 1);
    // Back up: the queued row is replaced by the suggestion.
    assert_eq!(r.svc.retry_queued(None).await.unwrap(), 1);
    assert!(rows(&r, "queued").await.is_empty());
    let p = rows(&r, "pending").await;
    assert_eq!((p.len(), p[0].amount_minor), (1, -1250));
    // And a second copy of the same email is still a duplicate.
    assert!(
        r.svc
            .ingest(&eml("q1", "Receipt", "receipt 12.50"))
            .await
            .unwrap()
            .duplicate
    );
    assert!(r.svc.status().await.unwrap().last_error.is_none());
}

#[tokio::test]
async fn an_unreadable_answer_becomes_a_failed_row_once_and_can_be_discarded() {
    let r = rig().await;
    let out = r.svc.ingest(&eml("w1", "weird", "weird")).await.unwrap();
    assert!(out.failed);
    let e = rows(&r, "error").await;
    assert_eq!(e.len(), 1);
    assert!(e[0].error.contains("not valid JSON"), "{}", e[0].error);
    assert!(
        r.svc
            .ingest(&eml("w1", "weird", "weird"))
            .await
            .unwrap()
            .duplicate
    );
    assert_eq!(r.w.lock().unwrap().ai_calls, 1);
    assert!(r.db.delete_email_suggestion(e[0].id).await.unwrap());
    assert!(rows(&r, "error").await.is_empty());
    // Decided rows cannot be removed this way.
    r.svc
        .ingest(&eml("m2", "Receipt", "receipt"))
        .await
        .unwrap();
    let id = rows(&r, "pending").await[0].id;
    assert!(!r.db.delete_email_suggestion(id).await.unwrap());
}

#[tokio::test]
async fn the_daily_limit_queues_the_rest_without_calling_the_ai() {
    let r = rig_with(|c| c.max_per_day = 1).await;
    assert_eq!(
        r.svc
            .ingest(&eml("a", "Receipt", "receipt 1"))
            .await
            .unwrap()
            .suggestions,
        1
    );
    let second = r
        .svc
        .ingest(&eml("b", "Receipt", "receipt 2"))
        .await
        .unwrap();
    assert!(second.queued);
    assert_eq!(r.w.lock().unwrap().ai_calls, 1);
    assert!(rows(&r, "queued").await[0].error.contains("daily limit"));
}

#[tokio::test]
async fn without_a_key_mail_waits_unread() {
    let r = rig_with(|c| c.ai_key = None).await;
    assert!(
        r.svc
            .ingest(&eml("a", "Receipt", "receipt"))
            .await
            .unwrap()
            .queued
    );
    assert_eq!(r.w.lock().unwrap().ai_calls, 0);
    assert!(!r.svc.status().await.unwrap().ai_configured);
}

#[tokio::test]
async fn something_that_is_not_an_email_is_refused() {
    let r = rig().await;
    assert!(matches!(
        r.svc.ingest(b"").await,
        Err(IngestError::Refused(_))
    ));
}

/// The real routes, over HTTP.
async fn serve(r: &Rig, svc: Arc<Service>) -> String {
    let app = crate::api::router_with(r.db.clone(), svc);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    base
}

#[tokio::test]
async fn the_inbound_endpoint_checks_the_shared_secret() {
    let r = rig().await;
    let app = serve(&r, r.svc.clone()).await;
    let http = reqwest::Client::new();
    let post = |auth: Option<&str>, body: Vec<u8>| {
        let mut req = http.post(format!("{app}/api/email/inbound")).body(body);
        if let Some(a) = auth {
            req = req.header("authorization", a);
        }
        async move { req.send().await.unwrap() }
    };
    let mail = eml("http1", "Receipt", "receipt 12.50");
    assert_eq!(post(None, mail.clone()).await.status(), 401);
    assert_eq!(post(Some("Bearer nope"), mail.clone()).await.status(), 401);
    assert_eq!(
        post(Some("s3cret"), mail.clone()).await.status(),
        401,
        "needs the Bearer prefix"
    );
    assert_eq!(
        r.w.lock().unwrap().ai_calls,
        0,
        "nothing is read without the secret"
    );
    let ok = post(Some("Bearer s3cret"), mail.clone()).await;
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.json::<Value>().await.unwrap()["suggestions"], 1);
    assert_eq!(
        post(Some("Bearer s3cret"), b"".to_vec()).await.status(),
        422
    );
    // The review queue is on the normal API.
    let list: Value = http
        .get(format!("{app}/api/email/suggestions?status=pending"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let id = list[0]["id"].as_i64().unwrap();
    assert_eq!(
        http.post(format!("{app}/api/email/suggestions/{id}/reject"))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        http.post(format!("{app}/api/email/suggestions/{id}/reject"))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let status: Value = http
        .get(format!("{app}/api/email/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["pending"], 0);
}

#[tokio::test]
async fn without_the_secret_setting_the_endpoint_does_not_exist() {
    let r = rig().await;
    let off = Service::new(r.db.clone(), None);
    let app = serve(&r, off).await;
    let resp = reqwest::Client::new()
        .post(format!("{app}/api/email/inbound"))
        .header("authorization", "Bearer s3cret")
        .body(eml("x", "Receipt", "receipt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let status: Value = reqwest::get(format!("{app}/api/email/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["configured"], false);
    let _ = &r.base;
}

#[tokio::test]
async fn deciding_removes_the_excerpt_and_cannot_be_redone() {
    let r = rig().await;
    r.svc
        .ingest(&eml("d1", "Receipt", "receipt 12.50"))
        .await
        .unwrap();
    let id = rows(&r, "pending").await[0].id;
    assert!(r
        .db
        .resolve_email_suggestion(id, "rejected", None)
        .await
        .unwrap());
    assert!(!r
        .db
        .resolve_email_suggestion(id, "accepted", Some(1))
        .await
        .unwrap());
    let all = r.db.list_email_suggestions(None).await.unwrap();
    assert_eq!(
        (all[0].status.as_str(), all[0].excerpt.as_str()),
        ("rejected", "")
    );
    assert!(
        r.svc
            .ingest(&eml("d1", "Receipt", "receipt 12.50"))
            .await
            .unwrap()
            .duplicate
    );
}

#[tokio::test]
async fn the_email_can_be_viewed_until_it_is_decided_and_never_with_active_content() {
    let r = rig().await;
    let raw = "From: a@b.example\r\nSubject: Invoice\r\nMessage-ID: <v1@x>\r\nDate: Mon, 02 Mar 2026 10:00:00 +0000\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Your <b>receipt</b></p><script>alert(1)</script><img src=\"https://tracker.example/p.gif\"></body></html>\r\n";
    r.svc.ingest(raw.as_bytes()).await.unwrap();
    let p = rows(&r, "pending").await;
    assert!(p[0].has_html);
    let app = serve(&r, r.svc.clone()).await;
    let resp = reqwest::get(format!("{app}/api/email/suggestions/{}/view", p[0].id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let csp = resp.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.contains("default-src 'none'")
            && csp.contains("sandbox")
            && !csp.contains("script-src"),
        "{csp}"
    );
    assert!(
        csp.contains("img-src data: https:"),
        "remote images are allowed: {csp}"
    );
    assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    assert!(resp.text().await.unwrap().contains("<b>receipt</b>"));
    // Decided: the HTML is gone.
    r.db.resolve_email_suggestion(p[0].id, "rejected", None)
        .await
        .unwrap();
    assert_eq!(
        reqwest::get(format!("{app}/api/email/suggestions/{}/view", p[0].id))
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn a_queued_message_keeps_its_html_through_the_retry() {
    let r = rig().await;
    r.w.lock().unwrap().ai_fail = vec![503];
    let raw = "From: a@b.example\r\nSubject: Invoice\r\nMessage-ID: <v2@x>\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>receipt <i>12.50</i></p>\r\n";
    r.svc.ingest(raw.as_bytes()).await.unwrap();
    let q = rows(&r, "queued").await;
    assert!(q[0].has_html);
    assert_eq!(r.svc.retry_queued(None).await.unwrap(), 1);
    let p = rows(&r, "pending").await;
    assert!(r
        .db
        .email_html(p[0].id)
        .await
        .unwrap()
        .unwrap()
        .contains("<i>12.50</i>"));
}

struct BillHandler;

#[async_trait::async_trait]
impl super::handlers::Handler for BillHandler {
    fn name(&self) -> &'static str {
        "bill"
    }
    fn matches(&self, mail: &super::inbound::Mail) -> bool {
        mail.from.contains("billing@")
    }
    async fn handle(
        &self,
        ctx: &super::handlers::Ctx,
        mail: &super::inbound::Mail,
    ) -> anyhow::Result<super::Ingested> {
        let mut row = ctx.suggestion(mail, 0);
        row.note = mail.links().first().cloned().unwrap_or_default();
        row.amount_minor = -4200;
        row.currency = "ILS".into();
        row.date = "2026-03-02".into();
        ctx.finish(mail, vec![row]).await
    }
}

#[tokio::test]
async fn a_manual_handler_takes_its_messages_before_the_ai() {
    let r = rig().await;
    let svc = Service::with_handlers(
        r.db.clone(),
        Some(config(&r.base)),
        vec![Box::new(BillHandler)],
    );
    let raw = "From: billing@utility.example\r\nSubject: Bill\r\nMessage-ID: <b1@x>\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Pay <a href=\"https://pay.example/x?a=1&amp;b=2\">here</a> <a href=\"mailto:a@b\">m</a></p>\r\n";
    let out = svc.ingest(raw.as_bytes()).await.unwrap();
    assert_eq!(out.suggestions, 1);
    assert!(!out.queued);
    assert_eq!(r.w.lock().unwrap().ai_calls, 0, "the AI was not asked");
    let p = rows(&r, "pending").await;
    assert_eq!(p[0].note, "https://pay.example/x?a=1&b=2");
    assert!(svc.ingest(raw.as_bytes()).await.unwrap().duplicate);
    // Other mail still goes to the AI.
    svc.ingest(&eml("o1", "Receipt", "Corner shop 12.50"))
        .await
        .unwrap();
    assert_eq!(r.w.lock().unwrap().ai_calls, 1);
}

#[tokio::test]
async fn each_ai_call_is_logged_with_its_cost_and_summed_for_a_period() {
    let r = rig().await;
    r.svc
        .ingest(&eml("c1", "Receipt", "Corner shop 12.50 receipt"))
        .await
        .unwrap();
    r.svc
        .ingest(&eml("c2", "Hello", "just chatting"))
        .await
        .unwrap();
    // A duplicate and a message the AI never saw cost nothing.
    r.svc
        .ingest(&eml("c1", "Receipt", "Corner shop 12.50 receipt"))
        .await
        .unwrap();
    let log = r.db.list_log(50, None).await.unwrap();
    let calls: Vec<_> = log.iter().filter(|e| e.kind == "ai_email").collect();
    assert_eq!(calls.len(), 2);
    let new = calls.iter().find_map(|e| e.new.clone()).unwrap();
    assert_eq!(new["input_tokens"], 1000);
    assert_eq!(new["output_tokens"], 150);
    // 1000 x $0.30 + 150 x $2.50 per million tokens = 675 millionths of a dollar.
    assert_eq!(new["cost_micro_usd"], 675);

    let app = serve(&r, r.svc.clone()).await;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let url = format!("{app}/api/email/cost?from={today}&to={today}");
    let body: Value = reqwest::get(url).await.unwrap().json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    assert_eq!(body["total_micro_usd"], 1350);
    let results: Vec<_> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["result"].as_str().unwrap().to_string())
        .collect();
    assert!(
        results.contains(&"1 suggestions".to_string()) && results.contains(&"ignored".to_string())
    );
    let url = format!("{app}/api/email/cost?from=2020-01-01&to=2020-01-31");
    let none: Value = reqwest::get(url).await.unwrap().json().await.unwrap();
    assert_eq!(none["total_micro_usd"], 0);
    let bad = reqwest::get(format!("{app}/api/email/cost?from=x&to=y"))
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

/// A mail with an attachment: `kind` is its content type, `name` its file name.
fn with_attachment(id: &str, body: &str, kind: &str, name: &str) -> Vec<u8> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.4 fake attachment bytes");
    format!(
        "From: Shop <shop@example.com>\r\nSubject: Your document\r\nMessage-ID: <{id}@example.com>\r\nDate: Mon, 02 Mar 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n--b1\r\nContent-Type: {kind}; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{data}\r\n--b1--\r\n"
    )
    .into_bytes()
}

#[tokio::test]
async fn a_pdf_attachment_is_read_with_the_body() {
    let r = rig().await;
    let raw = with_attachment(
        "p1",
        "Your invoice is attached.",
        "application/pdf",
        "invoice.pdf",
    );
    let out = r.svc.ingest(&raw).await.unwrap();
    assert_eq!(out.suggestions, 1);
    assert_eq!(r.w.lock().unwrap().pdf_parts, vec![1]);
    // A message with nothing but a PDF is read too.
    let raw = with_attachment("p2", "", "application/pdf", "bill.pdf");
    assert_eq!(r.svc.ingest(&raw).await.unwrap().suggestions, 1);
    assert_eq!(r.w.lock().unwrap().pdf_parts, vec![1, 1]);
}

#[tokio::test]
async fn an_attachment_the_ai_cannot_read_is_flagged_instead_of_dropped() {
    let r = rig().await;
    // Nothing to read in the body: no AI call, shown for a person to look at.
    let raw = with_attachment("i1", "", "image/jpeg", "scan.jpg");
    let out = r.svc.ingest(&raw).await.unwrap();
    assert!(out.failed);
    assert_eq!(r.w.lock().unwrap().ai_calls, 0);
    let e = rows(&r, "error").await;
    assert_eq!(e.len(), 1);
    assert!(e[0].error.contains("scan.jpg"), "{}", e[0].error);
    // A body the AI reads but finds nothing in, with such an attachment, is flagged too.
    let raw = with_attachment("i2", "see the scan", "image/png", "receipt.png");
    assert!(r.svc.ingest(&raw).await.unwrap().failed);
    assert_eq!(rows(&r, "error").await.len(), 2);
    // Without attachments, "nothing found" stays quiet.
    let out = r
        .svc
        .ingest(&eml("q1", "Hello", "just chatting"))
        .await
        .unwrap();
    assert!(out.ignored);
}

#[tokio::test]
async fn a_queued_message_keeps_its_pdf_for_the_retry() {
    let r = rig().await;
    r.w.lock().unwrap().ai_fail = vec![503];
    let raw = with_attachment(
        "p3",
        "Your invoice is attached.",
        "application/pdf",
        "invoice.pdf",
    );
    assert!(r.svc.ingest(&raw).await.unwrap().queued);
    let q = rows(&r, "queued").await;
    assert!(r.db.email_raw(q[0].id).await.unwrap().is_some());
    assert_eq!(r.svc.retry_queued(None).await.unwrap(), 1);
    // Second call (the retry) carried the PDF.
    assert_eq!(r.w.lock().unwrap().pdf_parts, vec![1, 1]);
    assert_eq!(rows(&r, "pending").await.len(), 1);
}

async fn tx_count(r: &Rig) -> usize {
    r.db.list_transactions().await.unwrap().len()
}

#[tokio::test]
async fn a_sure_answer_with_a_category_is_added_at_once() {
    let r = rig().await;
    let out = r
        .svc
        .ingest(&eml("a1", "Receipt", "Total 12.50 receipt sure"))
        .await
        .unwrap();
    assert_eq!((out.auto, out.suggestions), (1, 0));
    let a = &rows(&r, "auto").await[0];
    let t = r.db.list_transactions().await.unwrap();
    assert_eq!((t.len(), Some(t[0].id)), (1, a.transaction_id));
    assert_eq!(
        (t[0].amount_agorot, t[0].category_id),
        (-1250, a.category_id)
    );
    assert!(t[0].category_id.is_some());
    assert_eq!(rows(&r, "auto").await.len(), 1);
    assert_eq!(r.db.email_transaction_ids().await.unwrap(), vec![t[0].id]);
    let s = r.svc.status().await.unwrap();
    assert_eq!(
        s.latest,
        vec![Latest {
            auto: true,
            note: "Corner shop".into(),
            amount_minor: -1250,
            currency: "ILS".into()
        }]
    );
    // Not sure: waits for a person as before.
    let out = r
        .svc
        .ingest(&eml("a2", "Receipt 2", "Total 12.50 receipt"))
        .await
        .unwrap();
    assert_eq!((out.auto, out.suggestions, tx_count(&r).await), (0, 1, 1));
}

#[tokio::test]
async fn a_rule_fixes_the_category_and_business_and_the_model_is_not_asked_for_one() {
    let r = rig().await;
    let new = |name: &str, parent: Option<i64>| crate::storage::NewCategory {
        name: name.into(),
        is_income: false,
        parent_id: parent,
        linked_to: None,
    };
    let fun = r.db.create_category(&new("Fun", None)).await.unwrap();
    let gym = r.db.create_category(&new("Gym", Some(fun))).await.unwrap();
    let biz = r.db.insert_business("Studio", false).await.unwrap();
    rules::put(
        r.db.as_ref(),
        rules::Rule {
            subject: "Fwd: Gym receipt".into(),
            category_id: gym,
            business_id: Some(biz),
        },
    )
    .await
    .unwrap();
    // Even medium confidence is added: the rule names the category, and the amounts are the AI's.
    let out = r
        .svc
        .ingest(&eml("r1", "Gym receipt", "Total 12.50 receipt sure"))
        .await
        .unwrap();
    assert_eq!((out.auto, out.suggestions), (1, 0));
    let t = &r.db.list_transactions().await.unwrap()[0];
    assert_eq!((t.amount_agorot, t.category_id), (-1250, Some(gym)));
    let links = r.db.business_links().await.unwrap();
    assert_eq!(links.transactions, vec![(t.id, biz)]);
}

#[tokio::test]
async fn a_rule_with_an_unsure_answer_waits_with_the_category_chosen() {
    let r = rig().await;
    let cats = r.db.list_categories().await.unwrap();
    let groceries = cats.iter().find(|c| c.name == "Groceries").unwrap().id;
    rules::put(
        r.db.as_ref(),
        rules::Rule {
            subject: "Gym receipt".into(),
            category_id: groceries,
            business_id: None,
        },
    )
    .await
    .unwrap();
    let out = r
        .svc
        .ingest(&eml("r2", "Gym receipt", "Total 12.50 receipt"))
        .await
        .unwrap();
    assert_eq!((out.auto, out.suggestions, tx_count(&r).await), (0, 1, 0));
    assert_eq!(rows(&r, "pending").await[0].category_id, Some(groceries));
}

#[tokio::test]
async fn a_category_that_needs_a_business_waits_unless_a_rule_names_one() {
    let r = rig().await;
    let cats = r.db.list_categories().await.unwrap();
    let groceries = cats.iter().find(|c| c.name == "Groceries").unwrap().id;
    r.db.set_category_business_required(groceries, true)
        .await
        .unwrap();
    let out = r
        .svc
        .ingest(&eml("n1", "Receipt", "Total 12.50 receipt sure"))
        .await
        .unwrap();
    assert_eq!((out.auto, out.suggestions, tx_count(&r).await), (0, 1, 0));
}

#[tokio::test]
async fn an_auto_row_stays_a_month_then_closes_and_keeps_its_transaction() {
    let r = rig().await;
    r.svc
        .ingest(&eml("k1", "Receipt", "Total 12.50 receipt sure"))
        .await
        .unwrap();
    let a = rows(&r, "auto").await.remove(0);
    // Received this month: kept.
    assert_eq!(r.db.expire_auto_emails("2026-03-01").await.unwrap(), 0);
    assert_eq!(rows(&r, "auto").await.len(), 1);
    // Older than the cut-off (the status call applies it with today's date).
    r.svc.status().await.unwrap();
    assert!(rows(&r, "auto").await.is_empty());
    let done = rows(&r, "accepted").await.remove(0);
    assert_eq!(done.transaction_id, a.transaction_id);
    assert!(!done.has_html && done.transaction_id.is_some());
}

#[tokio::test]
async fn an_approved_suggestion_stays_listed_with_its_email_until_it_expires() {
    let r = rig().await;
    r.svc
        .ingest(&eml("p1", "Receipt", "Total 12.50 receipt"))
        .await
        .unwrap();
    let p = rows(&r, "pending").await.remove(0);
    let id = r.db.list_categories().await.unwrap()[0].id;
    let tx = crate::api::add_for_email(&r.db, "2026-03-01", -1250, "ILS", id + 1, "x")
        .await
        .unwrap_or_else(|_| panic!("add"));
    assert!(r
        .db
        .resolve_email_suggestion(p.id, "added", Some(tx))
        .await
        .unwrap());
    assert_eq!(rows(&r, "added").await.len(), 1);
    assert_eq!(r.db.email_transaction_ids().await.unwrap(), vec![tx]);
    r.svc.status().await.unwrap();
    assert!(rows(&r, "added").await.is_empty());
    assert_eq!(rows(&r, "accepted").await[0].transaction_id, Some(tx));
}
