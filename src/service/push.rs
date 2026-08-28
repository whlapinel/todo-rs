use crate::push::{PushConfig, PushSendOutcome, send_push};
use crate::storage::sqlite::{ItemRepo, PushSubscriptionRepo, ReminderRepo, RepoError, UserRepo};
use crate::web_ui::reminder_labels;
use chrono::{Offset, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

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
/// globally across users rather than per-request.
fn detail_url(item: &crate::domain::item::Item, project_id: &str) -> Option<String> {
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
    Utc::now()
        .with_timezone(&tz)
        .offset()
        .fix()
        .local_minus_utc()
        / 60
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
