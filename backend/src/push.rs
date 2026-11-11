//! Web push to the devices that installed the app, so a new receipt or email charge shows up as a phone
//! notification even when the app is closed.
//!
//! The push carries its text (title, body, and the page to open when tapped) as an encrypted payload
//! (RFC 8291, `aes128gcm`), so the service worker can show it without asking the server anything. That
//! needs the keys each browser makes when it subscribes (`p256dh` and `auth`), stored next to its
//! endpoint URL. A device stored without keys (subscribed before payloads existed) only gets an empty
//! wake-up until it subscribes again. Pushes are signed with VAPID keys from the environment:
//! `VAPID_PRIVATE_KEY_PEM_B64` (base64 of a PKCS#8 P-256 PEM), `VAPID_PUBLIC_KEY` (the matching
//! uncompressed point, base64url, which the browser needs to subscribe) and optionally
//! `VAPID_SUBJECT` (`mailto:` address). Without them push is off and the in-app list still works.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

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

/// What a browser hands over when it subscribes (`PushSubscription.toJSON().keys`), both base64url.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Device {
    endpoint: String,
    keys: Option<Keys>,
}

/// The stored list is either the old one (bare endpoint URLs) or devices with their keys.
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored {
    Old(String),
    Device(Device),
}

async fn devices(db: &dyn Storage) -> Result<Vec<Device>> {
    let stored: Vec<Stored> = crate::sumit::load(db, KEY).await?;
    Ok(stored
        .into_iter()
        .map(|s| match s {
            Stored::Old(endpoint) => Device {
                endpoint,
                keys: None,
            },
            Stored::Device(d) => d,
        })
        .collect())
}

fn b64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|_| anyhow!("push keys are not base64url"))
}

fn check_keys(k: &Keys) -> Result<()> {
    p256::PublicKey::from_sec1_bytes(&b64(&k.p256dh)?).map_err(|_| anyhow!("bad push key"))?;
    if b64(&k.auth)?.len() != 16 {
        bail!("bad push auth secret");
    }
    Ok(())
}

/// Remember a device (again, with its keys if it has them: a device that subscribed before payloads existed is replaced).
pub async fn subscribe(db: &dyn Storage, endpoint: &str, keys: Option<Keys>) -> Result<()> {
    check_endpoint(endpoint)?;
    if let Some(k) = &keys {
        check_keys(k)?;
    }
    let mut all = devices(db).await?;
    match all.iter_mut().find(|d| d.endpoint == endpoint) {
        Some(d) => {
            if keys.is_none() || d.keys == keys {
                return Ok(());
            }
            d.keys = keys;
        }
        None => {
            all.push(Device {
                endpoint: endpoint.to_string(),
                keys,
            });
            if all.len() > MAX_DEVICES {
                all.remove(0);
            }
        }
    }
    crate::sumit::save(db, KEY, &all).await
}

/// Can this device get a push with text? A device stored without keys is not: it is offered the notifications again.
pub async fn is_registered(db: &dyn Storage, endpoint: &str) -> Result<bool> {
    Ok(devices(db)
        .await?
        .iter()
        .any(|d| d.endpoint == endpoint && d.keys.is_some()))
}

/// What a notification says. `url` is the page opened when it is tapped.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// Notifications with the same tag replace each other on the phone.
    pub tag: String,
    pub url: String,
}

/// The encrypted body of a push (RFC 8291, `aes128gcm`): the header (salt, record size, our public key) and one record.
fn encrypt_with(
    plain: &[u8],
    ua_public: &[u8],
    auth: &[u8],
    as_secret: &p256::SecretKey,
    salt: [u8; 16],
) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let ua = p256::PublicKey::from_sec1_bytes(ua_public).map_err(|_| anyhow!("bad push key"))?;
    let as_public = as_secret.public_key().to_encoded_point(false);
    let shared = p256::ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua.as_affine());
    // The key shared with this browser, mixed with its auth secret.
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .map_err(|_| anyhow!("hkdf"))?;
    let prk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| anyhow!("hkdf"))?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| anyhow!("hkdf"))?;
    // One record: the text, then the 0x02 that marks the last record (no padding).
    let mut record = plain.to_vec();
    record.push(2);
    let sealed = aes_gcm::Aes128Gcm::new(&cek.into())
        .encrypt(&nonce.into(), record.as_slice())
        .map_err(|_| anyhow!("encrypting the push failed"))?;
    let mut body = salt.to_vec();
    body.extend_from_slice(&4096u32.to_be_bytes());
    body.push(as_public.as_bytes().len() as u8);
    body.extend_from_slice(as_public.as_bytes());
    body.extend_from_slice(&sealed);
    Ok(body)
}

