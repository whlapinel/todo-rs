use chrono::{DateTime, Utc};

/// A browser's `PushSubscription`, stored so the background sweep
/// (`service::push::sweep_due_reminders`) can push to it later. `p256dh_key`/`auth_key` are
/// the *subscriber's* encryption keys (from the browser's subscription object) — distinct
/// from this server's own VAPID keypair (`PushConfig`, `src/push.rs`), which identifies the
/// server to the push service rather than encrypting to a particular subscriber.
#[derive(Debug, Clone, PartialEq)]
pub struct PushSubscription {
    pub id: String,
    pub user_id: String,
    pub endpoint: String,
    pub p256dh_key: String,
    pub auth_key: String,
    pub created_at: DateTime<Utc>,
}
