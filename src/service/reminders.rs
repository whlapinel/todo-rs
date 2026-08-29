use crate::domain::item::{Item, ItemKind};
use crate::domain::reminder::{Reminder, ReminderKind};
use crate::service::error::ItemError;
use crate::storage::sqlite::{ItemRepo, ProjectRepo, ReminderRepo, RepoError};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Recomputes `item`'s auto-generated reminders from its current state — called after
/// every successful create/update in `service::project_items`'s funnel
/// (`create_project_item`/`update_project_item`), and via `delete_for_item` directly on
/// delete. Always a full resync (delete-then-reinsert via `ReminderRepo::sync_auto_reminders`),
/// not a diff — cheap, and correct regardless of which fields actually changed, mirroring
/// `sync_offset_children`'s own "just recompute" precedent (`src/service/items.rs`)
/// rather than tracking a delta.
///
/// Stage 1 of the reminders feature (docs/issues_and_features.md): "the instant it's
/// scheduled/due," no user-configurable offset, no mutation UI. Recipient resolution
/// mirrors the existing points precedent (CLAUDE.md's Points section) — personal
/// project's single owner, or a team-backed item's `assigned_to_user_id`, skipping
/// (clearing any existing reminders) rather than fanning out to the whole project when
/// unassigned.
pub async fn sync_item_reminders(
    reminders: &Arc<dyn ReminderRepo>,
    projects: &Arc<dyn ProjectRepo>,
    item: &Item,
) -> Result<(), ItemError> {
    // Simple has no date fields at all (Item::validate rejects them); Template is a
    // library blueprint, not a real commitment — neither gets reminders.
    if !matches!(item.kind(), ItemKind::Task | ItemKind::Event) {
        reminders.delete_for_item(&item.id).await?;
        return Ok(());
    }

    // A completed item has nothing left to remind about — clear any reminder rows
    // rather than letting them go stale. Un-completing recomputes fresh rows the next
    // time this function runs (the create/update funnel always calls it).
    if item.complete {
        reminders.delete_for_item(&item.id).await?;
        return Ok(());
    }

    let Some(project_id) = item.project_id.clone() else {
        // Every item reaching this function was created/updated via the project-scoped
        // funnel, so project_id is always set in practice — this branch only guards
        // against a hypothetical legacy row with no project_id at all.
        reminders.delete_for_item(&item.id).await?;
        return Ok(());
    };
    let project = projects.get(&project_id).await?;
    let recipient = match &project.team_id {
        Some(_) => item.assigned_to_user_id(),
        None => Some(project.owner_user_id.clone()),
    };
    let Some(user_id) = recipient else {
        // Unassigned team item — no one to notify. If it was previously assigned and
        // had reminders, clear them; if it never had any, this is a no-op delete.
        reminders.delete_for_item(&item.id).await?;
        return Ok(());
    };

    let mut rows: Vec<(ReminderKind, chrono::DateTime<chrono::Utc>)> = Vec::new();
    if let Some(due) = item.due_date() {
        rows.push((ReminderKind::Due, due));
    }
    if let Some(start) = item.scheduled_date() {
        rows.push((ReminderKind::ScheduledStart, start));
    }
    if let Some(end) = item.scheduled_end_date() {
        rows.push((ReminderKind::ScheduledEnd, end));
    }

    reminders
        .sync_auto_reminders(&item.id, &project_id, &user_id, &rows)
        .await?;
    Ok(())
}

/// One due, undismissed reminder ready to show in the notification badge/list — see
/// `list_due_notifications_for_user` below.
pub struct DueNotification {
    pub reminder: Reminder,
    pub item_name: String,
    pub detail_url: String,
}

/// Task/Event only — `sync_item_reminders` never creates rows for Simple/Template, but this
/// stays defensive rather than assuming that invariant holds forever. Mirrors
/// `web_ui::assigned_items::detail_url`, duplicated per that file's own "small per-screen
/// helper" precedent rather than shared.
fn detail_url(item: &Item, project_id: &str) -> Option<String> {
    match item.kind() {
        ItemKind::Task => Some(format!("/web/projects/{project_id}/tasks/{}", item.id)),
        ItemKind::Event => Some(format!("/web/projects/{project_id}/events/{}", item.id)),
        ItemKind::Simple | ItemKind::Template => None,
    }
}

