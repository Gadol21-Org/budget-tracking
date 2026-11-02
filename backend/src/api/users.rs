//! Who made a change: the Cloudflare Access email on the request, and the display names
//! given to the emails seen on recorded changes. The header is optional: without it (local
//! runs, staging) changes are simply recorded with no user.

use super::error::{internal, ApiError};
use super::Db;
use crate::oplog;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const HEADER: &str = "cf-access-authenticated-user-email";
const SETTING: &str = "user_display_names";

/// Run the request with its user (if any) available to the operation log.
pub(crate) async fn with_actor(req: Request, next: Next) -> Response {
    let actor = oplog::actor_from_header(req.headers().get(HEADER));
    oplog::ACTOR.scope(actor, next.run(req)).await
}

#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct KnownUser {
    email: String,
    /// Empty when no display name was given.
    name: String,
}

async fn names(db: &Db) -> Result<BTreeMap<String, String>, ApiError> {
    Ok(db
        .get_setting(SETTING)
        .await
        .map_err(internal)?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

/// Emails seen on recorded changes, with their display names.
pub(crate) async fn list_users(State(db): State<Db>) -> Result<Json<Vec<KnownUser>>, ApiError> {
    let names = names(&db).await?;
    let emails = db.log_users().await.map_err(internal)?;
    Ok(Json(
        emails
            .into_iter()
            .map(|email| KnownUser {
                name: names.get(&email).cloned().unwrap_or_default(),
                email,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub(crate) struct NameBody {
    email: String,
    /// Empty removes the display name.
    name: String,
}

pub(crate) async fn set_user_name(
    State(db): State<Db>,
    Json(b): Json<NameBody>,
) -> Result<StatusCode, ApiError> {
    let email = b.email.trim().to_lowercase();
    // Only emails that made a change before can be named.
    if !db.log_users().await.map_err(internal)?.contains(&email) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no recorded change from this email".into(),
        ));
    }
    let mut all = names(&db).await?;
    let name = b.name.trim();
    if name.is_empty() {
        all.remove(&email);
    } else {
        all.insert(email, name.chars().take(60).collect());
    }
    db.set_setting(SETTING, &serde_json::to_string(&all).map_err(internal)?)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oplog::Logged;
    use crate::storage::NewTransaction;
    use std::sync::Arc;

    async fn db() -> Db {
        Arc::new(Logged(Arc::new(crate::testdb::fresh().await)))
    }

    fn tx() -> NewTransaction {
        NewTransaction {
            date: "2026-01-02".into(),
            amount_agorot: -100,
            category_id: None,
            payment_method_id: None,
            note: String::new(),
        }
    }

    #[tokio::test]
    async fn changes_record_the_user_when_present_and_work_without() {
        let db = db().await;
        db.create_transaction(&tx()).await.unwrap(); // no header: no user, no error
        oplog::ACTOR
            .scope(Some("a@x.com".into()), async {
                db.create_transaction(&tx()).await.unwrap();
            })
            .await;
        let log = db.list_log(10, None).await.unwrap();
        assert_eq!(log[0].user_email.as_deref(), Some("a@x.com"));
        assert_eq!(log[1].user_email, None);
        let users = list_users(State(db.clone())).await.unwrap().0;
        assert_eq!(
            users,
            vec![KnownUser {
                email: "a@x.com".into(),
                name: "".into()
            }]
        );
    }

    #[tokio::test]
    async fn only_known_emails_can_be_named_and_empty_name_clears() {
        let db = db().await;
        oplog::ACTOR
            .scope(Some("a@x.com".into()), async {
                db.create_transaction(&tx()).await.unwrap();
            })
            .await;
        let set = |email: &str, name: &str| {
            set_user_name(
                State(db.clone()),
                Json(NameBody {
                    email: email.into(),
                    name: name.into(),
                }),
            )
        };
        assert!(set("nobody@x.com", "Nobody").await.is_err());
        set("A@x.com", " Dana ").await.unwrap();
        assert_eq!(
            list_users(State(db.clone())).await.unwrap().0[0].name,
            "Dana"
        );
        set("a@x.com", "").await.unwrap();
        assert_eq!(list_users(State(db.clone())).await.unwrap().0[0].name, "");
    }

    #[test]
    fn header_parsing_ignores_junk() {
        use axum::http::HeaderValue;
        assert_eq!(oplog::actor_from_header(None), None);
        assert_eq!(
            oplog::actor_from_header(Some(&HeaderValue::from_static("nope"))),
            None
        );
        assert_eq!(
            oplog::actor_from_header(Some(&HeaderValue::from_static(" Me@X.com "))),
            Some("me@x.com".into())
        );
    }
}
