use crate::domain::user::User;
use crate::service::error::ItemError;
use crate::storage::sqlite::UserRepo;
use chrono::{Offset, Utc};
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

/// Resolves `user_id`'s IANA timezone (`users.timezone`) to its current offset in minutes,
/// falling back to UTC (0) when unset or unparseable — the same `Tz::from_str(tz).ok()`
/// pattern `service::calendar_sync` uses. For callers with no live `X-Tz-Offset-Minutes`
/// header to read (`TzOffset`, `src/web_ui/mod.rs`): the push sweep's reminder labels, and
/// JSON API operations (CLI/MCP) whose Smithy input carries no offset field.
pub async fn resolve_tz_offset_minutes(users: &Arc<dyn UserRepo>, user_id: &str) -> i32 {
    let Ok(user) = users.get(user_id).await else {
        return 0;
    };
    let Some(tz_name) = user.timezone else {
        return 0;
    };
    let Ok(tz) = Tz::from_str(&tz_name) else {
        return 0;
    };
    // `to_local`/`reminder_labels` (`src/web_ui/mod.rs`) use the JS `Date.getTimezoneOffset()`
    // convention (positive when local is *behind* UTC), matching the `X-Tz-Offset-Minutes`
    // header a live request carries. `local_minus_utc()` is chrono's opposite-signed native
    // convention, so it's negated here to match.
    -(Utc::now()
        .with_timezone(&tz)
        .offset()
        .fix()
        .local_minus_utc()
        / 60)
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

    #[tokio::test]
    async fn resolve_tz_offset_minutes_matches_js_get_timezone_offset_sign() {
        let mut users = MockUserRepo::new();
        users.expect_get().returning(|_| {
            Ok(crate::domain::user::User {
                id: "user1".to_string(),
                first_name: "Test".to_string(),
                last_name: "User".to_string(),
                email: None,
                google_id: None,
                timezone: Some("America/New_York".to_string()),
            })
        });
        let users: Arc<dyn UserRepo> = Arc::new(users);

        let offset = resolve_tz_offset_minutes(&users, "user1").await;

        // `America/New_York` is always behind UTC (EST -300 / EDT -240), so under the
        // `Date.getTimezoneOffset()`/`to_local` convention this app uses everywhere else
        // (positive when local is behind UTC), the resolved offset must be positive.
        assert!(
            offset > 0,
            "expected a positive (behind-UTC) offset for America/New_York, got {offset}"
        );
    }
}
