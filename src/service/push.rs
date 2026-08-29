use crate::push::{PushConfig, PushSendOutcome, send_push};
use crate::storage::sqlite::{
    ItemRepo, ProjectRepo, PushSubscriptionRepo, ReminderRepo, RepoError, UserRepo,
};
use crate::web_ui::reminder_labels;
use chrono::{Offset, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

/// Ambient handle to the push-sending machinery (VAPID config, the subscriptions
/// table, and an HTTP client), set once at startup — see `src/main.rs` — only when
/// `PushConfig::from_env()` succeeds. This is the one deliberate global in an
/// otherwise all-DI codebase: `notify_completion_change`/`notify_assignment` below
/// are called from deep inside `service::team_items`, which is reached from ~30
/// `create_project_item`/`update_project_item` call sites across `web_ui`/`json_api`.
/// Threading three more parameters through every one of those, purely to support a
/// best-effort, fire-and-forget side effect, would mean touching every call site for
/// no behavioral benefit — the same "optional feature, silently absent if
/// unconfigured" shape `PushConfig`/`EmailConfig` already use, just read from a
/// static instead of an `Extension` because the call site is a service function, not
/// a handler.
static RUNTIME: OnceLock<PushRuntime> = OnceLock::new();

#[derive(Clone)]
pub struct PushRuntime {
    config: Arc<PushConfig>,
    subs: Arc<dyn PushSubscriptionRepo>,
    client: reqwest::Client,
}

impl PushRuntime {
    pub fn init(
        config: Arc<PushConfig>,
        subs: Arc<dyn PushSubscriptionRepo>,
        client: reqwest::Client,
    ) {
        // Ignored if already set — `main.rs` only ever calls this once, but a second
        // call (e.g. in a future test harness) silently no-ops rather than panicking.
        let _ = RUNTIME.set(PushRuntime {
            config,
            subs,
            client,
        });
    }

    fn get() -> Option<&'static PushRuntime> {
        RUNTIME.get()
    }
}

/// Sends a best-effort push to every subscription `user_id` has registered, logging
/// and swallowing any per-subscription failure — mirrors `sweep_due_reminders`'s own
/// per-subscription handling, minus the `Gone` pruning (a stale endpoint hit here is
/// cheap enough to leave for the reminder sweep to prune next time it hits the same
/// subscription, rather than duplicating that cleanup here too).
async fn push_to_user(runtime: &PushRuntime, user_id: &str, title: &str, body: &str, url: &str) {
    let subs = match runtime.subs.list_for_user(user_id).await {
        Ok(subs) => subs,
        Err(e) => {
            tracing::warn!(user_id = %user_id, error = ?e, "failed to list push subscriptions");
            return;
        }
    };
    for sub in &subs {
        if let PushSendOutcome::Failed(e) =
            send_push(&runtime.client, &runtime.config, sub, title, body, url).await
        {
            tracing::warn!(subscription_id = %sub.id, error = %e, "push send failed");
        }
    }
}

/// Notifies every other member of `project_id` that `item_name` was just completed or
/// reopened by `actor_user_id` — see docs/issues_and_features.md's "Send notification
/// on complete or uncomplete to all project team members". Called from
/// `service::team_items::update_team_item` on every genuine completion-state
/// transition, regardless of nesting depth (a sub-item's completion is just as much
/// "the team should know" as a top-level one, unlike points/activity-log which are
/// deliberately top-level-only). No-ops immediately if push isn't configured on this
/// server. Fire-and-forget (`tokio::spawn`): a notification failure must never fail,
/// or add latency to, the update request that triggered it.
pub fn notify_completion_change(
    projects: Arc<dyn ProjectRepo>,
    project_id: String,
    item_name: String,
    detail_url: Option<String>,
    actor_user_id: String,
    completed: bool,
) {
    let Some(runtime) = PushRuntime::get() else {
        return;
    };
    let runtime = runtime.clone();
    tokio::spawn(async move {
        let members = match projects.list_members(&project_id).await {
            Ok(members) => members,
            Err(e) => {
                tracing::warn!(project_id = %project_id, error = ?e, "failed to list project members for completion push");
                return;
            }
        };
        let title = item_name;
        let body = if completed {
            "Marked complete".to_string()
        } else {
            "Marked incomplete".to_string()
        };
        let url = detail_url.as_deref().unwrap_or("/web/tasks");
        for member in members {
            if member.user.id == actor_user_id {
                continue;
            }
            push_to_user(&runtime, &member.user.id, &title, &body, url).await;
        }
    });
}

