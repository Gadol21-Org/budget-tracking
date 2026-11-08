use super::*;
use crate::mysql::MySqlStorage;

/// Record a pending transaction as sent to Paamonim under `line`.
async fn mark(s: &MySqlStorage, id: i64, line: &str) {
    let p = s
        .pending_mirrors()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap();
    assert!(s.mark_mirrored(&p, line).await.unwrap());
}

fn tx(date: &str, amount: i64, cat: Option<i64>) -> NewTransaction {
    NewTransaction {
        date: date.into(),
        amount_agorot: amount,
        category_id: cat,
        payment_method_id: None,
        note: String::new(),
    }
}

#[tokio::test]
async fn business_tags_and_links_follow_their_rows() {
    let s = crate::testdb::fresh().await;
    let a = s.insert_business("A", false).await.unwrap();
    let g = s.insert_business("General", true).await.unwrap();
    let t1 = s
        .create_transaction(&tx("2026-10-03", -100, None))
        .await
        .unwrap();
    let t2 = s
        .create_transaction(&tx("2026-10-04", -200, None))
        .await
        .unwrap();
    s.set_transactions_business(&[t1, t2], Some(a))
        .await
        .unwrap();
    s.set_category_business(7, Some(g)).await.unwrap();
    let l = s.business_links().await.unwrap();
    assert_eq!(l.transactions.len(), 2);
    assert_eq!(l.categories, vec![(7, g)]);
    // A deleted transaction drops out of the tags.
    s.delete_transaction(t1).await.unwrap();
    assert_eq!(
        s.business_links().await.unwrap().transactions,
        vec![(t2, a)]
    );
    s.set_transactions_business(&[t2], None).await.unwrap();
    assert!(s.business_links().await.unwrap().transactions.is_empty());
    // Removing a business drops what pointed at it.
    s.set_transactions_business(&[t2], Some(a)).await.unwrap();
    s.remove_business(a).await.unwrap();
    s.remove_business(g).await.unwrap();
    let l = s.business_links().await.unwrap();
    assert!(l.transactions.is_empty() && l.categories.is_empty());
    assert!(s.list_businesses().await.unwrap().is_empty());
}

