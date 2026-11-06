//! End-to-end tests against a pretend Google (OAuth, Gmail) and a pretend AI service, both
//! served from one local server. They check what matters for safety: only the labelled mail is
//! read, nothing is processed twice, a failure loses nothing, and push requests are checked.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Value};

use super::push::{verify_oidc, Jwk, Jwks};
use super::*;

const LABEL: &str = "Label_7";

/// What the pretend servers know and what they were asked.
#[derive(Default)]
struct World {
    /// message id -> (label ids, subject, body)
    mail: HashMap<String, (Vec<String>, String, String)>,
    /// history answer: Some(ids) or None for 404
    history: Option<Vec<String>>,
    history_id: String,
    /// Fail the next N AI calls with this status.
    ai_fail: Vec<u16>,
    /// Every request path (with its relevant query) in order.
    log: Vec<String>,
    ai_calls: usize,
}

type W = Arc<Mutex<World>>;

fn b64(s: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
}

async fn token(
    State(w): State<W>,
    axum::Form(f): axum::Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    w.lock()
        .unwrap()
        .log
        .push(format!("token {}", f["grant_type"]));
    if f.get("refresh_token").is_some_and(|t| t == "revoked") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        );
    }
    let mut v = json!({ "access_token": "access-1", "expires_in": 3600 });
    if f["grant_type"] == "authorization_code" {
        v["refresh_token"] = json!("refresh-1");
    }
    (StatusCode::OK, Json(v))
}

async fn revoke(State(w): State<W>) -> StatusCode {
    w.lock().unwrap().log.push("revoke".into());
    StatusCode::OK
}

async fn profile(State(w): State<W>) -> Json<Value> {
    let h = w.lock().unwrap().history_id.clone();
    Json(json!({ "emailAddress": "me@example.com", "historyId": h }))
}

async fn labels() -> Json<Value> {
    Json(
        json!({ "labels": [{ "id": "INBOX", "name": "INBOX" }, { "id": LABEL, "name": "budget" }] }),
    )
}

async fn watch(State(w): State<W>, Json(b): Json<Value>) -> Json<Value> {
    w.lock().unwrap().log.push(format!(
        "watch {} {}",
        b["labelIds"], b["labelFilterBehavior"]
    ));
    Json(
        json!({ "historyId": "500", "expiration": (Utc::now().timestamp_millis() + 7 * 86_400_000).to_string() }),
    )
}

async fn stop(State(w): State<W>) -> StatusCode {
    w.lock().unwrap().log.push("stop".into());
    StatusCode::NO_CONTENT
}

async fn history(
    State(w): State<W>,
    Query(q): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let mut w = w.lock().unwrap();
    w.log.push(format!(
        "history label={}",
        q.get("labelId").cloned().unwrap_or_default()
    ));
    match &w.history {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": { "message": "too old" } })),
        ),
        Some(ids) => (
            StatusCode::OK,
            Json(
                json!({ "history": [{ "messagesAdded": ids.iter().map(|i| json!({ "message": { "id": i } })).collect::<Vec<_>>() }], "historyId": w.history_id }),
            ),
        ),
    }
}

async fn list_messages(
    State(w): State<W>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let mut w = w.lock().unwrap();
    w.log.push(format!(
        "list labelIds={}",
        q.get("labelIds").cloned().unwrap_or_default()
    ));
    let wanted = q.get("labelIds").cloned().unwrap_or_default();
    let mut ids: Vec<_> = w
        .mail
        .iter()
        .filter(|(_, m)| m.0.contains(&wanted))
        .map(|(i, _)| json!({ "id": i }))
        .collect();
    ids.sort_by_key(|v| v["id"].as_str().unwrap().to_string());
    Json(json!({ "messages": ids }))
}

async fn get_message(
    State(w): State<W>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let mut w = w.lock().unwrap();
    let format = q.get("format").cloned().unwrap_or_default();
    w.log.push(format!("get {id} {format}"));
    let Some((labels, subject, body)) = w.mail.get(&id).cloned() else {
        return (StatusCode::NOT_FOUND, Json(json!({})));
    };
    if format == "minimal" {
        return (
            StatusCode::OK,
            Json(json!({ "id": id, "labelIds": labels })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": id, "labelIds": labels, "internalDate": "1772409600000",
            "payload": { "mimeType": "multipart/alternative", "headers": [
                { "name": "From", "value": "Shop <shop@example.com>" }, { "name": "subject", "value": subject } ],
                "parts": [ { "mimeType": "text/plain", "body": { "data": b64(&body) } } ] }
        })),
    )
}

