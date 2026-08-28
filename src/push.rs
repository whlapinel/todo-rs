use base64ct::{Base64UrlUnpadded, Encoding as _};
use web_push_native::{
    Auth, WebPushBuilder,
    jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair},
    p256::PublicKey,
};

use crate::domain::push_subscription::PushSubscription;

/// VAPID identity for this server, mirroring `EmailConfig::from_env` (`src/email.rs`) — an
/// optional feature, silently absent (`None`) if unconfigured, with no `Extension` wiring:
/// callers (the push sweep, `src/service/push.rs`) just call `from_env()` at the point of
/// use. See `docs/push-notifications-plan.md` for why this crate (`web-push-native`, pure
/// Rust) was chosen over the more established `web-push` crate (which pulls `openssl-sys`
/// transitively with no pure-Rust option, risking this repo's cross-compiling Dockerfile).
pub struct PushConfig {
    pub vapid_key_pair: ES256KeyPair,
    /// Base64url-encoded raw uncompressed P-256 point, precomputed once — this is what the
    /// browser's `pushManager.subscribe({ applicationServerKey })` needs, handed out by
    /// `GET /web/push/public-key` (`src/web_ui/push.rs`).
    pub vapid_public_key_b64url: String,
    /// VAPID contact, e.g. `mailto:whlapinel@gmail.com` — `TODO_VAPID_SUBJECT`.
    pub subject: String,
}

impl PushConfig {
    /// `TODO_VAPID_PRIVATE_KEY` is a raw 32-byte P-256 private scalar, base64url-encoded —
    /// generate one with `cargo run --example gen_vapid_key`. Not a PEM file: this crate's
    /// key format (`ES256KeyPair::from_bytes`) is the raw scalar, unlike most VAPID
    /// tutorials which assume an OpenSSL-generated PEM.
    pub fn from_env() -> Option<Self> {
        let private_key_b64 = std::env::var("TODO_VAPID_PRIVATE_KEY").ok()?;
        let subject = std::env::var("TODO_VAPID_SUBJECT").ok()?;
        let private_key_bytes = Base64UrlUnpadded::decode_vec(&private_key_b64).ok()?;
        let vapid_key_pair = ES256KeyPair::from_bytes(&private_key_bytes).ok()?;
        let vapid_public_key_b64url = Base64UrlUnpadded::encode_string(
            &vapid_key_pair
                .public_key()
                .public_key()
                .to_bytes_uncompressed(),
        );
        Some(Self {
            vapid_key_pair,
            vapid_public_key_b64url,
            subject,
        })
    }
}

/// Outcome of one `send_push` call — distinguishes "the push service says this subscription
/// no longer exists" (404/410 — the sweep should prune it, see
/// `service::push::sweep_due_reminders`) from any other failure (left alone, retried on the
/// next sweep since `reminders.push_sent_at` isn't set until after the whole reminder's
/// send attempts finish regardless of outcome — see the plan doc's "best effort" note).
pub enum PushSendOutcome {
    Delivered,
    Gone,
    Failed(String),
}