#[tokio::test]
async fn lists_only_transactions_from_a_date_on() {
    let s = crate::testdb::fresh().await;
    for d in ["2024-01-05", "2025-06-30", "2025-07-01", "2026-10-03"] {
        s.create_transaction(&tx(d, -1000, None)).await.unwrap();
    }
    let dates = |v: Vec<Transaction>| v.into_iter().map(|t| t.date).collect::<Vec<_>>();
    assert_eq!(
        dates(s.list_transactions_from("2025-07-01").await.unwrap()),
        ["2026-10-03", "2025-07-01"]
    );
    assert_eq!(
        s.list_transactions_from("2000-01-01").await.unwrap().len(),
        4
    );
    assert!(s
        .list_transactions_from("2027-01-01")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn editing_a_synced_line_flags_it_modified() {
    let s = crate::testdb::fresh().await;
    let id = s
        .create_transaction(&tx("2026-10-03", -1000, None))
        .await
        .unwrap();
    s.update_transaction(id, &tx("2026-10-04", -2000, None))
        .await
        .unwrap();
    let t = &s.list_transactions().await.unwrap()[0];
    assert_eq!(
        (t.date.as_str(), t.amount_agorot, t.sync_state),
        ("2026-10-04", -2000, SyncState::Pending)
    );
    mark(&s, id, "77").await;
    s.update_transaction(id, &tx("2026-10-05", -2500, None))
        .await
        .unwrap();
    assert_eq!(
        s.list_transactions().await.unwrap()[0].sync_state,
        SyncState::Modified
    );
}

#[tokio::test]
async fn import_links_parents_and_fills_missing_links_on_rerun() {
    let s = crate::testdb::fresh().await;
    let line = |cat: Option<&str>| ImportTransaction {
        line_id: "1".into(),
        date: "2026-10-03".into(),
        amount_agorot: -100,
        category_key: cat.map(String::from),
        payment_key: None,
        note: String::new(),
    };
    let cat = |key: &str, parent: Option<&str>| ImportCategory {
        key: key.into(),
        name: key.into(),
        is_income: false,
        parent_key: parent.map(String::from),
    };
    // First run does not know the item yet, so the line has no category.
    let first = ImportBatch {
        transactions: vec![line(None)],
        ..Default::default()
    };
    s.import_paamonim(&first).await.unwrap();
    let second = ImportBatch {
        categories: vec![cat("ce:3", None), cat("e:1", Some("ce:3"))],
        transactions: vec![line(Some("e:1"))],
        ..Default::default()
    };
    s.import_paamonim(&second).await.unwrap();
    let cats = s.list_categories().await.unwrap();
    let child = cats.iter().find(|c| c.name == "e:1").unwrap();
    let parent = cats.iter().find(|c| c.name == "ce:3").unwrap();
    assert_eq!(child.parent_id, Some(parent.id));
    assert_eq!(
        s.list_transactions().await.unwrap()[0].category_id,
        Some(child.id)
    );
}

#[tokio::test]
async fn import_is_idempotent() {
    let s = crate::testdb::fresh().await;
    let batch = ImportBatch {
        categories: vec![ImportCategory {
            key: "e:1".into(),
            name: "Food".into(),
            is_income: false,
            parent_key: None,
        }],
        payment_methods: vec![ImportPaymentMethod {
            key: "7".into(),
            name: "Card".into(),
        }],
        transactions: vec![ImportTransaction {
            line_id: "100".into(),
            date: "2026-10-03".into(),
            amount_agorot: -12300,
            category_key: Some("e:1".into()),
            payment_key: Some("7".into()),
            note: String::new(),
        }],
    };
    let first = s.import_paamonim(&batch).await.unwrap();
    assert_eq!((first.categories_added, first.transactions_added), (1, 1));
    let second = s.import_paamonim(&batch).await.unwrap();
    assert_eq!(
        (
            second.categories_added,
            second.transactions_added,
            second.transactions_skipped
        ),
        (0, 0, 1)
    );
    let totals = s.monthly_totals().await.unwrap();
    assert_eq!((totals[0].total_agorot, totals[0].count), (-12300, 1));
}

#[tokio::test]
async fn monthly_totals_follow_inserts_and_deletes() {
    let s = crate::testdb::fresh().await;
    let a = s
        .create_transaction(&tx("2026-10-03", 1050, None))
        .await
        .unwrap();
    s.create_transaction(&tx("2026-10-20", 2025, None))
        .await
        .unwrap();
    let totals = s.monthly_totals().await.unwrap();
    assert_eq!(totals.len(), 1);
    assert_eq!(totals[0].total_agorot, 3075); // non-round amounts stay exact
    s.delete_transaction(a).await.unwrap();
    assert_eq!(s.monthly_totals().await.unwrap()[0].total_agorot, 2025);
}

/// Paamonim income category `ci:1` with items משכורת (`i:7`) and בונוס (`i:8`).
async fn with_income() -> (MySqlStorage, i64, i64, i64) {
    let s = crate::testdb::fresh().await;
    let cat = |key: &str, name: &str, parent: Option<&str>| ImportCategory {
        key: key.into(),
        name: name.into(),
        is_income: true,
        parent_key: parent.map(str::to_string),
    };
    let batch = ImportBatch {
        categories: vec![
            cat("ci:1", "הכנסות", None),
            cat("i:7", "משכורת", Some("ci:1")),
            cat("i:8", "בונוס", Some("ci:1")),
        ],
        ..Default::default()
    };
    s.import_paamonim(&batch).await.unwrap();
    let cats = s.list_categories().await.unwrap();
    let id = |n: &str| cats.iter().find(|c| c.name == n).unwrap().id;
    (s, id("הכנסות"), id("משכורת"), id("בונוס"))
}

fn top(name: &str, linked_to: Option<i64>) -> NewCategory {
    NewCategory {
        name: name.into(),
        is_income: true,
        parent_id: None,
        linked_to,
    }
}

#[tokio::test]
async fn a_new_top_level_category_must_be_linked_to_a_paamonim_one() {
    let (s, income, salary, _) = with_income().await;
    let err = s.create_category(&top("עומר", None)).await.unwrap_err();
    assert!(err.to_string().contains("Paamonim category"));
    // Not to a sub-category, and not across kinds.
    assert!(s.create_category(&top("עומר", Some(salary))).await.is_err());
    let exp = NewCategory {
        is_income: false,
        ..top("x", Some(income))
    };
    // No Paamonim expense categories: an expense top-level needs no link, and can't
    // take an income one.
    assert!(s.create_category(&exp).await.is_err());
    let id = s.create_category(&top("עומר", Some(income))).await.unwrap();
    let made = s.list_categories().await.unwrap();
    let made = made.iter().find(|c| c.id == id).unwrap();
    assert_eq!((made.linked_to, made.from_paamonim), (Some(income), false));
    // Neither can be deleted from under the link.
    assert!(s.delete_category(income, None).await.is_err());
}

#[tokio::test]
async fn a_sub_category_moves_with_its_lines_and_keeps_its_place_on_reimport() {
    let (s, income, salary, bonus) = with_income().await;
    let omer = s.create_category(&top("עומר", Some(income))).await.unwrap();
    let wife = s.create_category(&top("אשתי", Some(income))).await.unwrap();
    let t = s
        .create_transaction(&NewTransaction {
            date: "2026-10-01".into(),
            amount_agorot: 1_000_000,
            category_id: Some(salary),
            payment_method_id: None,
            note: String::new(),
        })
        .await
        .unwrap();
    s.move_category(salary, wife).await.unwrap();
    s.move_category(bonus, omer).await.unwrap();
    let parent = |cats: &[Category], id: i64| cats.iter().find(|c| c.id == id).unwrap().parent_id;
    let cats = s.list_categories().await.unwrap();
    assert_eq!(
        (parent(&cats, salary), parent(&cats, bonus)),
        (Some(wife), Some(omer))
    );
    let tx = s.list_transactions().await.unwrap();
    assert_eq!(
        tx.iter().find(|x| x.id == t).unwrap().category_id,
        Some(salary)
    );
    // A re-import leaves them where they were moved.
    let cat = |key: &str, name: &str, parent: Option<&str>| ImportCategory {
        key: key.into(),
        name: name.into(),
        is_income: true,
        parent_key: parent.map(str::to_string),
    };
    s.import_paamonim(&ImportBatch {
        categories: vec![
            cat("ci:1", "הכנסות", None),
            cat("i:7", "משכורת", Some("ci:1")),
            cat("i:8", "בונוס", Some("ci:1")),
        ],
        ..Default::default()
    })
    .await
    .unwrap();
    let cats = s.list_categories().await.unwrap();
    assert_eq!(
        (parent(&cats, salary), parent(&cats, bonus)),
        (Some(wife), Some(omer))
    );
    // Back under the Paamonim category is fine too.
    s.move_category(bonus, income).await.unwrap();
    // A Paamonim item can't move to a category that isn't linked to its Paamonim category.
    let other = s
        .create_category(&NewCategory {
            is_income: false,
            ..top("הוצאה", None)
        })
        .await
        .unwrap();
    assert!(s.move_category(salary, other).await.is_err());
    // The link can't change while a Paamonim item sits under it.
    assert!(s.link_category_to(wife, None).await.is_err());
    // A name the target already has is refused.
    let dup = s
        .create_category(&NewCategory {
            parent_id: Some(omer),
            ..top("משכורת", None)
        })
        .await
        .unwrap();
    assert!(dup > 0);
    assert!(s.move_category(salary, omer).await.is_err());
}

fn new_cat(name: &str, parent_id: Option<i64>, is_income: bool) -> NewCategory {
    NewCategory {
        name: name.into(),
        is_income,
        parent_id,
        linked_to: None,
    }
}

fn imported(key: &str, parent: Option<&str>) -> ImportCategory {
    ImportCategory {
        key: key.into(),
        name: key.into(),
        is_income: false,
        parent_key: parent.map(String::from),
    }
}

fn imported_line(line: &str, cat: &str) -> ImportTransaction {
    ImportTransaction {
        line_id: line.into(),
        date: "2026-10-03".into(),
        amount_agorot: -100,
        category_key: Some(cat.into()),
        payment_key: None,
        note: String::new(),
    }
}

#[tokio::test]
async fn creating_categories_validates_and_nests_two_levels() {
    let s = crate::testdb::fresh().await;
    let top = s
        .create_category(&new_cat(" Home ", None, true))
        .await
        .unwrap();
    let kid = s
        .create_category(&new_cat("Rent", Some(top), false))
        .await
        .unwrap();
    let cats = s.list_categories().await.unwrap();
    let kid = cats.iter().find(|c| c.id == kid).unwrap();
    // Trimmed, and a sub-category takes its parent's kind.
    assert_eq!((kid.parent_id, kid.is_income), (Some(top), true));
    assert!(cats.iter().any(|c| c.name == "Home"));
    let refused = |r: anyhow::Result<i64>| {
        matches!(
            r.unwrap_err().downcast_ref::<CategoryError>(),
            Some(CategoryError::Invalid(_))
        )
    };
    assert!(refused(
        s.create_category(&new_cat("  ", None, false)).await
    ));
    assert!(refused(
        s.create_category(&new_cat("Home", None, true)).await
    ));
    assert!(refused(
        s.create_category(&new_cat("x", Some(kid.id), false)).await
    ));
    assert!(refused(
        s.create_category(&new_cat("x", Some(999), false)).await
    ));
}

#[tokio::test]
async fn deleting_a_used_category_needs_a_valid_replacement() {
    let s = crate::testdb::fresh().await;
    let home = s
        .create_category(&new_cat("Home", None, false))
        .await
        .unwrap();
    let rent = s
        .create_category(&new_cat("Rent", Some(home), false))
        .await
        .unwrap();
    let food = s
        .create_category(&new_cat("Food", None, false))
        .await
        .unwrap();
    let pay = s
        .create_category(&new_cat("Pay", None, true))
        .await
        .unwrap();
    s.create_transaction(&tx("2026-10-03", -100, Some(rent)))
        .await
        .unwrap();
    let err = |r: anyhow::Result<usize>| r.unwrap_err().downcast::<CategoryError>().unwrap();
    // Used through a sub-category, so deleting the parent needs a replacement too.
    assert_eq!(
        err(s.delete_category(home, None).await),
        CategoryError::NeedsReplacement {
            transactions: 1,
            rules: 0
        }
    );
    assert!(matches!(
        err(s.delete_category(home, Some(rent)).await),
        CategoryError::Invalid(_)
    ));
    assert!(matches!(
        err(s.delete_category(home, Some(pay)).await),
        CategoryError::Invalid(_)
    ));
    assert_eq!(
        err(s.delete_category(999, None).await),
        CategoryError::NotFound
    );
    assert_eq!(s.delete_category(home, Some(food)).await.unwrap(), 1);
    let t = &s.list_transactions().await.unwrap()[0];
    assert_eq!(t.category_id, Some(food));
    let names: Vec<_> = s
        .list_categories()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["Food", "Pay"]);
    // Unused categories go without a replacement.
    assert_eq!(s.delete_category(pay, None).await.unwrap(), 0);
}

#[tokio::test]
async fn moving_lines_flags_synced_ones_as_modified() {
    let s = crate::testdb::fresh().await;
    let a = s.create_category(&new_cat("A", None, false)).await.unwrap();
    let b = s.create_category(&new_cat("B", None, false)).await.unwrap();
    let synced = s
        .create_transaction(&tx("2026-10-03", -100, Some(a)))
        .await
        .unwrap();
    mark(&s, synced, "5").await;
    s.create_transaction(&tx("2026-10-04", -100, Some(a)))
        .await
        .unwrap();
    s.delete_category(a, Some(b)).await.unwrap();
    let mut states: Vec<_> = s
        .list_transactions()
        .await
        .unwrap()
        .into_iter()
        .map(|t| (t.category_id, t.sync_state))
        .collect();
    states.sort();
    assert_eq!(
        states,
        [
            (Some(b), SyncState::Pending),
            (Some(b), SyncState::Modified)
        ]
    );
}

#[tokio::test]
async fn reimport_does_not_bring_back_deleted_categories() {
    let s = crate::testdb::fresh().await;
    let batch = |extra: Vec<ImportCategory>, lines: Vec<ImportTransaction>| ImportBatch {
        categories: [
            imported("ce:1", None),
            imported("e:1", Some("ce:1")),
            imported("e:2", None),
        ]
        .into_iter()
        .chain(extra)
        .collect(),
        transactions: lines,
        ..Default::default()
    };
    s.import_paamonim(&batch(vec![], vec![imported_line("1", "e:1")]))
        .await
        .unwrap();
    let find = |cats: &[Category], n: &str| cats.iter().find(|c| c.name == n).map(|c| c.id);
    let cats = s.list_categories().await.unwrap();
    let (parent, target) = (find(&cats, "ce:1").unwrap(), find(&cats, "e:2").unwrap());
    s.delete_category(parent, Some(target)).await.unwrap();

    // Same data again, plus a new line in the deleted category and a new item under
    // the deleted parent: nothing is recreated and new lines follow the move.
    let again = batch(
        vec![imported("e:3", Some("ce:1"))],
        vec![
            imported_line("1", "e:1"),
            imported_line("2", "e:1"),
            imported_line("3", "e:3"),
        ],
    );
    let sum = s.import_paamonim(&again).await.unwrap();
    assert_eq!((sum.categories_added, sum.transactions_added), (1, 2));
    let names: Vec<_> = s
        .list_categories()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["e:2"]);
    assert!(s
        .list_transactions()
        .await
        .unwrap()
        .iter()
        .all(|t| t.category_id == Some(target)));
    // And a third run adds nothing.
    let sum = s.import_paamonim(&again).await.unwrap();
    assert_eq!((sum.categories_added, sum.transactions_added), (0, 0));
}

