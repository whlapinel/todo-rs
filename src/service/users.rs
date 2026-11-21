use crate::domain::user::User;
use crate::service::error::ItemError;
use crate::storage::sqlite::UserRepo;
use chrono_tz::Tz;
use std::str::FromStr;
use std::sync::Arc;

/// What a user may change about their own profile. `timezone` is the full desired value,
/// not a patch: `None` clears it (falls back to UTC in every consumer), unlike the JSON
/// API's `UpdateUser`, where omitting it preserves the current value.
pub struct ProfileUpdate {
    pub first_name: String,
    pub last_name: String,
    pub timezone: Option<String>,
}

/// Updates the requester's own profile. Email and Google id are identity, owned by the
/// auth flow, and are carried over unchanged.
///
/// `timezone` must be an IANA name chrono-tz recognizes. Its consumers
/// (`service::push`'s reminder labels, `service::calendar_sync`'s all-day localization)
/// each silently fall back to UTC on an unparseable value, so an unchecked typo here
/// would look like it saved but quietly behave as UTC.
pub async fn update_profile(
    users: &Arc<dyn UserRepo>,
    user_id: &str,
    update: ProfileUpdate,
) -> Result<User, ItemError> {
    let first_name = update.first_name.trim().to_string();
    let last_name = update.last_name.trim().to_string();
    if first_name.is_empty() {
        return Err(ItemError::Invalid("first name is required".to_string()));
    }
    let timezone = update
        .timezone
        .map(|tz| tz.trim().to_string())
        .filter(|tz| !tz.is_empty());
    if let Some(tz) = &timezone
        && Tz::from_str(tz).is_err()
    {
        return Err(ItemError::Invalid(format!("unknown timezone \"{tz}\"")));
    }

    let current = users.get(user_id).await?;
    let user = User {
        first_name,
        last_name,
        timezone,
        ..current
    };
    users.update(&user).await?;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::MockUserRepo;

    fn existing() -> User {
        User {
            id: "u1".to_string(),
            first_name: "Old".to_string(),
            last_name: "Name".to_string(),
            email: Some("a@example.com".to_string()),
            google_id: Some("g1".to_string()),
            timezone: Some("America/New_York".to_string()),
        }
    }

    fn update(tz: Option<&str>) -> ProfileUpdate {
        ProfileUpdate {
            first_name: " New ".to_string(),
            last_name: "Person".to_string(),
            timezone: tz.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn saves_names_and_timezone_preserving_identity_fields() {
        let mut repo = MockUserRepo::new();
        repo.expect_get().returning(|_| Ok(existing()));
        repo.expect_update()
            .withf(|u| {
                u.first_name == "New"
                    && u.timezone.as_deref() == Some("America/Los_Angeles")
                    && u.email.as_deref() == Some("a@example.com")
                    && u.google_id.as_deref() == Some("g1")
            })
            .times(1)
            .returning(|_| Ok(()));
        let repo: Arc<dyn UserRepo> = Arc::new(repo);
        update_profile(&repo, "u1", update(Some("America/Los_Angeles")))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn blank_timezone_clears_it() {
        let mut repo = MockUserRepo::new();
        repo.expect_get().returning(|_| Ok(existing()));
        repo.expect_update()
            .withf(|u| u.timezone.is_none())
            .times(1)
            .returning(|_| Ok(()));
        let repo: Arc<dyn UserRepo> = Arc::new(repo);
        update_profile(&repo, "u1", update(Some("  ")))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_an_unknown_timezone_without_writing() {
        let mut repo = MockUserRepo::new();
        repo.expect_update().never();
        let repo: Arc<dyn UserRepo> = Arc::new(repo);
        let err = update_profile(&repo, "u1", update(Some("Pacific/Atlantis")))
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn rejects_an_empty_first_name() {
        let mut repo = MockUserRepo::new();
        repo.expect_update().never();
        let repo: Arc<dyn UserRepo> = Arc::new(repo);
        let mut u = update(None);
        u.first_name = "   ".to_string();
        assert!(matches!(
            update_profile(&repo, "u1", u).await,
            Err(ItemError::Invalid(_))
        ));
    }
}