/// Notifies every other member of `project_id` that `author_user_id` just commented on
/// `item_name` — see docs/issues_and_features.md's "Add notifications for comments --
/// all team members notified of any comment by default". Called from
/// `service::comments::create_comment` after the comment is persisted. Same
/// no-op-if-unconfigured, fan-out-to-every-other-member, fire-and-forget shape as
/// `notify_completion_change` above (comments have no assignee to single out, so
/// there's no `notify_assignment`-style single-recipient variant). The author's display
/// name is resolved from the same `list_members` call used for fan-out, rather than a
/// separate `UserRepo` lookup — every commenter is by definition a project member.
pub fn notify_comment(
    projects: Arc<dyn ProjectRepo>,
    project_id: String,
    item_name: String,
    detail_url: Option<String>,
    author_user_id: String,
    comment_body: String,
) {
    let Some(runtime) = PushRuntime::get() else {
        return;
    };
    let runtime = runtime.clone();
    tokio::spawn(async move {
        let members = match projects.list_members(&project_id).await {
            Ok(members) => members,
            Err(e) => {
                tracing::warn!(project_id = %project_id, error = ?e, "failed to list project members for comment push");
                return;
            }
        };
        let author_name = members
            .iter()
            .find(|m| m.user.id == author_user_id)
            .map(|m| format!("{} {}", m.user.first_name, m.user.last_name))
            .unwrap_or_else(|| "Someone".to_string());
        let title = format!("{author_name} commented on {item_name}");
        let url = detail_url.as_deref().unwrap_or("/web/tasks");
        for member in members.iter().filter(|m| m.user.id != author_user_id) {
            push_to_user(&runtime, &member.user.id, &title, &comment_body, url).await;
        }
    });
}

/// Notifies `assignee_user_id` that `item_name` was just assigned to them — see
/// docs/issues_and_features.md's "Send notification on being assigned a task to the
/// assignee". Called from `service::team_items::create_team_item`/`update_team_item`
/// only on a genuine new-or-changed assignment (not on every edit of an
/// already-assigned item), and only when the assignee isn't the person making the
/// change. Same no-op-if-unconfigured, fire-and-forget shape as
/// `notify_completion_change` above.
pub fn notify_assignment(
    assignee_user_id: String,
    actor_user_id: String,
    item_name: String,
    detail_url: Option<String>,
) {
    if assignee_user_id == actor_user_id {
        return;
    }
    let Some(runtime) = PushRuntime::get() else {
        return;
    };
    let runtime = runtime.clone();
    tokio::spawn(async move {
        let url = detail_url.as_deref().unwrap_or("/web/tasks");
        push_to_user(
            &runtime,
            &assignee_user_id,
            "New assignment",
            &format!("Assigned to you: {item_name}"),
            url,
        )
        .await;
    });
}

