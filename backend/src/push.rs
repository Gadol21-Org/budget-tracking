//! Web push to the devices that installed the app, so a new receipt shows up as a phone
//! notification even when the app is closed.
//!
//! The push carries no payload (so no message encryption is needed): the service worker, on
//! waking, asks `/api/sumit` for the unread notices and shows them. Only the push endpoint URLs
//! are stored. Pushes are signed with VAPID keys from the environment:
//! `VAPID_PRIVATE_KEY_PEM_B64` (base64 of a PKCS#8 P-256 PEM), `VAPID_PUBLIC_KEY` (the matching
//! uncompressed point, base64url, which the browser needs to subscribe) and optionally
//! `VAPID_SUBJECT` (`mailto:` address). Without them push is off and the in-app list still works.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;

use crate::storage::Storage;

const KEY: &str = "push_endpoints";
const MAX_DEVICES: usize = 20;

/// Push services browsers use; endpoints elsewhere are refused (the server posts to them).
const ALLOWED_HOSTS: [&str; 5] = [
    "fcm.googleapis.com",
    "updates.push.services.mozilla.com",
    "push.apple.com",
    "notify.windows.com",
    "push.services.mozilla.com",
];

pub fn public_key() -> Option<String> {
    std::env::var("VAPID_PUBLIC_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .filter(|_| private_pem().is_some())
}

fn private_pem() -> Option<String> {
    let raw = std::env::var("VAPID_PRIVATE_KEY_PEM_B64").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("-----") {
        return Some(raw.replace("\\n", "\n"));
    }
    // Tolerate wrapped lines, stray spaces and url-safe or unpadded base64 from a copy-paste.
    let clean: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .take_while(|c| *c != '=')
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(clean)
        .ok()?;
    String::from_utf8(bytes).ok()
}

pub fn check_endpoint(endpoint: &str) -> Result<()> {
    let rest = endpoint
        .strip_prefix("https://")
        .ok_or_else(|| anyhow!("push endpoint must be https"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.contains('@') || host.contains(':') {
        bail!("push endpoint host not allowed");
    }
    if !ALLOWED_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        bail!("push endpoint host not allowed");
    }
    Ok(())
}

async fn endpoints(db: &dyn Storage) -> Result<Vec<String>> {
    crate::sumit::load(db, KEY).await
}

pub async fn subscribe(db: &dyn Storage, endpoint: &str) -> Result<()> {
    check_endpoint(endpoint)?;
    let mut all = endpoints(db).await?;
    if !all.iter().any(|e| e == endpoint) {
        all.push(endpoint.to_string());
        if all.len() > MAX_DEVICES {
            all.remove(0);
        }
        crate::sumit::save(db, KEY, &all).await?;
    }
    Ok(())
}

/// Is this device's push endpoint stored on the server?
pub async fn is_registered(db: &dyn Storage, endpoint: &str) -> Result<bool> {
    Ok(endpoints(db).await?.iter().any(|e| e == endpoint))
}

#[derive(Serialize)]
struct Claims {
    aud: String,
    exp: u64,
    sub: String,
}

fn vapid_header(endpoint: &str, pem: &str, public: &str) -> Result<String> {
    let origin: String = endpoint
        .splitn(4, '/')
        .take(3)
        .collect::<Vec<_>>()
        .join("/");
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        + 12 * 3600;
    let sub = std::env::var("VAPID_SUBJECT").unwrap_or_else(|_| "mailto:admin@example.com".into());
    let jwt = jsonwebtoken::encode(
        &Header::new(Algorithm::ES256),
        &Claims {
            aud: origin,
            exp,
            sub,
        },
        &EncodingKey::from_ec_pem(pem.as_bytes()).context("VAPID private key")?,
    )?;
    Ok(format!("vapid t={jwt}, k={public}"))
}

/// Wake every subscribed device. Dead subscriptions (404/410) are dropped; any other failure is
/// only logged. Never fails the caller.
pub async fn notify_all(db: &dyn Storage) {
    let (Some(public), Some(pem)) = (public_key(), private_pem()) else {
        return;
    };
    let Ok(all) = endpoints(db).await else {
        return;
    };
    let http = reqwest::Client::new();
    let mut keep = Vec::new();
    for ep in all {
        let sent = async {
            let auth = vapid_header(&ep, &pem, &public)?;
            let r = http
                .post(&ep)
                .header("Authorization", auth)
                .header("TTL", "86400")
                .header("Urgency", "high")
                .header("Content-Length", "0")
                .send()
                .await
                .map_err(|e| anyhow!("{}", e.without_url()))?;
            Ok::<_, anyhow::Error>(r.status())
        }
        .await;
        match sent {
            Ok(s) if s.as_u16() == 404 || s.as_u16() == 410 => continue,
            Ok(s) if !s.is_success() => tracing::warn!("push: service answered {s}"),
            Err(e) => tracing::warn!("push failed: {e:#}"),
            Ok(_) => {}
        }
        keep.push(ep);
    }
    if let Err(e) = crate::sumit::save(db, KEY, &keep).await {
        tracing::warn!("push: saving subscriptions failed: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_push_services_are_accepted() {
        assert!(check_endpoint("https://fcm.googleapis.com/fcm/send/abc").is_ok());
        assert!(check_endpoint("https://web.push.apple.com/xyz").is_ok());
        assert!(check_endpoint("https://wns2-par02p.notify.windows.com/?token=1").is_ok());
        assert!(check_endpoint("http://fcm.googleapis.com/x").is_err());
        assert!(check_endpoint("https://evil.example.com/x").is_err());
        assert!(check_endpoint("https://fcm.googleapis.com@evil.com/x").is_err());
        assert!(check_endpoint("https://notfcm.googleapis.com.evil.com/x").is_err());
    }

    // A throwaway key made only for this test.
    const TEST_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgpOQ9JdX3PqeRe7vx\nJi1jugv+q/aGEhbC8zWePrdJY3OhRANCAAT4iLsUBO8lR3FLJnM0Z3Omj7Zkkops\nQMr/A/W/Y9+gYKdbJ2PUqsfkOj6HuNj/Ka8kmceMl8oNi/PovjprV+mw\n-----END PRIVATE KEY-----\n";

    #[test]
    fn vapid_header_is_signed_with_a_p256_key() {
        let h = vapid_header(
            "https://fcm.googleapis.com/fcm/send/abc",
            TEST_PEM,
            "PUBKEY",
        )
        .unwrap();
        assert!(h.starts_with("vapid t=") && h.ends_with(", k=PUBKEY"));
        let jwt = h.trim_start_matches("vapid t=").split(',').next().unwrap();
        assert_eq!(jwt.split('.').count(), 3);
        assert!(vapid_header("https://x/y", "not a key", "K").is_err());
    }
}
