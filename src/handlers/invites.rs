use axum::{
    Extension, Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{auth::AuthUser, email, storage::UserRepo};

#[derive(Deserialize)]
pub struct AppInviteRequest {
    pub email: String,
}

pub async fn send_app_invite(
    Extension(auth_user): Extension<AuthUser>,
    Extension(user_repo): Extension<Arc<dyn UserRepo>>,
    Json(body): Json<AppInviteRequest>,
) -> Response {
    let config = match email::EmailConfig::from_env() {
        Some(c) => c,
        None => {
            tracing::error!("send_app_invite: TODO_SMTP_USER or TODO_SMTP_PASSWORD not set");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "email not configured"})),
            )
                .into_response();
        }
    };

    let user = match user_repo.get(&auth_user.user_id).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("send_app_invite: failed to fetch inviter {}: {e:?}", auth_user.user_id);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let inviter_name = format!("{} {}", user.first_name, user.last_name);

    match email::send_app_invite(&config, &body.email, &inviter_name).await {
        Ok(()) => {
            tracing::info!("app invite sent to {} by {}", body.email, auth_user.user_id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            tracing::error!("send_app_invite: failed to send to {}: {e}", body.email);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "failed to send invite"})),
            )
                .into_response()
        }
    }
}