#[tokio::test]
async fn deleted_paamonim_category_without_replacement_leaves_new_lines_uncategorized() {
    let s = crate::testdb::fresh().await;
    let batch = |lines| ImportBatch {
        categories: vec![imported("e:1", None)],
        transactions: lines,
        ..Default::default()
    };
    s.import_paamonim(&batch(vec![])).await.unwrap();
    let id = s.list_categories().await.unwrap()[0].id;
    s.delete_category(id, None).await.unwrap();
    s.import_paamonim(&batch(vec![imported_line("9", "e:1")]))
        .await
        .unwrap();
    assert!(s.list_categories().await.unwrap().is_empty());
    assert_eq!(s.list_transactions().await.unwrap()[0].category_id, None);
}

#[tokio::test]
async fn repeating_rules_count_as_references_and_move_with_the_category() {
    use crate::recurring::NewRecurring;
    let s = crate::testdb::fresh().await;
    let a = s.create_category(&new_cat("A", None, false)).await.unwrap();
    let b = s.create_category(&new_cat("B", None, false)).await.unwrap();
    let rule = |month: &str| NewRecurring {
        start_month: month.into(),
        end_month: None,
        day: 1,
        amount_agorot: -500,
        category_id: Some(a),
        payment_method_id: None,
        note: String::new(),
        currency: None,
        orig_minor: None,
    };
    let first = s.insert_recurring(None, &rule("2026-01")).await.unwrap();
    s.insert_recurring(Some(first), &rule("2026-06"))
        .await
        .unwrap();
    // No transactions, but one series (two segments) still uses the category.
    let err = s.delete_category(a, None).await.unwrap_err();
    assert_eq!(
        err.downcast::<CategoryError>().unwrap(),
        CategoryError::NeedsReplacement {
            transactions: 0,
            rules: 1
        }
    );
    s.delete_category(a, Some(b)).await.unwrap();
    let rules = s.list_recurring().await.unwrap();
    assert!(rules.iter().all(|r| r.category_id == Some(b)));
    assert_eq!(rules.len(), 2);
}