/// The pretend model: reports a 12.50 expense for a "receipt", nothing for anything else,
/// garbage for "weird".
async fn gemini(State(w): State<W>, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut w = w.lock().unwrap();
    w.ai_calls += 1;
    w.log.push("ai".into());
    if !w.ai_fail.is_empty() {
        let code = w.ai_fail.remove(0);
        return (
            StatusCode::from_u16(code).unwrap(),
            Json(json!({ "error": { "message": "busy" } })),
        );
    }
    let prompt = b["contents"][0]["parts"][0]["text"].as_str().unwrap_or("");
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
        .route("/token", post(token))
        .route("/revoke", post(revoke))
        .route("/gmail/v1/users/me/profile", get(profile))
        .route("/gmail/v1/users/me/labels", get(labels))
        .route("/gmail/v1/users/me/watch", post(watch))
        .route("/gmail/v1/users/me/stop", post(stop))
        .route("/gmail/v1/users/me/history", get(history))
        .route("/gmail/v1/users/me/messages", get(list_messages))
        .route("/gmail/v1/users/me/messages/:id", get(get_message))
        .route("/v1beta/models/test-model:generateContent", post(gemini))
        .with_state(w);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    base
}

fn config(base: &str) -> Config {
    Config {
        client_id: "cid".into(),
        client_secret: "secret".into(),
        redirect_uri: "https://budget.example.com/api/email/gmail/callback".into(),
        label: "Budget".into(),
        topic: Some("projects/p/topics/t".into()),
        push_token: Some("s3cret".into()),
        push_audience: None,
        push_service_account: None,
        ai_key: Some("test-key".into()),
        model: "test-model".into(),
        poll_minutes: 10,
        gmail_base: base.into(),
        oauth_base: base.into(),
        auth_url: "https://accounts.example.com/auth".into(),
        ai_base: base.into(),
        jwks_url: format!("{base}/certs"),
    }
}

struct Rig {
    svc: Arc<Service>,
    w: W,
    db: Db,
}

async fn rig() -> Rig {
    let w: W = Arc::new(Mutex::new(World {
        history_id: "100".into(),
        history: Some(vec![]),
        ..Default::default()
    }));
    let base = start(w.clone()).await;
    let db: Db = Arc::new(crate::testdb::fresh().await);
    // Two categories to match the AI's suggestion against.
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
    let svc = Service::new(db.clone(), Some(config(&base)));
    Rig { svc, w, db }
}

async fn connect(r: &Rig) {
    let url = r.svc.consent_url().await.unwrap();
    let state = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    r.svc.finish_connect("code-1", &state).await.unwrap();
}

fn add_mail(r: &Rig, id: &str, labels: &[&str], subject: &str, body: &str) {
    r.w.lock().unwrap().mail.insert(
        id.into(),
        (
            labels.iter().map(|s| s.to_string()).collect(),
            subject.into(),
            body.into(),
        ),
    );
}

fn count(r: &Rig, prefix: &str) -> usize {
    r.w.lock()
        .unwrap()
        .log
        .iter()
        .filter(|l| l.starts_with(prefix))
        .count()
}

#[tokio::test]
async fn consent_asks_for_read_only_and_checks_its_state() {
    let r = rig().await;
    let url = reqwest::Url::parse(&r.svc.consent_url().await.unwrap()).unwrap();
    let q: HashMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(q["scope"], "https://www.googleapis.com/auth/gmail.readonly");
    assert_eq!(q["access_type"], "offline");
    assert_eq!(q["prompt"], "consent");
    // A wrong state is refused, and the right one is then spent (one try each).
    assert!(r.svc.finish_connect("code", "wrong").await.is_err());
    assert!(r.svc.finish_connect("code", &q["state"]).await.is_err());
    assert!(r.svc.conn().await.unwrap().is_none());
}

