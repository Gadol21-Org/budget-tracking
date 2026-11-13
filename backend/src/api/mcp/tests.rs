use super::*;
use crate::oplog::Logged;
use crate::storage::NewTransaction;
use crate::storage::Storage;

async fn db() -> Db {
    Arc::new(Logged(Arc::new(crate::testdb::fresh().await)))
}

fn tx(date: &str, agorot: i64, cat: Option<i64>) -> NewTransaction {
    NewTransaction {
        date: date.into(),
        amount_agorot: agorot,
        category_id: cat,
        payment_method_id: None,
        note: format!("n{date}"),
    }
}

/// Call a tool the way a client does; `Ok` is the structured result, `Err` the error text.
async fn call(db: &Db, tool: &str, args: Value) -> Result<Value, String> {
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": args}});
    let res = process(db, req.to_string().as_bytes()).await.unwrap();
    let r = &res["result"];
    if r["isError"] == true {
        Err(r["content"][0]["text"].as_str().unwrap().to_string())
    } else {
        Ok(r["structuredContent"].clone())
    }
}

async fn cats(db: &Db) -> (i64, i64) {
    let food = db.insert_category("Food", false, None).await.unwrap();
    let pay = db.insert_category("Salary", true, None).await.unwrap();
    (food, pay)
}

fn ids(page: &Value) -> Vec<i64> {
    page["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn protocol_basics() {
    let db = db().await;
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-03-26"}});
    let r = process(&db, init.to_string().as_bytes()).await.unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    let list = process(&db, br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
        .await
        .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"list_transactions") && names.contains(&"list_operations_log"));
    // A notification gets no answer; junk and unknown methods get JSON-RPC errors.
    assert!(process(
        &db,
        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
    )
    .await
    .is_none());
    assert_eq!(process(&db, b"{").await.unwrap()["error"]["code"], -32700);
    let r = process(&db, br#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#)
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], -32601);
}

#[tokio::test]
async fn endpoint_needs_the_token() {
    let db = db().await;
    let send = |token: Option<&str>, auth: Option<&str>, method: Method| {
        let db = db.clone();
        let cfg = Arc::new(Config::with_token(token.map(Into::into)));
        let mut h = HeaderMap::new();
        if let Some(a) = auth {
            h.insert(header::AUTHORIZATION, a.parse().unwrap());
        }
        async move {
            handle(
                Extension(cfg),
                State(db),
                method,
                h,
                Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
            )
            .await
            .status()
        }
    };
    let good = "x".repeat(40);
    let bearer = format!("Bearer {good}");
    // Off without a token, and with one that is too short to be safe.
    assert_eq!(send(None, Some(&bearer), Method::POST).await, 404);
    assert_eq!(
        send(Some("short"), Some("Bearer short"), Method::POST).await,
        404
    );
    assert_eq!(send(Some(&good), None, Method::POST).await, 401);
    assert_eq!(
        send(Some(&good), Some("Bearer nope"), Method::POST).await,
        401
    );
    assert_eq!(send(Some(&good), Some(&good), Method::POST).await, 401);
    assert_eq!(send(Some(&good), Some(&bearer), Method::POST).await, 200);
    assert_eq!(send(Some(&good), Some(&bearer), Method::GET).await, 405);
}

#[tokio::test]
async fn pagination_is_required_and_capped() {
    let db = db().await;
    let e = call(&db, "list_transactions", json!({})).await.unwrap_err();
    assert!(e.contains("limit is required"), "{e}");
    for bad in [json!(0), json!(101), json!(-5), json!("10")] {
        assert!(call(&db, "list_transactions", json!({"limit": bad}))
            .await
            .is_err());
        assert!(call(&db, "list_operations_log", json!({"limit": bad}))
            .await
            .is_err());
    }
    assert!(call(&db, "list_operations_log", json!({})).await.is_err());
    // A misspelt argument is an error, not silently ignored (no way to dodge the limit).
    let e = call(&db, "list_transactions", json!({"limit": 5, "page": 2}))
        .await
        .unwrap_err();
    assert!(e.contains("unknown argument"), "{e}");
    assert!(call(
        &db,
        "list_transactions",
        json!({"limit": 5, "cursor": "junk!"})
    )
    .await
    .is_err());
    assert!(call(&db, "list_transactions", json!({"limit": 100}))
        .await
        .is_ok());
}