/// The background sweep's single tick — see `docs/push-notifications-plan.md`. Called from a
/// `tokio::spawn` loop in `src/main.rs`, gated on `PushConfig::from_env().is_some()` (no loop
/// spawned at all if push isn't configured on this server, mirroring `EmailConfig`'s
/// "optional feature, absent if unconfigured" pattern). Fire-and-forget, like
/// `service::calendar_subscriptions::sync_all_subscriptions` — logs and continues on any
/// per-item/per-subscription failure rather than propagating a `Result`, since there's no
/// caller to report one to.
pub async fn sweep_due_reminders(
    reminders: &Arc<dyn ReminderRepo>,
    items: &Arc<dyn ItemRepo>,
    users: &Arc<dyn UserRepo>,
    push_subs: &Arc<dyn PushSubscriptionRepo>,
    http_client: &reqwest::Client,
    push_config: &PushConfig,
) {
    let due = match reminders.list_due_for_push(Utc::now()).await {
        Ok(due) => due,
        Err(e) => {
            tracing::error!(error = ?e, "failed to list due reminders for push sweep");
            return;
        }
    };

    // Small per-sweep caches — a user with several due reminders in one tick shouldn't
    // re-fetch their subscriptions/timezone once per reminder. Result sets here are small
    // by construction (see `service::reminders::list_due_notifications_for_user`'s identical
    // "small by construction" note), so a plain HashMap is enough.
    let mut tz_offset_cache: HashMap<String, i32> = HashMap::new();
    let mut subs_cache: HashMap<String, Vec<crate::domain::push_subscription::PushSubscription>> =
        HashMap::new();

    for reminder in due {
        let item = match items
            .get_by_project(&reminder.project_id, &reminder.item_id)
            .await
        {
            Ok(item) => item,
            Err(RepoError::NotFound) => {
                // Deleted since the reminder was created — mark pushed so the sweep stops
                // retrying it, mirroring `list_due_notifications_for_user`'s defensive skip.
                let _ = reminders.mark_pushed(&reminder.id).await;
                continue;
            }
            Err(e) => {
                tracing::warn!(reminder_id = %reminder.id, error = ?e, "failed to fetch item for push sweep");
                continue;
            }
        };
        if item.complete {
            let _ = reminders.mark_pushed(&reminder.id).await;
            continue;
        }

        let tz_offset = *match tz_offset_cache.get(&reminder.user_id) {
            Some(offset) => offset,
            None => {
                let offset = resolve_tz_offset_minutes(users, &reminder.user_id).await;
                tz_offset_cache.insert(reminder.user_id.clone(), offset);
                tz_offset_cache.get(&reminder.user_id).unwrap()
            }
        };
        let label = reminder_labels(std::slice::from_ref(&reminder), tz_offset)
            .into_iter()
            .next()
            .unwrap_or_default();
        let Some(detail_url) = detail_url(&item, &reminder.project_id) else {
            let _ = reminders.mark_pushed(&reminder.id).await;
            continue;
        };

        let subs = match subs_cache.get(&reminder.user_id) {
            Some(subs) => subs.clone(),
            None => {
                let subs = match push_subs.list_for_user(&reminder.user_id).await {
                    Ok(subs) => subs,
                    Err(e) => {
                        tracing::warn!(user_id = %reminder.user_id, error = ?e, "failed to list push subscriptions");
                        Vec::new()
                    }
                };
                subs_cache.insert(reminder.user_id.clone(), subs.clone());
                subs
            }
        };

        for sub in &subs {
            let outcome = send_push(
                http_client,
                push_config,
                sub,
                &item.name,
                &label,
                &detail_url,
            )
            .await;
            match outcome {
                PushSendOutcome::Delivered => {}
                PushSendOutcome::Gone => {
                    let _ = push_subs.delete_by_endpoint(&sub.endpoint).await;
                }
                PushSendOutcome::Failed(e) => {
                    tracing::warn!(
                        subscription_id = %sub.id,
                        reminder_id = %reminder.id,
                        error = %e,
                        "push send failed"
                    );
                }
            }
        }

        // Marked pushed once per reminder regardless of per-subscription outcome — best
        // effort, matching the CSV import precedent of not blocking on partial failure. A
        // user with zero subscriptions is correctly marked pushed too: there's nothing to
        // retry.
        if let Err(e) = reminders.mark_pushed(&reminder.id).await {
            tracing::error!(reminder_id = %reminder.id, error = ?e, "failed to mark reminder pushed");
        }
    }
}

/// Task/Event only, mirroring `service::reminders::detail_url` — duplicated per that file's
/// own "small per-purpose helper" precedent rather than shared, since this one operates
/// globally across users rather than per-request. `pub(crate)` so `service::team_items` can
/// reuse it for `notify_completion_change`/`notify_assignment`'s own links, rather than a
/// third duplicate of the same two-line match.
pub(crate) fn detail_url(item: &crate::domain::item::Item, project_id: &str) -> Option<String> {
    use crate::domain::item::ItemKind;
    match item.kind() {
        ItemKind::Task => Some(format!("/web/projects/{project_id}/tasks/{}", item.id)),
        ItemKind::Event => Some(format!("/web/projects/{project_id}/events/{}", item.id)),
        ItemKind::Simple | ItemKind::Template => None,
    }
}

