//! One-time VAPID keypair generator for push notifications
//! (docs/push-notifications-plan.md). Run with `cargo run --example gen_vapid_key` and set
//! the printed value as `TODO_VAPID_PRIVATE_KEY`. Pure Rust (via `web-push-native`'s own
//! dependencies) — no OpenSSL/PEM step needed, unlike most VAPID tutorials: this crate's key
//! format is a raw 32-byte P-256 private scalar, base64url-encoded.

use base64ct::{Base64UrlUnpadded, Encoding as _};
use web_push_native::jwt_simple::algorithms::ES256KeyPair;

fn main() {
    let key_pair = ES256KeyPair::generate();
    let private_key_b64url = Base64UrlUnpadded::encode_string(&key_pair.to_bytes());
    println!("TODO_VAPID_PRIVATE_KEY={private_key_b64url}");
    println!("TODO_VAPID_SUBJECT=mailto:you@example.com");
}