/// Encrypts `title`/`body`/`url` as a JSON payload and sends it to one subscriber via Web
/// Push (RFC 8030 + VAPID/RFC 8292), reusing this process's existing reqwest client rather
/// than `web-push-native`'s own client feature (which it doesn't have — it only builds an
/// `http::Request`). `web-push-native` depends on `http` 1.x while `reqwest` 0.11 (this
/// repo's pinned version, for its rustls-tls feature) depends on `http` 0.2 — two different
/// crate versions of the same-named types, with no automatic conversion between them — so
/// the built request's method/uri/headers/body are copied across by hand rather than
/// converted.
pub async fn send_push(
    client: &reqwest::Client,
    config: &PushConfig,
    sub: &PushSubscription,
    title: &str,
    body: &str,
    url: &str,
) -> PushSendOutcome {
    let endpoint: http1::Uri = match sub.endpoint.parse() {
        Ok(uri) => uri,
        Err(e) => return PushSendOutcome::Failed(format!("invalid endpoint: {e}")),
    };
    let ua_public_bytes = match Base64UrlUnpadded::decode_vec(&sub.p256dh_key) {
        Ok(bytes) => bytes,
        Err(e) => return PushSendOutcome::Failed(format!("invalid p256dh key: {e}")),
    };
    let ua_public = match PublicKey::from_sec1_bytes(&ua_public_bytes) {
        Ok(key) => key,
        Err(e) => return PushSendOutcome::Failed(format!("invalid p256dh key: {e}")),
    };
    let ua_auth_bytes = match Base64UrlUnpadded::decode_vec(&sub.auth_key) {
        Ok(bytes) => bytes,
        Err(e) => return PushSendOutcome::Failed(format!("invalid auth key: {e}")),
    };
    if ua_auth_bytes.len() != 16 {
        return PushSendOutcome::Failed("auth key is not 16 bytes".to_string());
    }
    let ua_auth = Auth::clone_from_slice(&ua_auth_bytes);

    let payload = serde_json::json!({ "title": title, "body": body, "url": url }).to_string();

    let builder = WebPushBuilder::new(endpoint, ua_public, ua_auth)
        .with_vapid(&config.vapid_key_pair, &config.subject);
    let request = match builder.build(payload.into_bytes()) {
        Ok(request) => request,
        Err(e) => return PushSendOutcome::Failed(format!("failed to build push request: {e}")),
    };

    let (parts, push_body) = request.into_parts();
    let mut req_builder = client.post(parts.uri.to_string());
    for (name, value) in parts.headers.iter() {
        req_builder = req_builder.header(name.as_str(), value.as_bytes());
    }

    match req_builder.body(push_body).send().await {
        Ok(response) if response.status().is_success() => PushSendOutcome::Delivered,
        Ok(response) if matches!(response.status().as_u16(), 404 | 410) => PushSendOutcome::Gone,
        Ok(response) => {
            PushSendOutcome::Failed(format!("push service returned {}", response.status()))
        }
        Err(e) => PushSendOutcome::Failed(format!("push send error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn from_env_round_trips_a_generated_key() {
        let key_pair = ES256KeyPair::generate();
        let private_key_b64url = Base64UrlUnpadded::encode_string(&key_pair.to_bytes());

        // SAFETY: single-threaded test, no other test in this process reads these vars.
        unsafe {
            std::env::set_var("TODO_VAPID_PRIVATE_KEY", &private_key_b64url);
            std::env::set_var("TODO_VAPID_SUBJECT", "mailto:test@example.com");
        }
        let config = PushConfig::from_env().expect("config should parse");
        unsafe {
            std::env::remove_var("TODO_VAPID_PRIVATE_KEY");
            std::env::remove_var("TODO_VAPID_SUBJECT");
        }

        assert_eq!(config.subject, "mailto:test@example.com");
        // 65-byte uncompressed P-256 point, base64url-unpadded == 87 chars.
        assert_eq!(config.vapid_public_key_b64url.len(), 87);
    }

    #[test]
    fn from_env_is_none_when_unconfigured() {
        // SAFETY: single-threaded test, no other test in this process reads these vars.
        unsafe {
            std::env::remove_var("TODO_VAPID_PRIVATE_KEY");
            std::env::remove_var("TODO_VAPID_SUBJECT");
        }
        assert!(PushConfig::from_env().is_none());
    }

    #[tokio::test]
    async fn send_push_against_an_unreachable_endpoint_fails_without_panicking() {
        let key_pair = ES256KeyPair::generate();
        let config = PushConfig {
            vapid_public_key_b64url: Base64UrlUnpadded::encode_string(
                &key_pair.public_key().public_key().to_bytes_uncompressed(),
            ),
            vapid_key_pair: key_pair,
            subject: "mailto:test@example.com".to_string(),
        };

        // A subscriber's own keypair — we just need any valid P-256 public point + 16-byte
        // auth secret, not a real browser subscription, since this only exercises
        // build+send, not decryption on the receiving end.
        let subscriber_key = ES256KeyPair::generate();
        let ua_public_b64 = Base64UrlUnpadded::encode_string(
            &subscriber_key
                .public_key()
                .public_key()
                .to_bytes_uncompressed(),
        );
        let sub = PushSubscription {
            id: "sub1".to_string(),
            user_id: "user1".to_string(),
            endpoint: "https://push.example.invalid/does-not-exist".to_string(),
            p256dh_key: ua_public_b64,
            auth_key: Base64UrlUnpadded::encode_string(&[0u8; 16]),
            created_at: Utc::now(),
        };

        let client = reqwest::Client::new();
        let outcome = send_push(&client, &config, &sub, "title", "body", "/web/tasks").await;
        assert!(matches!(outcome, PushSendOutcome::Failed(_)));
    }
}