#[tokio::test]
async fn connecting_watches_only_the_label_and_remembers_the_place() {
    let r = rig().await;
    connect(&r).await;
    let c = r.svc.conn().await.unwrap().unwrap();
    assert_eq!(
        (c.email.as_str(), c.label_id.as_str(), c.label_name.as_str()),
        ("me@example.com", LABEL, "budget")
    );
    assert_eq!(c.history_id.as_deref(), Some("500"));
    assert!(c.watch_expiration.is_some());
    assert!(r
        .w
        .lock()
        .unwrap()
        .log
        .contains(&format!("watch [\"{LABEL}\"] \"INCLUDE\"")));
    let s = r.svc.status().await.unwrap();
    assert!(s.connected && s.configured && s.ai_configured && s.push_enabled);
    assert_eq!(s.email, "me@example.com");
}

#[tokio::test]
async fn only_labelled_mail_is_read_and_nothing_is_done_twice() {
    let r = rig().await;
    connect(&r).await;
    add_mail(
        &r,
        "m1",
        &["INBOX", LABEL],
        "Your receipt",
        "Total 12.50 receipt",
    );
    add_mail(&r, "m2", &[LABEL], "Newsletter", "hello");
    // Handed to us by the history but without the label: its content must never be fetched.
    add_mail(&r, "m3", &["INBOX"], "Private", "secret receipt");
    r.w.lock().unwrap().history = Some(vec!["m1".into(), "m2".into(), "m3".into()]);
    r.w.lock().unwrap().history_id = "600".into();

    let s = r.svc.sync().await.unwrap();
    assert_eq!(
        (s.messages, s.suggestions, s.ignored, s.failed),
        (2, 1, 1, 0)
    );
    assert_eq!(
        count(&r, "get m3 full"),
        0,
        "an unlabelled message must not be read"
    );
    assert_eq!(count(&r, "get m3 minimal"), 1);
    assert_eq!(r.w.lock().unwrap().ai_calls, 2);
    assert!(r
        .w
        .lock()
        .unwrap()
        .log
        .contains(&format!("history label={LABEL}")));

    let pending = r.db.list_email_suggestions(Some("pending")).await.unwrap();
    assert_eq!(pending.len(), 1);
    let p = &pending[0];
    assert_eq!(
        (
            p.gmail_id.as_str(),
            p.amount_minor,
            p.currency.as_str(),
            p.date.as_str()
        ),
        ("m1", -1250, "ILS", "2026-03-01")
    );
    assert_eq!(p.sender, "Shop <shop@example.com>");
    assert_eq!(p.subject, "Your receipt");
    assert_eq!(p.received, "2026-03-02");
    assert!(p.excerpt.contains("receipt"));
    assert_eq!(
        r.svc.conn().await.unwrap().unwrap().history_id.as_deref(),
        Some("600")
    );

    // The same history again, and a scan of the label: no new work, no new rows.
    let again = r.svc.sync().await.unwrap();
    assert_eq!(again, SyncSummary::default());
    let scanned = r.svc.scan(Some(7)).await.unwrap();
    assert_eq!(scanned, SyncSummary::default());
    assert_eq!(r.w.lock().unwrap().ai_calls, 2);
    assert_eq!(r.db.list_email_suggestions(None).await.unwrap().len(), 2);
    // Every list call carried the label.
    assert!(r
        .w
        .lock()
        .unwrap()
        .log
        .iter()
        .filter(|l| l.starts_with("list"))
        .all(|l| l.ends_with(LABEL)));
}

