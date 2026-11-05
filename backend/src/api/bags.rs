//! Bags (טיוגים): nested groups of transactions that cut across categories.

use super::error::{internal, ApiError};
use super::Db;
use crate::storage::{Bag, BagInput};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

/// Bags nest at most this deep (Vacations › Japan 2026 › Kyoto).
pub(crate) const MAX_DEPTH: usize = 3;

#[derive(Serialize)]
pub(crate) struct BagData {
    bags: Vec<Bag>,
    /// (transaction id, bag id)
    links: Vec<(i64, i64)>,
}

pub(crate) async fn get_bags(State(db): State<Db>) -> Result<Json<BagData>, ApiError> {
    Ok(Json(BagData {
        bags: db.list_bags().await.map_err(internal)?,
        links: db.bag_links().await.map_err(internal)?,
    }))
}

fn bad(msg: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, msg.into())
}

fn depth_of(bags: &[Bag], id: i64) -> usize {
    let mut d = 1;
    let mut cur = bags.iter().find(|b| b.id == id).and_then(|b| b.parent_id);
    while let Some(p) = cur {
        d += 1;
        if d > 100 {
            break;
        }
        cur = bags.iter().find(|b| b.id == p).and_then(|b| b.parent_id);
    }
    d
}

/// How many levels the bag `id` and everything under it take (1 for a bag with no sub-bags).
fn height_of(bags: &[Bag], id: i64) -> usize {
    1 + bags
        .iter()
        .filter(|b| b.parent_id == Some(id))
        .map(|b| height_of(bags, b.id))
        .max()
        .unwrap_or(0)
}

/// Checks a bag's new values against the others: a name, a parent that exists and is not under the bag
/// itself, a sibling name that is free, and no more than [`MAX_DEPTH`] levels.
fn validate(bags: &[Bag], id: Option<i64>, b: &mut BagInput) -> Result<(), ApiError> {
    b.name = b.name.trim().to_string();
    if b.name.is_empty() {
        return Err(bad("יש להזין שם"));
    }
    if let Some(p) = b.parent_id {
        if !bags.iter().any(|x| x.id == p) {
            return Err(bad("טיוג האב לא נמצא"));
        }
        if let Some(id) = id {
            // The parent may not be the bag itself or anything under it.
            let mut cur = Some(p);
            let mut guard = 0;
            while let Some(c) = cur {
                if c == id {
                    return Err(bad("אי אפשר להכניס טיוג אל תוך עצמו או אל תוך תת-טיוג שלו"));
                }
                guard += 1;
                if guard > 100 {
                    break;
                }
                cur = bags.iter().find(|x| x.id == c).and_then(|x| x.parent_id);
            }
        }
    }
    let above = b.parent_id.map_or(0, |p| depth_of(bags, p));
    let below = id.map_or(1, |i| height_of(bags, i));
    if above + below > MAX_DEPTH {
        return Err(bad("טיוגים מקוננים עד 3 רמות"));
    }
    if bags
        .iter()
        .any(|x| Some(x.id) != id && x.parent_id == b.parent_id && x.name == b.name)
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "כבר יש כאן טיוג בשם הזה".into(),
        ));
    }
    if b.budget_agorot.is_some_and(|v| v < 0) {
        return Err(bad("התקציב אינו יכול להיות שלילי"));
    }
    let date_ok = |d: &Option<String>| {
        d.as_ref().is_none_or(|s| {
            s.is_empty()
                || s.len() == 10
                    && s.chars().enumerate().all(|(i, c)| {
                        if i == 4 || i == 7 {
                            c == '-'
                        } else {
                            c.is_ascii_digit()
                        }
                    })
        })
    };
    if !date_ok(&b.start_date) || !date_ok(&b.end_date) {
        return Err(bad("התאריכים אינם תקינים"));
    }
    // Empty strings mean "none".
    b.start_date = b.start_date.take().filter(|s| !s.is_empty());
    b.end_date = b.end_date.take().filter(|s| !s.is_empty());
    Ok(())
}