#[tokio::test]
async fn transactions_page_through_with_filters() {
    let db = db().await;
    let (food, pay) = cats(&db).await;
    for d in 1..=9 {
        // Two on day 5 so a page can end between two lines of one day.
        let date = format!("2026-03-0{d}");
        db.create_transaction(&tx(&date, -1000 * d, Some(food)))
            .await
            .unwrap();
        if d == 5 {
            db.create_transaction(&tx(&date, 500, Some(pay)))
                .await
                .unwrap();
        }
    }
    db.create_transaction(&tx("2026-03-10", 7000, Some(pay)))
        .await
        .unwrap();
    db.create_transaction(&tx("2026-03-11", -300, None))
        .await
        .unwrap(); // by sign: expense

    // Walk every page of 3 and meet each transaction exactly once, newest first.
    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut args = json!({"limit": 3});
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let p = call(&db, "list_transactions", args).await.unwrap();
        assert_eq!(p["total_matching"], 12);
        assert!(p["returned"].as_u64().unwrap() <= 3);
        seen.extend(ids(&p));
        cursor = p["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(seen.len(), 12);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 12);
    let all = db.list_transactions().await.unwrap();
    assert_eq!(seen, all.iter().map(|t| t.id).collect::<Vec<_>>());

    // A transaction created between two pages cannot shift the next page.
    let p1 = call(&db, "list_transactions", json!({"limit": 4}))
        .await
        .unwrap();
    db.create_transaction(&tx("2026-03-12", -1, None))
        .await
        .unwrap();
    let p2 = call(
        &db,
        "list_transactions",
        json!({"limit": 4, "cursor": p1["next_cursor"]}),
    )
    .await
    .unwrap();
    assert_eq!(ids(&p2), seen[4..8].to_vec());

    let income = call(
        &db,
        "list_transactions",
        json!({"limit": 50, "type": "income"}),
    )
    .await
    .unwrap();
    assert_eq!(income["total_matching"], 2);
    let expense = call(
        &db,
        "list_transactions",
        json!({"limit": 50, "type": "expense"}),
    )
    .await
    .unwrap();
    assert_eq!(expense["total_matching"], 11); // 9 + the uncategorised one + the one just added
    let range = call(
        &db,
        "list_transactions",
        json!({"limit": 50, "from": "2026-03-05", "to": "2026-03-07"}),
    )
    .await
    .unwrap();
    assert_eq!(range["total_matching"], 4); // two on the 5th, one each on the 6th and 7th
    assert!(call(
        &db,
        "list_transactions",
        json!({"limit": 5, "from": "2026-03-09", "to": "2026-03-01"})
    )
    .await
    .is_err());
    assert!(call(
        &db,
        "list_transactions",
        json!({"limit": 5, "from": "yesterday"})
    )
    .await
    .is_err());
    assert!(call(
        &db,
        "list_transactions",
        json!({"limit": 5, "type": "both"})
    )
    .await
    .is_err());
}

#[tokio::test]
async fn a_transaction_carries_all_its_metadata_and_source() {
    let db = db().await;
    let (food, _) = cats(&db).await;
    let mk = |who: Option<&str>, t: NewTransaction| {
        let db = db.clone();
        let who = who.map(String::from);
        async move {
            oplog::ACTOR
                .scope(who, async { db.create_transaction(&t).await.unwrap() })
                .await
        }
    };
    let by_person = mk(Some("me@x.com"), tx("2026-04-01", -100, Some(food))).await;
    let by_sync = oplog::as_sync(async {
        db.create_transaction(&tx("2026-04-02", 2500, None))
            .await
            .unwrap()
    })
    .await;
    let anon = mk(None, tx("2026-04-03", -5, None)).await;
    let via_mcp = call(
        &db,
        "create_transaction",
        json!({"date": "2026-04-04", "amount_agorot": -4250, "category_id": food, "note": "lunch"}),
    )
    .await
    .unwrap();
    assert_eq!(via_mcp["transaction"]["source"]["type"], "mcp");
    assert_eq!(via_mcp["transaction"]["created_by"], "mcp");
    assert_eq!(via_mcp["transaction"]["amount"], "-42.50");
    assert_eq!(via_mcp["transaction"]["category"]["path"], "Food");

    // An accepted email and a Sumit receipt point at their transactions.
    let mailed = mk(Some("me@x.com"), tx("2026-04-05", -900, Some(food))).await;
    db.add_email_suggestions(&[crate::storage::NewEmailSuggestion {
        message_id: "m1".into(),
        status: "pending".into(),
        received: "2026-04-05".into(),
        sender: "shop@x.com".into(),
        subject: "Receipt".into(),
        date: "2026-04-05".into(),
        amount_minor: -900,
        currency: "ILS".into(),
        ..Default::default()
    }])
    .await
    .unwrap();
    let row = db
        .list_email_suggestions(Some("pending"))
        .await
        .unwrap()
        .remove(0);
    db.resolve_email_suggestion(row.id, "accepted", Some(mailed))
        .await
        .unwrap();
    let receipt = mk(Some("me@x.com"), tx("2026-04-06", 100, None)).await;
    let rec = ReceiptRecord {
        doc_id: 77,
        number: Some(1234),
        date: "2026-04-06".into(),
        customer: "Dana".into(),
        total_agorot: 100,
        items: vec![],
        tx_ids: vec![receipt],
        sheet_sent: false,
    };
    db.set_setting(
        crate::sumit::RECEIPTS_KEY,
        &serde_json::to_string(&vec![rec]).unwrap(),
    )
    .await
    .unwrap();

    let get = |id: i64| {
        let db = db.clone();
        async move {
            call(&db, "get_transaction", json!({"id": id}))
                .await
                .unwrap()["transaction"]
                .clone()
        }
    };
    let t = get(by_person).await;
    assert_eq!(t["source"]["type"], "manual");
    assert_eq!(t["source"]["details"]["created_by"], "me@x.com");
    assert_eq!(t["created_by"], "me@x.com");
    assert_eq!(t["paamonim"]["sync_state"], "pending");
    assert_eq!(t["type"], "expense");
    assert!(t["created_at"].is_string());
    assert_eq!(get(by_sync).await["source"]["type"], "sumit");
    assert_eq!(get(anon).await["source"]["type"], "manual");
    let m = get(mailed).await;
    assert_eq!(m["source"]["type"], "email");
    assert_eq!(m["source"]["details"]["sender"], "shop@x.com");
    assert_eq!(m["source"]["details"]["subject"], "Receipt");
    let r = get(receipt).await;
    assert_eq!(r["source"]["type"], "sumit");
    assert_eq!(r["source"]["details"]["document_id"], 77);

    // Every documented field is present, even when empty.
    for k in [
        "id",
        "date",
        "amount_agorot",
        "amount",
        "type",
        "category",
        "payment_method",
        "note",
        "foreign_currency",
        "business",
        "source",
        "created_at",
        "created_by",
        "last_updated_at",
        "last_updated_by",
        "recurring_series_id",
        "paamonim",
    ] {
        assert!(t.get(k).is_some(), "missing {k}");
    }
    assert!(call(&db, "get_transaction", json!({"id": 999999}))
        .await
        .is_err());
}

#[tokio::test]
async fn a_row_with_no_trace_says_unknown_and_imports_are_recognised() {
    let db = db().await;
    // Straight into the unwrapped storage: no change-log entry, like a row from before the log
    // existed or older than its one-year retention.
    let raw = crate::testdb::fresh().await;
    let id = raw
        .create_transaction(&tx("2020-01-01", -1, None))
        .await
        .unwrap();
    let raw: Db = Arc::new(raw);
    let t = call(&raw, "get_transaction", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(t["transaction"]["source"]["type"], "unknown");
    assert!(t["transaction"]["created_at"].is_null());

    let batch = crate::storage::ImportBatch {
        transactions: vec![crate::storage::ImportTransaction {
            line_id: "L1".into(),
            date: "2026-01-05".into(),
            amount_agorot: -1500,
            category_key: None,
            payment_key: None,
            note: "from paamonim".into(),
        }],
        ..Default::default()
    };
    db.import_paamonim(&batch).await.unwrap();
    let p = call(&db, "list_transactions", json!({"limit": 10}))
        .await
        .unwrap();
    let t = &p["transactions"][0];
    assert_eq!(t["source"]["type"], "paamonim_import");
    assert_eq!(t["paamonim"]["line_id"], "L1");
    assert_eq!(t["paamonim"]["sync_state"], "synced");
}

#[tokio::test]
async fn writes_are_validated_applied_and_logged_as_mcp() {
    let db = db().await;
    let (food, pay) = cats(&db).await;
    // The same checks as the app's own dialog, plus unknown ids.
    assert!(
        call(&db, "create_transaction", json!({"date": "2026-05-01"}))
            .await
            .is_err()
    );
    assert!(call(
        &db,
        "create_transaction",
        json!({"date": "2026-05-01", "amount_agorot": 0})
    )
    .await
    .is_err());
    assert!(call(
        &db,
        "create_transaction",
        json!({"date": "May", "amount_agorot": 5})
    )
    .await
    .is_err());
    assert!(call(
        &db,
        "create_transaction",
        json!({"date": "2026-05-01", "amount_agorot": 5, "category_id": 9999})
    )
    .await
    .is_err());
    assert!(call(
        &db,
        "create_transaction",
        json!({"date": "2026-05-01", "amount_agorot": 5, "payment_method_id": 9999})
    )
    .await
    .is_err());
    assert!(call(
        &db,
        "create_transaction",
        json!({"date": "2026-05-01", "amount_agorot": 5, "bogus": 1})
    )
    .await
    .is_err());
    assert!(db.list_transactions().await.unwrap().is_empty());

    let created = call(
        &db,
        "create_transaction",
        json!({"date": "2026-05-01", "amount_agorot": -1999, "category_id": food, "note": "a"}),
    )
    .await
    .unwrap();
    let id = created["transaction"]["id"].as_i64().unwrap();

    // A partial update keeps the other fields; null clears a link.
    let u = call(
        &db,
        "update_transaction",
        json!({"id": id, "note": "b", "category_id": pay}),
    )
    .await
    .unwrap();
    assert_eq!(u["transaction"]["note"], "b");
    assert_eq!(u["transaction"]["amount_agorot"], -1999);
    assert_eq!(u["transaction"]["date"], "2026-05-01");
    assert_eq!(u["transaction"]["category"]["id"], pay);
    assert_eq!(u["transaction"]["last_updated_by"], "mcp");
    let u = call(
        &db,
        "update_transaction",
        json!({"id": id, "category_id": null}),
    )
    .await
    .unwrap();
    assert!(u["transaction"]["category"].is_null());
    assert_eq!(u["transaction"]["amount_agorot"], -1999);
    assert!(
        call(&db, "update_transaction", json!({"id": 4242, "note": "x"}))
            .await
            .is_err()
    );
    assert!(call(
        &db,
        "update_transaction",
        json!({"id": id, "amount_agorot": 0})
    )
    .await
    .is_err());

    // Delete answers with what was removed, and the log keeps it.
    let d = call(&db, "delete_transaction", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(d["deleted"]["id"], id);
    assert!(db.list_transactions().await.unwrap().is_empty());
    assert!(call(&db, "delete_transaction", json!({"id": id}))
        .await
        .is_err());

    let log = call(&db, "list_operations_log", json!({"limit": 20}))
        .await
        .unwrap();
    let kinds: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        &kinds[..4],
        [
            "transaction_delete",
            "transaction_update",
            "transaction_update",
            "transaction_create"
        ]
    );
    for e in &log["entries"].as_array().unwrap()[..4] {
        assert_eq!(e["user_email"], "mcp");
        assert_eq!(e["actor_type"], "mcp");
    }
    // A forged Access header on the request does not change who made it.
    let forged = oplog::ACTOR
        .scope(
            Some("boss@x.com".into()),
            call(
                &db,
                "create_transaction",
                json!({"date": "2026-05-02", "amount_agorot": 1}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(forged["transaction"]["created_by"], "mcp");
}

#[tokio::test]
async fn a_foreign_line_keeps_its_rate_when_only_the_note_changes() {
    let _g = crate::fx::ENV_LOCK.lock().await;
    std::env::set_var("FX_FAKE_RATES", "USD=3.6");
    let db = db().await;
    let c = call(
        &db,
        "create_transaction",
        json!({"date": "2026-05-01", "currency": "USD", "orig_minor": -1000}),
    )
    .await
    .unwrap();
    let id = c["transaction"]["id"].as_i64().unwrap();
    assert_eq!(c["transaction"]["foreign_currency"]["currency"], "USD");
    assert_eq!(c["transaction"]["amount_agorot"], -3600);
    let u = call(&db, "update_transaction", json!({"id": id, "note": "trip"}))
        .await
        .unwrap();
    assert_eq!(
        u["transaction"]["foreign_currency"]["original_minor"],
        -1000
    );
    assert_eq!(u["transaction"]["amount_agorot"], -3600);
}

#[tokio::test]
async fn operations_log_pages_by_id_and_filters_by_date() {
    let db = db().await;
    for d in 1..=7 {
        db.create_transaction(&tx(&format!("2026-06-0{d}"), -d, None))
            .await
            .unwrap();
    }
    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut args = json!({"limit": 3});
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let p = call(&db, "list_operations_log", args).await.unwrap();
        seen.extend(
            p["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["id"].as_i64().unwrap()),
        );
        cursor = p["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(seen.len(), 7);
    assert!(seen.windows(2).all(|w| w[0] > w[1]));

    // Everything was logged today (UTC); a range that ends yesterday or starts tomorrow is empty.
    let today = chrono::Utc::now().date_naive();
    let day = |n: i64| {
        (today + chrono::Duration::days(n))
            .format("%Y-%m-%d")
            .to_string()
    };
    let n = |args: Value| {
        let db = db.clone();
        async move {
            call(&db, "list_operations_log", args).await.unwrap()["returned"]
                .as_u64()
                .unwrap()
        }
    };
    assert_eq!(
        n(json!({"limit": 50, "from": day(0), "to": day(0)})).await,
        7
    );
    assert_eq!(n(json!({"limit": 50, "to": day(-1)})).await, 0);
    assert_eq!(n(json!({"limit": 50, "from": day(1)})).await, 0);
    assert_eq!(
        n(json!({"limit": 50, "kind": "transaction_delete"})).await,
        0
    );
    let first_tx = db.list_transactions().await.unwrap()[0].id;
    assert_eq!(n(json!({"limit": 50, "entity_id": first_tx})).await, 1);
}

#[tokio::test]
async fn categories_list_everything() {
    let db = db().await;
    let (food, pay) = cats(&db).await;
    let sub = db
        .insert_category("Groceries", false, Some(food))
        .await
        .unwrap();
    let r = call(&db, "list_categories", json!({})).await.unwrap();
    assert_eq!(r["count"], 3);
    let find = |id: i64| {
        r["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(find(sub)["path"], "Food › Groceries");
    assert_eq!(find(sub)["parent_id"], food);
    assert_eq!(find(pay)["type"], "income");
    assert!(call(&db, "list_categories", json!({"limit": 1}))
        .await
        .is_err());
}
