//! Authenticating the Google Pub/Sub push request.
//!
//! Pub/Sub can sign its push requests with an OIDC token (`Authorization: Bearer <JWT>`) for a
//! service account you choose, with an audience you choose. We check that token against
//! Google's public keys, and also a secret in the URL (`?token=`). The body is never trusted:
//! it only wakes the sync (see the module docs of [`super`]).

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use axum::http::StatusCode;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use super::Service;

#[derive(Deserialize, Clone, Debug)]
pub struct Jwk {
    pub kid: String,
    pub n: String,
    pub e: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    email: String,
    #[serde(default)]
    email_verified: bool,
}

/// Equal strings, compared without stopping at the first difference.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

/// Check a Google-signed ID token: signature, issuer, audience, expiry, and the service
/// account it was issued for (when one is configured).
pub fn verify_oidc(
    token: &str,
    jwks: &Jwks,
    audience: &str,
    service_account: Option<&str>,
) -> Result<()> {
    let header = decode_header(token).map_err(|_| anyhow!("not a JWT"))?;
    if header.alg != Algorithm::RS256 {
        bail!("unexpected signing algorithm");
    }
    let kid = header.kid.ok_or_else(|| anyhow!("token has no key id"))?;
    let key = jwks
        .keys
        .iter()
        .find(|k| k.kid == kid)
        .ok_or_else(|| anyhow!("unknown signing key"))?;
    let mut v = Validation::new(Algorithm::RS256);
    v.set_audience(&[audience]);
    v.set_issuer(&["https://accounts.google.com", "accounts.google.com"]);
    v.set_required_spec_claims(&["exp", "aud", "iss"]);
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_rsa_components(&key.n, &key.e)?,
        &v,
    )
    .map_err(|e| anyhow!("token rejected: {e}"))?;
    if let Some(sa) = service_account {
        if !data.claims.email_verified
            || !same(&data.claims.email.to_lowercase(), &sa.to_lowercase())
        {
            bail!("token was issued for another account");
        }
    }
    Ok(())
}

impl Service {
    async fn jwks_for(&self, kid_hint: Option<&str>) -> Result<Jwks> {
        let cfg = self.cfg()?;
        {
            let cache = self.jwks_cache().lock().unwrap();
            if let Some((j, at)) = &*cache {
                let known = kid_hint.is_none_or(|k| j.keys.iter().any(|x| x.kid == k));
                // A key we do not know may be a new one: look again, but not more than once a minute.
                if at.elapsed() < Duration::from_secs(3600)
                    && (known || at.elapsed() < Duration::from_secs(60))
                {
                    return Ok(j.clone());
                }
            }
        }
        let j: Jwks = self
            .http()
            .get(&cfg.jwks_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        *self.jwks_cache().lock().unwrap() = Some((j.clone(), Instant::now()));
        Ok(j)
    }

    /// Is this push request genuine? `Err` carries the status to answer with. With neither a
    /// URL token nor an audience configured the endpoint is off (404), never open.
    pub async fn authorize_push(
        &self,
        url_token: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<(), StatusCode> {
        let Some(cfg) = self.cfg.as_ref() else {
            return Err(StatusCode::NOT_FOUND);
        };
        if cfg.push_token.is_none() && cfg.push_audience.is_none() {
            return Err(StatusCode::NOT_FOUND);
        }
        if let Some(expected) = &cfg.push_token {
            if !url_token.is_some_and(|t| same(t, expected)) {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        if let Some(aud) = &cfg.push_audience {
            let token = bearer
                .and_then(|b| b.strip_prefix("Bearer "))
                .ok_or(StatusCode::UNAUTHORIZED)?;
            let kid = decode_header(token).ok().and_then(|h| h.kid);
            let jwks = self.jwks_for(kid.as_deref()).await.map_err(|e| {
                tracing::warn!("fetching Google's signing keys failed: {e:#}");
                StatusCode::SERVICE_UNAVAILABLE
            })?;
            if let Err(e) = verify_oidc(token, &jwks, aud, cfg.push_service_account.as_deref()) {
                tracing::warn!("rejected a push request: {e:#}");
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        Ok(())
    }
}