pub(crate) async fn create_bag(
    State(db): State<Db>,
    Json(mut b): Json<BagInput>,
) -> Result<Json<i64>, ApiError> {
    let bags = db.list_bags().await.map_err(internal)?;
    validate(&bags, None, &mut b)?;
    db.insert_bag(&b).await.map(Json).map_err(internal)
}

pub(crate) async fn change_bag(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(mut b): Json<BagInput>,
) -> Result<StatusCode, ApiError> {
    let bags = db.list_bags().await.map_err(internal)?;
    if !bags.iter().any(|x| x.id == id) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "הטיוג לא נמצא".into()));
    }
    if is_root(&bags, id) {
        return Err(bad("אי אפשר לשנות את הטיוג עסקים"));
    }
    validate(&bags, Some(id), &mut b)?;
    db.update_bag(id, &b).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

fn is_root(bags: &[Bag], id: i64) -> bool {
    bags.iter()
        .any(|b| b.id == id && b.role.as_deref() == Some("business_root"))
}

/// Removes the bag and its sub-bags; the transactions in them stay.
pub(crate) async fn delete_bag(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let bags = db.list_bags().await.map_err(internal)?;
    if is_root(&bags, id) {
        return Err(bad("אי אפשר למחוק את הטיוג עסקים"));
    }
    db.remove_bag(id).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct Assign {
    ids: Vec<i64>,
    #[serde(default)]
    add: Vec<i64>,
    #[serde(default)]
    remove: Vec<i64>,
}

/// Put transactions in some bags and take them out of others.
pub(crate) async fn assign_bags(
    State(db): State<Db>,
    Json(a): Json<Assign>,
) -> Result<StatusCode, ApiError> {
    let bags = db.list_bags().await.map_err(internal)?;
    if a.add.iter().any(|id| !bags.iter().any(|b| b.id == *id)) {
        return Err(bad("הטיוג לא נמצא"));
    }
    db.assign_bags(&a.ids, &a.add, &a.remove)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bag(id: i64, name: &str, parent: Option<i64>) -> Bag {
        Bag {
            id,
            name: name.into(),
            parent_id: parent,
            archived: false,
            budget_agorot: None,
            start_date: None,
            end_date: None,
            role: None,
        }
    }
    fn input(name: &str, parent: Option<i64>) -> BagInput {
        BagInput {
            name: name.into(),
            parent_id: parent,
            archived: false,
            budget_agorot: None,
            start_date: None,
            end_date: None,
        }
    }

    #[test]
    fn nests_three_levels_and_no_more() {
        let bags = vec![
            bag(1, "A", None),
            bag(2, "B", Some(1)),
            bag(3, "C", Some(2)),
        ];
        assert!(validate(&bags, None, &mut input("D", Some(1))).is_ok());
        assert!(validate(&bags, None, &mut input("D", Some(3))).is_err());
        // Moving a bag with a sub-bag under a level-2 bag would make four levels.
        let bags = vec![
            bag(1, "A", None),
            bag(2, "B", Some(1)),
            bag(4, "X", None),
            bag(5, "Y", Some(4)),
        ];
        assert!(validate(&bags, Some(4), &mut input("X", Some(2))).is_err());
        assert!(validate(&bags, Some(5), &mut input("Y", Some(2))).is_ok());
    }

    #[test]
    fn rejects_cycles_blank_and_duplicate_names() {
        let bags = vec![bag(1, "A", None), bag(2, "B", Some(1))];
        assert!(validate(&bags, Some(1), &mut input("A", Some(2))).is_err());
        assert!(validate(&bags, Some(1), &mut input("A", Some(1))).is_err());
        assert!(validate(&bags, None, &mut input("  ", None)).is_err());
        assert!(validate(&bags, None, &mut input("B", Some(1))).is_err());
        assert!(validate(&bags, None, &mut input("B", None)).is_ok());
        assert!(validate(&bags, Some(2), &mut input("B", Some(1))).is_ok());
    }
}