/// Resolves `user_id`'s IANA timezone (`users.timezone`, still unset for most users today —
/// see CLAUDE.md's Scheduled start/end section) to an offset-minutes value for
/// `reminder_labels`, falling back to UTC (0) when unset or unparseable — the same
/// `Tz::from_str(tz).ok()` pattern `service::calendar_sync` already uses. A background sweep
/// has no live `X-Tz-Offset-Minutes` header to fall back to the way a request handler does
/// (`TzOffset`, `src/web_ui/mod.rs`), so this is the best available signal.
async fn resolve_tz_offset_minutes(users: &Arc<dyn UserRepo>, user_id: &str) -> i32 {
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
    use crate::domain::item::Item;
    use crate::domain::reminder::{Reminder, ReminderKind};
    use crate::push::PushConfig;
    use crate::storage::sqlite::{
        MockItemRepo, MockPushSubscriptionRepo, MockReminderRepo, MockUserRepo,
    };
    use base64ct::{Base64UrlUnpadded, Encoding as _};
    use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair};

    fn ts(secs: i64) -> chrono::DateTime<Utc> {
        chrono::DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn due_reminder(id: &str, item_id: &str, user_id: &str) -> Reminder {
        Reminder {
            id: id.to_string(),
            item_id: item_id.to_string(),
            project_id: "proj1".to_string(),
            user_id: user_id.to_string(),
            kind: ReminderKind::Due,
            source: "AUTO".to_string(),
            remind_at: ts(1_000),
            sent_at: None,
            push_sent_at: None,
            created_at: ts(0),
        }
    }

    fn test_push_config() -> PushConfig {
        let key_pair = ES256KeyPair::generate();
        PushConfig {
            vapid_public_key_b64url: Base64UrlUnpadded::encode_string(
                &key_pair.public_key().public_key().to_bytes_uncompressed(),
            ),
            vapid_key_pair: key_pair,
            subject: "mailto:test@example.com".to_string(),
        }
    }

    #[tokio::test]
    async fn completed_items_reminder_is_marked_pushed_without_sending() {
        let reminder = due_reminder("r1", "item1", "user1");
        let mut reminders = MockReminderRepo::new();
        reminders
            .expect_list_due_for_push()
            .returning(move |_| Ok(vec![reminder.clone()]));
        reminders
            .expect_mark_pushed()
            .withf(|id| id == "r1")
            .returning(|_| Ok(()));

        let mut items = MockItemRepo::new();
        items.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                complete: true,
                ..Item::new_project_item("proj1", "Task")
            })
        });

        let users = MockUserRepo::new();
        // No subscriptions call expected — a completed item is skipped before that point.
        let push_subs = MockPushSubscriptionRepo::new();

        sweep_due_reminders(
            &(Arc::new(reminders) as Arc<dyn ReminderRepo>),
            &(Arc::new(items) as Arc<dyn ItemRepo>),
            &(Arc::new(users) as Arc<dyn UserRepo>),
            &(Arc::new(push_subs) as Arc<dyn PushSubscriptionRepo>),
            &reqwest::Client::new(),
            &test_push_config(),
        )
        .await;
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
                personal_project_id: None,
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

    #[tokio::test]
    async fn due_incomplete_item_with_no_subscriptions_still_gets_marked_pushed() {
        let reminder = due_reminder("r1", "item1", "user1");
        let mut reminders = MockReminderRepo::new();
        reminders
            .expect_list_due_for_push()
            .returning(move |_| Ok(vec![reminder.clone()]));
        reminders
            .expect_mark_pushed()
            .withf(|id| id == "r1")
            .returning(|_| Ok(()));

        let mut items = MockItemRepo::new();
        items
            .expect_get_by_project()
            .returning(|_, _| Ok(Item::new_project_item("proj1", "Task")));

        let mut users = MockUserRepo::new();
        users.expect_get().returning(|_| Err(RepoError::NotFound));

        let mut push_subs = MockPushSubscriptionRepo::new();
        push_subs.expect_list_for_user().returning(|_| Ok(vec![]));

        sweep_due_reminders(
            &(Arc::new(reminders) as Arc<dyn ReminderRepo>),
            &(Arc::new(items) as Arc<dyn ItemRepo>),
            &(Arc::new(users) as Arc<dyn UserRepo>),
            &(Arc::new(push_subs) as Arc<dyn PushSubscriptionRepo>),
            &reqwest::Client::new(),
            &test_push_config(),
        )
        .await;
    }
}
