use crate::auth::AuthUser;
use crate::push::PushConfig;
use crate::storage::sqlite::PushSubscriptionRepo;
use axum::extract::{Extension, Json};
use axum::http::StatusCode;
use serde::Deserialize;
use std::sync::Arc;

/// Subscribe/unsubscribe/public-key routes for push notifications
/// (`docs/push-notifications-plan.md`) — plain `web_ui` routes, no Smithy operation, same
/// precedent as `notifications.rs`: this is a browser-native concept with no CLI/MCP
/// equivalent. Driven by a plain `fetch()` call from `pwa-assets/push.js`, not htmx, so
/// `axum::Json` (this module's only user of it in `web_ui`, everything else here is htmx
/// form posts) is the right extractor rather than a deviation to avoid.
pub async fn push_public_key() -> Result<String, StatusCode> {
    match PushConfig::from_env() {
        Some(config) => Ok(config.vapid_public_key_b64url),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[derive(Deserialize)]
pub struct SubscribeBody {
    endpoint: String,
    keys: SubscribeKeys,
}

#[derive(Deserialize)]
pub struct SubscribeKeys {
    p256dh: String,
    auth: String,
}

pub async fn push_subscribe(
    Extension(auth_user): Extension<AuthUser>,
    Extension(push_subs): Extension<Arc<dyn PushSubscriptionRepo>>,
    Json(body): Json<SubscribeBody>,
) -> Result<StatusCode, StatusCode> {
    push_subs
        .create_or_update(
            &auth_user.user_id,
            &body.endpoint,
            &body.keys.p256dh,
            &body.keys.auth,
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct UnsubscribeBody {
    endpoint: String,
}

pub async fn push_unsubscribe(
    Extension(push_subs): Extension<Arc<dyn PushSubscriptionRepo>>,
    Json(body): Json<UnsubscribeBody>,
) -> Result<StatusCode, StatusCode> {
    push_subs
        .delete_by_endpoint(&body.endpoint)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}
