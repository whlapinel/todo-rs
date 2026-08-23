use super::{internal, not_found};
use crate::domain::user::User;
use crate::storage::sqlite::{RepoError, UserRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server};

pub async fn get_user(
    input: input::GetUserInput,
    server::Extension(repo): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::GetUserOutput, error::GetUserError> {
    let user = repo.get(&input.user_id).await.map_err(|e| match e {
        RepoError::NotFound => error::GetUserError::from(not_found()),
        _ => error::GetUserError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::GetUserOutput {
        user_id: user.id,
        first_name: user.first_name,
        last_name: user.last_name,
        timezone: user.timezone,
    })
}

pub async fn update_user(
    input: input::UpdateUserInput,
    server::Extension(repo): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::UpdateUserOutput, error::UpdateUserError> {
    // `timezone` is optional and preserved when omitted (unlike firstName/lastName,
    // which are always overwritten) — see UpdateUser's doc comment in user.smithy.
    let current = repo.get(&input.user_id).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateUserError::from(not_found()),
        _ => error::UpdateUserError::from(internal(format!("{e:?}"))),
    })?;
    let user = User {
        id: input.user_id,
        first_name: input.first_name,
        last_name: input.last_name,
        email: current.email,
        google_id: current.google_id,
        timezone: input.timezone.or(current.timezone),
        personal_project_id: current.personal_project_id,
    };
    repo.update(&user).await.map_err(|e| match e {
        RepoError::NotFound => error::UpdateUserError::from(not_found()),
        _ => error::UpdateUserError::from(internal(format!("{e:?}"))),
    })?;
    Ok(output::UpdateUserOutput {})
}

pub async fn list_users(
    _input: input::ListUsersInput,
    server::Extension(repo): server::Extension<Arc<dyn UserRepo>>,
) -> Result<output::ListUsersOutput, error::ListUsersError> {
    let users = repo.list().await.map_err(|e| internal(format!("{e:?}")))?;
    let users = users
        .into_iter()
        .map(|u| todo_server_sdk::model::UserSummary {
            user_id: u.id,
            first_name: u.first_name,
            last_name: u.last_name,
        })
        .collect();
    Ok(output::ListUsersOutput { users })
}
