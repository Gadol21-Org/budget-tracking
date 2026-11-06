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
                json!({ "candidates": [{ "content": { "parts": [{ "text": text }] }, "finishReason": "STOP" }] }),
            ),
        )
    };
    if prompt.contains("weird") {
        return reply("not json at all".into());
    }
    let txs = if prompt.contains("receipt") {
        json!([{ "direction": "expense", "amount": 12.5, "currency": "ILS", "date": "2026-03-01", "note": "Corner shop", "category": "Food > Groceries", "confidence": "high" }])
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
        forwarder: None,
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
async fn the_forwarder_check_needs_the_gmail_header() {
    let r = rig_with(|c| c.forwarder = Some("me@gmail.example".into())).await;
    assert!(matches!(
        r.svc.ingest(&eml("a", "Receipt", "receipt")).await,
        Err(IngestError::Refused(_))
    ));
    let raw = "X-Forwarded-For: Me@Gmail.example budget@example.com\r\nFrom: a@b.example\r\nSubject: Receipt\r\nMessage-ID: <f1@x>\r\n\r\nreceipt\r\n";
    assert_eq!(r.svc.ingest(raw.as_bytes()).await.unwrap().suggestions, 1);
    // The header in the body of a message does not count.
    let in_body = "From: a@b.example\r\nSubject: Receipt\r\nMessage-ID: <f2@x>\r\n\r\nX-Forwarded-For: me@gmail.example\r\n";
    assert!(matches!(
        r.svc.ingest(in_body.as_bytes()).await,
        Err(IngestError::Refused(_))
    ));
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