fn encrypt(plain: &[u8], keys: &Keys) -> Result<Vec<u8>> {
    use rand_core::{OsRng, RngCore};
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    encrypt_with(
        plain,
        &b64(&keys.p256dh)?,
        &b64(&keys.auth)?,
        &p256::SecretKey::random(&mut OsRng),
        salt,
    )
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

/// Send a notification to every subscribed device. Dead subscriptions (404/410) are dropped; any other failure is
/// only logged. Never fails the caller.
pub async fn notify_all(db: &dyn Storage, msg: &Message) {
    let (Some(public), Some(pem)) = (public_key(), private_pem()) else {
        return;
    };
    let Ok(all) = devices(db).await else {
        return;
    };
    let Ok(text) = serde_json::to_vec(msg) else {
        return;
    };
    let http = reqwest::Client::new();
    let mut keep = Vec::new();
    for d in all {
        let sent = async {
            let auth = vapid_header(&d.endpoint, &pem, &public)?;
            let req = http
                .post(&d.endpoint)
                .header("Authorization", auth)
                .header("TTL", "86400")
                .header("Urgency", "high");
            // Without keys there is nothing to encrypt for: an empty wake-up (the app shows a generic notice).
            let req = match &d.keys {
                Some(k) => req
                    .header("Content-Encoding", "aes128gcm")
                    .header("Content-Type", "application/octet-stream")
                    .body(encrypt(&text, k)?),
                None => req.header("Content-Length", "0"),
            };
            let r = req
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
        keep.push(d);
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

    fn keys_for(secret: &p256::SecretKey) -> Keys {
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        Keys {
            p256dh: e.encode(secret.public_key().to_encoded_point(false).as_bytes()),
            auth: e.encode([7u8; 16]),
        }
    }

    #[tokio::test]
    async fn devices_are_remembered_once_and_bad_endpoints_refused() {
        let db = crate::testdb::fresh().await;
        let ep = "https://fcm.googleapis.com/fcm/send/abc";
        let keys = keys_for(&p256::SecretKey::random(&mut rand_core::OsRng));
        assert!(!is_registered(&db, ep).await.unwrap());
        subscribe(&db, ep, Some(keys.clone())).await.unwrap();
        subscribe(&db, ep, Some(keys)).await.unwrap();
        assert!(is_registered(&db, ep).await.unwrap());
        assert_eq!(devices(&db).await.unwrap().len(), 1);
        assert!(subscribe(&db, "https://evil.example.com/x", None)
            .await
            .is_err());
        let bad = Keys {
            p256dh: "AAAA".into(),
            auth: "AAAA".into(),
        };
        assert!(subscribe(&db, ep, Some(bad)).await.is_err());
    }

    #[tokio::test]
    async fn a_device_stored_before_payloads_has_to_subscribe_again() {
        let db = crate::testdb::fresh().await;
        let ep = "https://fcm.googleapis.com/fcm/send/old";
        // The old format: bare endpoint URLs.
        crate::sumit::save(&db, KEY, &vec![ep.to_string()])
            .await
            .unwrap();
        assert!(!is_registered(&db, ep).await.unwrap());
        let keys = keys_for(&p256::SecretKey::random(&mut rand_core::OsRng));
        subscribe(&db, ep, Some(keys)).await.unwrap();
        assert!(is_registered(&db, ep).await.unwrap());
        assert_eq!(devices(&db).await.unwrap().len(), 1);
    }

    /// The example in RFC 8291 appendix A.
    #[test]
    fn the_payload_is_encrypted_as_rfc_8291_says() {
        let d = |s: &str| b64(s).unwrap();
        let as_secret =
            p256::SecretKey::from_slice(&d("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let ua_public = d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let salt: [u8; 16] = d("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body = encrypt_with(
            b"When I grow up, I want to be a watermelon",
            &ua_public,
            &d("BTBZMqHH6r4Tts7J_aSIgg"),
            &as_secret,
            salt,
        )
        .unwrap();
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }
}