#[tokio::test]
async fn deciding_removes_the_excerpt_and_cannot_be_redone() {
    let r = rig().await;
    connect(&r).await;
    add_mail(&r, "m1", &[LABEL], "receipt", "receipt 12.50");
    r.svc.scan(None).await.unwrap();
    let id = r.db.list_email_suggestions(Some("pending")).await.unwrap()[0].id;
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
    // Scanning again does not bring it back.
    r.svc.scan(None).await.unwrap();
    assert!(r
        .db
        .list_email_suggestions(Some("pending"))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_busy_ai_service_loses_nothing() {
    let r = rig().await;
    connect(&r).await;
    add_mail(&r, "m1", &[LABEL], "receipt", "receipt 12.50");
    {
        let mut w = r.w.lock().unwrap();
        w.history = Some(vec!["m1".into()]);
        w.history_id = "700".into();
        w.ai_fail = vec![529];
    }
    let e = r.svc.sync().await.unwrap_err();
    assert!(format!("{e:#}").contains("tried again"), "{e:#}");
    assert!(r.db.list_email_suggestions(None).await.unwrap().is_empty());
    // The history position stays, and the problem is shown in the status.
    assert_eq!(
        r.svc.conn().await.unwrap().unwrap().history_id.as_deref(),
        Some("500")
    );
    assert!(r
        .svc
        .status()
        .await
        .unwrap()
        .last_error
        .unwrap()
        .contains("tried again"));
    // Next time it works, and the message is handled once.
    let s = r.svc.sync().await.unwrap();
    assert_eq!(s.suggestions, 1);
    assert_eq!(
        r.svc.conn().await.unwrap().unwrap().history_id.as_deref(),
        Some("700")
    );
    assert!(r.svc.status().await.unwrap().last_error.is_none());
}

#[tokio::test]
async fn an_unreadable_answer_becomes_a_failed_row_once() {
    let r = rig().await;
    connect(&r).await;
    add_mail(&r, "m1", &[LABEL], "weird", "weird");
    let s = r.svc.scan(None).await.unwrap();
    assert_eq!(s.failed, 1);
    let rows = r.db.list_email_suggestions(Some("error")).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].error.contains("not valid JSON"),
        "{}",
        rows[0].error
    );
    r.svc.scan(None).await.unwrap();
    assert_eq!(r.w.lock().unwrap().ai_calls, 1);
}

#[tokio::test]
async fn lost_history_falls_back_to_a_scan_of_the_label() {
    let r = rig().await;
    connect(&r).await;
    add_mail(&r, "m1", &[LABEL], "receipt", "receipt");
    add_mail(&r, "m9", &["INBOX"], "x", "receipt");
    {
        let mut w = r.w.lock().unwrap();
        w.history = None;
        w.history_id = "900".into();
    }
    let s = r.svc.sync().await.unwrap();
    assert_eq!(s.suggestions, 1);
    assert_eq!(
        r.svc.conn().await.unwrap().unwrap().history_id.as_deref(),
        Some("900")
    );
    assert_eq!(count(&r, "get m9 full"), 0);
}

#[tokio::test]
async fn a_revoked_sign_in_is_reported_and_disconnect_forgets_it() {
    let r = rig().await;
    connect(&r).await;
    let mut c = r.svc.conn().await.unwrap().unwrap();
    c.refresh_token = "revoked".into();
    r.svc.save(&c).await.unwrap();
    *r.svc.access.lock().unwrap() = None;
    let e = r.svc.sync().await.unwrap_err();
    assert!(format!("{e:#}").contains("connect Gmail again"), "{e:#}");
    assert!(r
        .svc
        .status()
        .await
        .unwrap()
        .last_error
        .unwrap()
        .contains("connect Gmail again"));
    r.svc.disconnect().await.unwrap();
    assert!(r.svc.conn().await.unwrap().is_none());
    assert!(!r.svc.status().await.unwrap().connected);
}

#[tokio::test]
async fn disconnect_stops_the_watch_and_revokes_the_token() {
    let r = rig().await;
    connect(&r).await;
    r.svc.disconnect().await.unwrap();
    assert_eq!((count(&r, "stop"), count(&r, "revoke")), (1, 1));
}

#[tokio::test]
async fn watch_is_renewed_only_when_it_runs_out() {
    let r = rig().await;
    connect(&r).await;
    assert!(!r.svc.renew_watch(48).await.unwrap());
    let mut c = r.svc.conn().await.unwrap().unwrap();
    c.watch_expiration = Some(Utc::now().timestamp_millis() + 3_600_000);
    r.svc.save(&c).await.unwrap();
    assert!(r.svc.renew_watch(48).await.unwrap());
    assert_eq!(count(&r, "watch"), 2);
}