/// Fetches `user_id`'s due-and-undismissed reminders (`ReminderRepo::list_due_for_user`) and
/// filters out anything that shouldn't actually show: a completed item (`sync_item_reminders`
/// clears a completed item's reminder rows going forward, but this stays as a defensive
/// read-time filter for any row that predates that fix) or an item that's been deleted since
/// the reminder was created (`delete_for_item` should already prevent this in the normal path,
/// but don't assume).
pub async fn list_due_notifications_for_user(
    reminders: &Arc<dyn ReminderRepo>,
    items: &Arc<dyn ItemRepo>,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<DueNotification>, ItemError> {
    let due = reminders.list_due_for_user(user_id, now).await?;
    let mut out = Vec::with_capacity(due.len());
    for reminder in due {
        let item = match items
            .get_by_project(&reminder.project_id, &reminder.item_id)
            .await
        {
            Ok(item) => item,
            Err(RepoError::NotFound) => continue,
            Err(e) => return Err(e.into()),
        };
        if item.complete {
            continue;
        }
        let Some(detail_url) = detail_url(&item, &reminder.project_id) else {
            continue;
        };
        out.push(DueNotification {
            item_name: item.name.clone(),
            detail_url,
            reminder,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{Item, ItemType, Recurrence, Schedule, TeamAssignment};
    use crate::domain::project::Project;
    use crate::storage::sqlite::{MockProjectRepo, MockReminderRepo};
    use chrono::{DateTime, Utc};

    fn ts(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn task_item(
        project_id: &str,
        schedule: Schedule,
        team_assignment: Option<TeamAssignment>,
    ) -> Item {
        Item {
            id: "item1".to_string(),
            project_id: Some(project_id.to_string()),
            item_type: ItemType::Task {
                schedule,
                recurrence: Recurrence::default(),
                team_assignment,
                source_event_id: None,
                priority: None,
            },
            ..Item::new_project_item(project_id, "Task")
        }
    }

    fn personal_project(id: &str, owner: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "Personal".to_string(),
            owner_user_id: owner.to_string(),
            team_id: None,
        }
    }

    fn team_project(id: &str, owner: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "Team".to_string(),
            owner_user_id: owner.to_string(),
            team_id: Some("team1".to_string()),
        }
    }

    #[tokio::test]
    async fn personal_project_item_reminds_the_owner() {
        let schedule = Schedule {
            due_date: Some(ts(1_000)),
            ..Schedule::default()
        };
        let item = task_item("proj1", schedule, None);

        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .withf(|id| id == "proj1")
            .returning(|_| Ok(personal_project("proj1", "owner1")));

        let mut reminder_repo = MockReminderRepo::new();
        reminder_repo
            .expect_sync_auto_reminders()
            .withf(|item_id, project_id, user_id, rows| {
                item_id == "item1"
                    && project_id == "proj1"
                    && user_id == "owner1"
                    && rows == [(ReminderKind::Due, ts(1_000))]
            })
            .returning(|_, _, _, _| Ok(()));

        sync_item_reminders(
            &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &item,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn team_project_item_reminds_the_assignee() {
        let schedule = Schedule {
            scheduled_date: Some(ts(2_000)),
            ..Schedule::default()
        };
        let item = task_item(
            "proj1",
            schedule,
            Some(TeamAssignment {
                assigned_to_user_id: Some("assignee1".to_string()),
                points: None,
            }),
        );

        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|_| Ok(team_project("proj1", "owner1")));

        let mut reminder_repo = MockReminderRepo::new();
        reminder_repo
            .expect_sync_auto_reminders()
            .withf(|_, _, user_id, rows| {
                user_id == "assignee1" && rows == [(ReminderKind::ScheduledStart, ts(2_000))]
            })
            .returning(|_, _, _, _| Ok(()));

        sync_item_reminders(
            &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &item,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unassigned_team_project_item_clears_reminders() {
        let schedule = Schedule {
            due_date: Some(ts(1_000)),
            ..Schedule::default()
        };
        let item = task_item("proj1", schedule, None);

        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|_| Ok(team_project("proj1", "owner1")));

        let mut reminder_repo = MockReminderRepo::new();
        reminder_repo
            .expect_delete_for_item()
            .withf(|id| id == "item1")
            .returning(|_| Ok(()));

        sync_item_reminders(
            &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &item,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn simple_item_clears_reminders_without_touching_projects() {
        let item = Item {
            id: "item1".to_string(),
            project_id: Some("proj1".to_string()),
            item_type: ItemType::Simple,
            ..Item::new_project_item("proj1", "Simple")
        };

        let projects = MockProjectRepo::new();
        let mut reminder_repo = MockReminderRepo::new();
        reminder_repo
            .expect_delete_for_item()
            .withf(|id| id == "item1")
            .returning(|_| Ok(()));

        sync_item_reminders(
            &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &item,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn completed_task_item_clears_reminders_without_touching_projects() {
        let schedule = Schedule {
            due_date: Some(ts(1_000)),
            ..Schedule::default()
        };
        let mut item = task_item("proj1", schedule, None);
        item.complete = true;

        let projects = MockProjectRepo::new();
        let mut reminder_repo = MockReminderRepo::new();
        reminder_repo
            .expect_delete_for_item()
            .withf(|id| id == "item1")
            .returning(|_| Ok(()));

        sync_item_reminders(
            &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &item,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn all_three_date_kinds_produce_independent_rows() {
        let schedule = Schedule {
            due_date: Some(ts(1_000)),
            scheduled_date: Some(ts(2_000)),
            scheduled_end_date: Some(ts(3_000)),
            ..Schedule::default()
        };
        let item = task_item("proj1", schedule, None);

        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|_| Ok(personal_project("proj1", "owner1")));

        let mut reminder_repo = MockReminderRepo::new();
        reminder_repo
            .expect_sync_auto_reminders()
            .withf(|_, _, _, rows| {
                rows == [
                    (ReminderKind::Due, ts(1_000)),
                    (ReminderKind::ScheduledStart, ts(2_000)),
                    (ReminderKind::ScheduledEnd, ts(3_000)),
                ]
            })
            .returning(|_, _, _, _| Ok(()));

        sync_item_reminders(
            &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &item,
        )
        .await
        .unwrap();
    }

    mod list_due_notifications {
        use super::*;
        use crate::storage::sqlite::MockItemRepo;

        fn due_reminder(item_id: &str, kind: ReminderKind) -> Reminder {
            Reminder {
                id: format!("reminder-{item_id}"),
                item_id: item_id.to_string(),
                project_id: "proj1".to_string(),
                user_id: "user1".to_string(),
                kind,
                source: "AUTO".to_string(),
                remind_at: ts(1_000),
                sent_at: None,
                push_sent_at: None,
                created_at: ts(0),
            }
        }

        #[tokio::test]
        async fn builds_a_notification_for_an_incomplete_task() {
            let mut reminder_repo = MockReminderRepo::new();
            reminder_repo
                .expect_list_due_for_user()
                .returning(|_, _| Ok(vec![due_reminder("item1", ReminderKind::Due)]));

            let mut items = MockItemRepo::new();
            items.expect_get_by_project().returning(|_, _| {
                Ok(task_item(
                    "proj1",
                    Schedule {
                        due_date: Some(ts(1_000)),
                        ..Schedule::default()
                    },
                    None,
                ))
            });

            let out = list_due_notifications_for_user(
                &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
                &(Arc::new(items) as Arc<dyn ItemRepo>),
                "user1",
                ts(2_000),
            )
            .await
            .unwrap();

            assert_eq!(out.len(), 1);
            assert_eq!(out[0].detail_url, "/web/projects/proj1/tasks/item1");
            assert_eq!(out[0].item_name, "Task");
        }

        #[tokio::test]
        async fn skips_a_completed_item() {
            let mut reminder_repo = MockReminderRepo::new();
            reminder_repo
                .expect_list_due_for_user()
                .returning(|_, _| Ok(vec![due_reminder("item1", ReminderKind::Due)]));

            let mut items = MockItemRepo::new();
            items.expect_get_by_project().returning(|_, _| {
                let mut item = task_item(
                    "proj1",
                    Schedule {
                        due_date: Some(ts(1_000)),
                        ..Schedule::default()
                    },
                    None,
                );
                item.complete = true;
                Ok(item)
            });

            let out = list_due_notifications_for_user(
                &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
                &(Arc::new(items) as Arc<dyn ItemRepo>),
                "user1",
                ts(2_000),
            )
            .await
            .unwrap();

            assert!(out.is_empty());
        }

        #[tokio::test]
        async fn skips_an_item_deleted_since_the_reminder_was_created() {
            let mut reminder_repo = MockReminderRepo::new();
            reminder_repo
                .expect_list_due_for_user()
                .returning(|_, _| Ok(vec![due_reminder("item1", ReminderKind::Due)]));

            let mut items = MockItemRepo::new();
            items
                .expect_get_by_project()
                .returning(|_, _| Err(RepoError::NotFound));

            let out = list_due_notifications_for_user(
                &(Arc::new(reminder_repo) as Arc<dyn ReminderRepo>),
                &(Arc::new(items) as Arc<dyn ItemRepo>),
                "user1",
                ts(2_000),
            )
            .await
            .unwrap();

            assert!(out.is_empty());
        }
    }
}
