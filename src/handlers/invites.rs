use crate::{auth::AuthUser, email, storage::UserRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server};

pub async fn send_app_invite(
    input: input::SendAppInviteInput,
    server::Extension(user_repo): server::Extension<Arc<dyn UserRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::SendAppInviteOutput, error::SendAppInviteError> {
    let config = email::EmailConfig::from_env().ok_or_else(|| {
        tracing::error!("send_app_invite: TODO_SMTP_USER or TODO_SMTP_PASSWORD not set");
        error::SendAppInviteError::from(error::PeoplesRepublicOfListsError {
            message: "email not configured".to_string(),
        })
    })?;

    let user = user_repo.get(&auth.user_id).await.map_err(|e| {
        tracing::error!("send_app_invite: failed to fetch inviter {}: {e:?}", auth.user_id);
        error::SendAppInviteError::from(error::PeoplesRepublicOfListsError {
            message: "internal error".to_string(),
        })
    })?;

    let inviter_name = format!("{} {}", user.first_name, user.last_name);

    email::send_app_invite(&config, &input.email, &inviter_name)
        .await
        .map_err(|e| {
            tracing::error!("send_app_invite: failed to send to {}: {e}", input.email);
            error::SendAppInviteError::from(error::PeoplesRepublicOfListsError {
                message: "failed to send invite".to_string(),
            })
        })?;

    tracing::info!("app invite sent to {} by {}", input.email, auth.user_id);
    Ok(output::SendAppInviteOutput {})
}