#[tokio::test]
async fn push_needs_the_url_token_and_is_off_without_any_setting() {
    let r = rig().await;
    assert_eq!(r.svc.authorize_push(Some("s3cret"), None).await, Ok(()));
    assert_eq!(
        r.svc.authorize_push(Some("nope"), None).await,
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(
        r.svc.authorize_push(None, None).await,
        Err(StatusCode::UNAUTHORIZED)
    );
    let mut cfg = config("http://unused");
    cfg.push_token = None;
    let off = Service::new(r.db.clone(), Some(cfg));
    assert_eq!(
        off.authorize_push(Some("anything"), None).await,
        Err(StatusCode::NOT_FOUND)
    );
    let none = Service::new(r.db.clone(), None);
    assert_eq!(
        none.authorize_push(None, None).await,
        Err(StatusCode::NOT_FOUND)
    );
}

#[tokio::test]
async fn push_also_checks_the_google_token_when_an_audience_is_set() {
    let r = rig().await;
    let mut cfg = config("http://unused");
    cfg.push_audience = Some(AUD.into());
    cfg.push_service_account = Some(SA.into());
    // The pretend key server.
    let certs = Json(json!({ "keys": [{ "kid": "k1", "n": N, "e": "AQAB" }] }));
    let app = Router::new().route(
        "/certs",
        get(move || {
            let c = certs.clone();
            async move { c }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    cfg.jwks_url = format!("http://{}/certs", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let svc = Service::new(r.db.clone(), Some(cfg));
    let good = sign(&claims(AUD, SA, 3600));
    let bearer = format!("Bearer {good}");
    assert_eq!(
        svc.authorize_push(Some("s3cret"), Some(&bearer)).await,
        Ok(())
    );
    assert_eq!(
        svc.authorize_push(Some("s3cret"), None).await,
        Err(StatusCode::UNAUTHORIZED)
    );
    let other = format!("Bearer {}", sign(&claims("https://evil.example", SA, 3600)));
    assert_eq!(
        svc.authorize_push(Some("s3cret"), Some(&other)).await,
        Err(StatusCode::UNAUTHORIZED)
    );
}

const KEY_BODY: &str = "MIIEuwIBADANBgkqhkiG9w0BAQEFAASCBKUwggShAgEAAoIBAQDN1mrY233/R5lt083tIZetm8IVY9SYpfVNSnf6Kl0hc5TsvteJj0UIBu9Nd9G6FWfsLLQq3kZ6iIf7kk+RfByqgxeiwPlWk9N+JozniWUSkn6LXllRiq+CusxhmYKIk7Ml+7jNyf99LR1gdU/X5Rb3/+4PV8Y/xcyUcfQq4WHC56MoA5Xl84st88gI4pBddUaL9A21eSLdtrgB8u0yFUe7/6LHeOFIdw0uIs99fd09X3FdLAECF5ryw8LQcQETyoTM0AV4cXzntyYonWrSqkYAaYOdOzd6l1+ekAha5tVBOTcV+wb13tMiDKyVJIt1KN5WRdEG0NZ3TMOspprhuJFpAgMBAAECgf8hKquMxdFHkQHn8Hj9/Ku0DaT4TV20OwmLqrQqWykmerlEbHvObodxA60LjoL8uPZF1+4v+iDhHZ7LqoenqZf04aqtTureSHOLoCqUJE9k7rX1Pv6lZPNi3TaRg2rFFweG10iv4ByKzvos0LpMz10kGNkDH+GR5Q1R3gq53hcikGZzxcBoJhgRJtogCZ1+HQvxErbamtwgshLWUOQH72jnNdrCd/P28a7BzUsX/Sd272sH9wf2g2X7OLw45YUib84CmypCpuA3h8X9RVhBaZijZ8ow00xJdFnuVZRG4bjytel9ISvTVIOunpPk9gSGpxiTtIMTxLj9ahMH05+Y52UCgYEA7djZWqVGf5deGUjgIZgciV2YYOWb5kyWp+4UJVt+9O0BorTGk7AUlO786bG/p1gl/qWTHiX4r43A5N3RXPsu6/RO4EetUemNOJLLgtIg7l+LWvRJRVJUaLSAFCdvYoPdRofEWtGcWqhi4B+dsyzyOfXOU99GniwCAWFuR0JZKHUCgYEA3Ywnajyjux8k7KTEbpVhzefL97zRDJPiTe9RutMDTA3b0ba0FifDTRuPFophIgOzMcMu/zwKDxzlmHpWxHwsQ9eBm+8v7b8iOwnoqDnlzTp1Ghl9pvzNvjgJrz6R0/1jA3zjcwOram+GDYl0xb6fY1bXIWoQ5HXiaKXShb47xqUCgYAEsF24l8GICmfkf7dLckA9xQaYAm7MX34o2Vy0T9V05qc9merARpxhQ0ThJovstKQrHQCFe6Inc9f1qxnRh3IpXKuhaEhWcRBDqUEx1wqnKGMK6/31y+W2GrjbrNtZSdg2575LPA6lCuTIHldxjNVJpzrmFGTEAYIYnN8bsIcpcQKBgFzPvSddVf4rEu3sLLpi7EE1TRZwzGbngRPDIUsR4Ty4RZF6Z8EsUT35QJ1hxUVZpXVAXmzgexpPscOJ5NWVa3nUS2DG+B4yRyXShdwLvUCXr70KVsBhAdQhZ7U5kh/r89HfPO3O6L3DELaVEg1RSTZ1h9Rgucjrtbi/Vw1eX25dAoGBAN4yYvkJjyci4vIUPvEqvEhtQ0fGSl0Zo3Tr5OxmarvgcnEK2OBczmfFKCi40Em5390A1zg1U0ZoXy2K6/Jn92zP/yRwb9nAt2Z2+q/ioF8CuOysLojHEgiO1sSMeGYf2Kx36VT25yqyUtkfy4eLorlZ7uF8AsW2X4O5H+mXQLUO";
const N: &str = "zdZq2Nt9_0eZbdPN7SGXrZvCFWPUmKX1TUp3-ipdIXOU7L7XiY9FCAbvTXfRuhVn7Cy0Kt5GeoiH-5JPkXwcqoMXosD5VpPTfiaM54llEpJ-i15ZUYqvgrrMYZmCiJOzJfu4zcn_fS0dYHVP1-UW9__uD1fGP8XMlHH0KuFhwuejKAOV5fOLLfPICOKQXXVGi_QNtXki3ba4AfLtMhVHu_-ix3jhSHcNLiLPfX3dPV9xXSwBAhea8sPC0HEBE8qEzNAFeHF857cmKJ1q0qpGAGmDnTs3epdfnpAIWubVQTk3FfsG9d7TIgyslSSLdSjeVkXRBtDWd0zDrKaa4biRaQ";
const AUD: &str = "https://budget.example.com/api/email/push";
const SA: &str = "pusher@proj.iam.gserviceaccount.com";

fn claims(aud: &str, email: &str, ttl: i64) -> Value {
    json!({ "iss": "https://accounts.google.com", "aud": aud, "email": email, "email_verified": true, "exp": Utc::now().timestamp() + ttl })
}

fn sign(c: &Value) -> String {
    let pem = format!(
        "-----BEGIN {0}-----\n{1}\n-----END {0}-----\n",
        "PRIVATE KEY", KEY_BODY
    );
    let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    h.kid = Some("k1".into());
    jsonwebtoken::encode(
        &h,
        c,
        &jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap()
}

#[test]
fn oidc_tokens_must_match_audience_account_and_time() {
    let jwks = Jwks {
        keys: vec![Jwk {
            kid: "k1".into(),
            n: N.into(),
            e: "AQAB".into(),
        }],
    };
    let ok = |t: &str, aud: &str, sa: Option<&str>| verify_oidc(t, &jwks, aud, sa).is_ok();
    let good = sign(&claims(AUD, SA, 3600));
    assert!(ok(&good, AUD, Some(SA)));
    assert!(ok(&good, AUD, None));
    assert!(
        !ok(&good, "https://other.example", Some(SA)),
        "wrong audience"
    );
    assert!(!ok(&good, AUD, Some("someone@else.com")), "wrong account");
    assert!(
        !ok(&sign(&claims(AUD, SA, -3600)), AUD, Some(SA)),
        "expired"
    );
    assert!(!ok("not.a.jwt", AUD, Some(SA)));
    let mut unverified = claims(AUD, SA, 3600);
    unverified["email_verified"] = json!(false);
    assert!(!ok(&sign(&unverified), AUD, Some(SA)));
    // Signed by some other key: the signature does not fit the published one.
    let other = Jwks {
        keys: vec![Jwk {
            kid: "k1".into(),
            n: format!("{}A{}", &N[..50], &N[51..]),
            e: "AQAB".into(),
        }],
    };
    assert!(verify_oidc(&good, &other, AUD, Some(SA)).is_err());
}

#[test]
fn html_mail_is_reduced_to_text() {
    let html = "<html><style>p{color:red}</style><body><p>Paid&nbsp;<b>12.50</b> &amp; thanks</p><script>x()</script><br>Bye</body></html>";
    let t = gmail::strip_html(html)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(t.contains("Paid 12.50") && t.contains("& thanks") && t.contains("Bye"));
    assert!(!t.contains("color") && !t.contains("x()"));
}
