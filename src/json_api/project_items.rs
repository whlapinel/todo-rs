use super::{internal, not_found, to_domain_item_type, to_sdk_item_type};
use crate::auth::AuthUser;
use crate::domain::item::{ItemKind, Schedule, TeamAssignment};
use crate::service::item_input::{
    EditEvent, EditItem, EditItemKind, EditSimple, EditTask, EditTemplate, NewEvent, NewItem,
    NewItemKind, NewSimple, NewTask, NewTemplate, reject_event_due_date, reject_field,
    reject_simple_only_fields, reject_task_only_fields, task_anchor_from_fields, template_parent,
};
use crate::service::items::ItemError;
use crate::service::project_items::{self as project_item_service};
use crate::service::projects::require_project_member;
use crate::storage::sqlite::{
    ActivityLogRepo, ItemDependencyRepo, ItemRepo, ItemSeriesRepo, ProjectRepo, ReminderRepo,
    RepoError, TeamRepo, UserRepo,
};
use std::collections::HashMap;
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server, types::DateTime as SmithyDateTime};

fn to_create_project_item_error(e: ItemError) -> error::CreateProjectItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

fn to_delete_project_item_error(e: ItemError) -> error::DeleteProjectItemError {
    match e {
        ItemError::NotFound => not_found().into(),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => internal(msg).into(),
    }
}

fn to_utc(dt: Option<SmithyDateTime>) -> Option<chrono::DateTime<chrono::Utc>> {
    dt.and_then(|dt| chrono::DateTime::from_timestamp(dt.secs(), dt.subsec_nanos()))
        .map(|d| d.with_timezone(&chrono::Utc))
}

fn try_into_new_item(input: input::CreateProjectItemInput) -> Result<NewItem, ItemError> {
    // An omitted `itemType` means Task on create — `ItemKind`'s own `#[default]`, which is
    // what the flat params this replaced resolved an omitted kind to as well. (Update is
    // different: there, omitting it means "leave the kind unchanged", so the caller resolves
    // it from the stored item before building an `EditItem` at all.)
    let kind = to_domain_item_type(input.item_type).unwrap_or_default();
    let schedule = Schedule {
        due_date: to_utc(input.due_date),
        has_due_time: input.has_due_time.unwrap_or(false),
        scheduled_date: to_utc(input.scheduled_date),
        has_scheduled_time: input.has_scheduled_time.unwrap_or(false),
        scheduled_end_date: to_utc(input.scheduled_end_date),
        has_end_time: input.has_end_time.unwrap_or(false),
    };
    let payload = match kind {
        ItemKind::Task => {
            reject_field(kind, "eventType", &input.event_type)?;
            NewItemKind::Task(NewTask {
                anchor: task_anchor_from_fields(input.parent_item_id, input.source_event_id)?,
                schedule,
                due_offset_days: input.due_offset_days,
                priority: input.priority,
                complete: input.complete.unwrap_or(false),
                assignment: TeamAssignment {
                    assigned_to_user_id: input.assigned_to_user_id,
                    points: input.points,
                },
                series_id: None,
                source_template_id: None,
            })
        }
        ItemKind::Event => {
            // An Event structurally cannot be a child of anything, so `parentItemId` is
            // rejected here rather than lumped in with the Task-only set.
            reject_field(kind, "parentItemId", &input.parent_item_id)?;
            reject_task_only_fields(
                kind,
                input.complete,
                &input.priority,
                &input.points,
                &input.assigned_to_user_id,
                &input.source_event_id,
            )?;
            reject_event_due_date(&schedule)?;
            NewItemKind::Event(NewEvent {
                schedule,
                event_type: input.event_type,
                due_offset_days: input.due_offset_days,
                series_id: None,
            })
        }
        ItemKind::Simple => {
            reject_simple_only_fields(&input.event_type, &input.due_offset_days, &schedule)?;
            reject_task_only_fields(
                kind,
                input.complete,
                &input.priority,
                &input.points,
                &input.assigned_to_user_id,
                &input.source_event_id,
            )?;
            NewItemKind::Simple(NewSimple {
                parent_item_id: input.parent_item_id,
            })
        }
        ItemKind::Template => {
            reject_task_only_fields(
                kind,
                input.complete,
                &input.priority,
                &input.points,
                &input.assigned_to_user_id,
                &input.source_event_id,
            )?;
            NewItemKind::Template(NewTemplate {
                parent_item_id: template_parent(input.parent_item_id)?,
                schedule,
                event_type: input.event_type,
                due_offset_days: input.due_offset_days,
            })
        }
    };
    Ok(NewItem {
        project_id: input.project_id,
        name: input.name,
        description: input.description,
        timezone_offset_minutes: input.timezone_offset_minutes,
        kind: payload,
    })
}

/// `kind` is resolved by the caller, not read off the input: `itemType` is optional on
/// update and omitting it means "leave the kind unchanged", which only the stored item
/// knows. See `update_project_item` below for why that read has to follow the membership
/// check.
fn try_into_edit_item(
    input: input::UpdateProjectItemInput,
    kind: ItemKind,
) -> Result<EditItem, ItemError> {
    let schedule = Schedule {
        due_date: to_utc(input.due_date),
        has_due_time: input.has_due_time.unwrap_or(false),
        scheduled_date: to_utc(input.scheduled_date),
        has_scheduled_time: input.has_scheduled_time.unwrap_or(false),
        scheduled_end_date: to_utc(input.scheduled_end_date),
        has_end_time: input.has_end_time.unwrap_or(false),
    };
    let payload = match kind {
        ItemKind::Task => {
            reject_field(kind, "eventType", &input.event_type)?;
            EditItemKind::Task(EditTask {
                anchor: task_anchor_from_fields(input.parent_item_id, input.source_event_id)?,
                schedule,
                due_offset_days: input.due_offset_days,
                priority: input.priority,
                complete: input.complete,
                assignment: TeamAssignment {
                    assigned_to_user_id: input.assigned_to_user_id,
                    points: input.points,
                },
            })
        }
        ItemKind::Event => {
            reject_field(kind, "parentItemId", &input.parent_item_id)?;
            reject_task_only_fields(
                kind,
                Some(input.complete),
                &input.priority,
                &input.points,
                &input.assigned_to_user_id,
                &input.source_event_id,
            )?;
            reject_event_due_date(&schedule)?;
            EditItemKind::Event(EditEvent {
                schedule,
                event_type: input.event_type,
                due_offset_days: input.due_offset_days,
            })
        }
        ItemKind::Simple => {
            reject_simple_only_fields(&input.event_type, &input.due_offset_days, &schedule)?;
            reject_task_only_fields(
                kind,
                Some(input.complete),
                &input.priority,
                &input.points,
                &input.assigned_to_user_id,
                &input.source_event_id,
            )?;
            EditItemKind::Simple(EditSimple {
                parent_item_id: input.parent_item_id,
            })
        }
        ItemKind::Template => {
            reject_task_only_fields(
                kind,
                Some(input.complete),
                &input.priority,
                &input.points,
                &input.assigned_to_user_id,
                &input.source_event_id,
            )?;
            EditItemKind::Template(EditTemplate {
                parent_item_id: template_parent(input.parent_item_id)?,
                schedule,
                event_type: input.event_type,
                due_offset_days: input.due_offset_days,
            })
        }
    };
    Ok(EditItem {
        project_id: input.project_id,
        item_id: input.item_id,
        name: input.name,
        description: input.description,
        timezone_offset_minutes: input.timezone_offset_minutes,
        depends_on_item_ids: input.depends_on_item_ids,
        kind: payload,
    })
}

pub async fn create_project_item(
    input: input::CreateProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(reminders): server::Extension<Arc<dyn ReminderRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::CreateProjectItemOutput, error::CreateProjectItemError> {
    // Purely a function of the request — `itemType` is either given or defaults to Task, so
    // nothing has to be read to know the kind, and a rejection here reveals only that the
    // caller's own request was self-inconsistent. The update path below cannot say the same.
    let new = try_into_new_item(input).map_err(to_create_project_item_error)?;
    let item_id = project_item_service::create_project_item(
        &repo,
        &projects,
        &teams,
        &reminders,
        &auth.user_id,
        new,
    )
    .await
    .map_err(to_create_project_item_error)?;
    Ok(output::CreateProjectItemOutput { item_id })
}

pub async fn get_project_item(
    input: input::GetProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(item_dependencies): server::Extension<Arc<dyn ItemDependencyRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::GetProjectItemOutput, error::GetProjectItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &input.project_id,
        &auth.user_id,
        &input.item_id,
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::GetProjectItemError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::GetProjectItemError::from(internal(msg))
        }
    })?;
    let depends_on_item_ids =
        item_dependencies
            .list_for_item(&item.id)
            .await
            .map_err(|e| match e {
                RepoError::NotFound => error::GetProjectItemError::from(not_found()),
                RepoError::Internal(msg) => error::GetProjectItemError::from(internal(msg)),
            })?;
    let due_date = item
        .due_date()
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    let scheduled_date = item
        .scheduled_date()
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    let scheduled_end_date = item
        .scheduled_end_date()
        .map(|dt| SmithyDateTime::from_secs(dt.timestamp()));
    Ok(output::GetProjectItemOutput {
        name: item.name.clone(),
        description: item.description.clone(),
        due_date,
        scheduled_date,
        scheduled_end_date,
        complete: item.complete(),
        has_due_time: Some(item.has_due_time()),
        has_scheduled_time: Some(item.has_scheduled_time()),
        has_end_time: Some(item.has_end_time()),
        parent_item_id: item.parent_item_id(),
        has_children: Some(item.has_children),
        item_type: Some(to_sdk_item_type(item.kind())),
        event_type: item.event_type(),
        due_offset_days: item.due_offset_days(),
        assigned_to_user_id: item.assigned_to_user_id(),
        points: item.points(),
        priority: item.priority(),
        source_event_id: item.source_event_id(),
        google_event_id: item.google_event_id(),
        calendar_subscription_id: item.calendar_subscription_id(),
        depends_on_item_ids: Some(depends_on_item_ids),
    })
}

pub async fn update_project_item(
    input: input::UpdateProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(activity_log): server::Extension<Arc<dyn ActivityLogRepo>>,
    server::Extension(series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(reminders): server::Extension<Arc<dyn ReminderRepo>>,
    server::Extension(item_dependencies): server::Extension<Arc<dyn ItemDependencyRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::UpdateProjectItemOutput, error::UpdateProjectItemError> {
    let to_error = |e: ItemError| match e {
        ItemError::NotFound => error::UpdateProjectItemError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::UpdateProjectItemError::from(internal(msg))
        }
    };
    // `itemType` is optional on update and omitting it means "leave the kind unchanged" —
    // `items::update_item`'s `params.item_type.unwrap_or(current.kind())`. A typed
    // `EditItemKind` has to *be* some kind, so when the request doesn't say, the stored item
    // is the only thing that knows.
    //
    // That read is gated on the membership check, not merely followed by one. Reading first
    // and rejecting afterwards would let a non-member tell "this item exists in that project"
    // (a cross-kind field error) from "it doesn't" (not found) by deliberately sending a bad
    // field — `update_project_item` re-checks membership anyway, so the check here is redundant
    // for authorization and load-bearing only for that ordering. The extra queries are the
    // price of the kind-optional wire contract; a request that states its `itemType` pays
    // nothing.
    let kind = match to_domain_item_type(input.item_type.clone()) {
        Some(kind) => kind,
        None => {
            require_project_member(&projects, &teams, &input.project_id, &auth.user_id)
                .await
                .map_err(to_error)?;
            repo.get_by_project(&input.project_id, &input.item_id)
                .await
                .map_err(ItemError::from)
                .map_err(to_error)?
                .kind()
        }
    };
    let edit = try_into_edit_item(input, kind).map_err(to_error)?;
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &reminders,
        &item_dependencies,
        &auth.user_id,
        edit,
    )
    .await
    .map_err(to_error)?;
    Ok(output::UpdateProjectItemOutput {})
}

pub async fn delete_project_item(
    input: input::DeleteProjectItemInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(series): server::Extension<Arc<dyn ItemSeriesRepo>>,
    server::Extension(reminders): server::Extension<Arc<dyn ReminderRepo>>,
    server::Extension(item_dependencies): server::Extension<Arc<dyn ItemDependencyRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::DeleteProjectItemOutput, error::DeleteProjectItemError> {
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &series,
        &reminders,
        &item_dependencies,
        &auth.user_id,
        &input.project_id,
        &input.item_id,
    )
    .await
    .map_err(to_delete_project_item_error)?;
    Ok(output::DeleteProjectItemOutput {})
}

pub async fn list_project_items(
    input: input::ListProjectItemsInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(users): server::Extension<Arc<dyn UserRepo>>,
    server::Extension(auth): server::Extension<AuthUser>,
) -> Result<output::ListProjectItemsOutput, error::ListProjectItemsError> {
    let items = project_item_service::list_project_items(
        &repo,
        &projects,
        &teams,
        &input.project_id,
        &auth.user_id,
        input.parent_item_id,
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::ListProjectItemsError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::ListProjectItemsError::from(internal(msg))
        }
    })?;
    let mut names = HashMap::<String, String>::new();
    for item in items.iter() {
        if let Some(id) = item.assigned_to_user_id() {
            match get_user_name(&id, &users).await {
                Some(name) => {
                    names.insert(id, name);
                }
                None => {
                    tracing::error!(
                        "unable to map assigned user id to assigned username: get_user_name returned None"
                    );
                }
            }
        }
    }
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::ProjectItemSummary {
            item_id: Some(i.id.clone()),
            name: Some(i.name.clone()),
            description: i.description.clone(),
            due_date: i
                .due_date()
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            scheduled_date: i
                .scheduled_date()
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            scheduled_end_date: i
                .scheduled_end_date()
                .map(|dt| SmithyDateTime::from_secs(dt.timestamp())),
            complete: Some(i.complete()),
            has_due_time: Some(i.has_due_time()),
            has_scheduled_time: Some(i.has_scheduled_time()),
            has_end_time: Some(i.has_end_time()),
            parent_item_id: i.parent_item_id(),
            has_children: Some(i.has_children),
            item_type: Some(to_sdk_item_type(i.kind())),
            event_type: i.event_type(),
            due_offset_days: i.due_offset_days(),
            assigned_to_user_id: i.assigned_to_user_id(),
            assigned_to_user_name: i
                .assigned_to_user_id()
                .map(|id| names.get(&id).unwrap_or(&"<Name>".to_string()).clone()),
            points: i.points(),
            priority: i.priority(),
            source_event_id: i.source_event_id(),
            google_event_id: i.google_event_id(),
            calendar_subscription_id: i.calendar_subscription_id(),
        })
        .collect();
    Ok(output::ListProjectItemsOutput { items })
}

async fn get_user_name(id: &str, user_repo: &Arc<dyn UserRepo>) -> Option<String> {
    let user = user_repo
        .get(&id)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => {
                tracing::error!("error: id {} not found", id);
            }
            RepoError::Internal(s) => {
                tracing::error!("internal error: {s}");
            }
        })
        .ok()?;
    Some(format!("{} {}", user.first_name, user.last_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests assert on a resolved anchor, so this stays out of the module's own
    // imports rather than riding along unused in a non-test build.
    use crate::service::item_input::TaskAnchor;
    use todo_server_sdk::model::ItemType as WireItemType;

    fn create_input(kind: Option<WireItemType>) -> input::CreateProjectItemInput {
        input::CreateProjectItemInput {
            project_id: "p1".to_string(),
            name: "n".to_string(),
            description: None,
            due_date: None,
            scheduled_date: None,
            scheduled_end_date: None,
            complete: None,
            has_due_time: None,
            has_scheduled_time: None,
            has_end_time: None,
            parent_item_id: None,
            item_type: kind,
            event_type: None,
            due_offset_days: None,
            assigned_to_user_id: None,
            points: None,
            priority: None,
            source_event_id: None,
            timezone_offset_minutes: None,
        }
    }

    fn update_input(kind: Option<WireItemType>) -> input::UpdateProjectItemInput {
        input::UpdateProjectItemInput {
            project_id: "p1".to_string(),
            item_id: "i1".to_string(),
            name: "n".to_string(),
            description: None,
            due_date: None,
            scheduled_date: None,
            scheduled_end_date: None,
            complete: false,
            has_due_time: None,
            has_scheduled_time: None,
            has_end_time: None,
            parent_item_id: None,
            item_type: kind,
            event_type: None,
            due_offset_days: None,
            assigned_to_user_id: None,
            points: None,
            priority: None,
            source_event_id: None,
            depends_on_item_ids: None,
            timezone_offset_minutes: None,
        }
    }

    fn invalid_message(e: ItemError) -> String {
        match e {
            ItemError::Invalid(msg) => msg,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// The behavior change Stage 7 exists to make. Every one of these used to succeed, drop
    /// the field, and return a `201`/`200` that told the caller nothing.
    #[test]
    fn cross_kind_fields_are_rejected_rather_than_dropped() {
        let mut points_on_event = create_input(Some(WireItemType::Event));
        points_on_event.points = Some(5);
        assert_eq!(
            invalid_message(try_into_new_item(points_on_event).unwrap_err()),
            "points is not valid on EVENT items"
        );

        let mut event_type_on_task = create_input(Some(WireItemType::Task));
        event_type_on_task.event_type = Some("rain".to_string());
        assert_eq!(
            invalid_message(try_into_new_item(event_type_on_task).unwrap_err()),
            "eventType is not valid on TASK items"
        );

        let mut due_date_on_simple = create_input(Some(WireItemType::Simple));
        due_date_on_simple.due_date = Some(SmithyDateTime::from_secs(1_000));
        assert_eq!(
            invalid_message(try_into_new_item(due_date_on_simple).unwrap_err()),
            "dueDate is not valid on SIMPLE items"
        );

        let mut parent_on_event = create_input(Some(WireItemType::Event));
        parent_on_event.parent_item_id = Some("p".to_string());
        assert_eq!(
            invalid_message(try_into_new_item(parent_on_event).unwrap_err()),
            "parentItemId is not valid on EVENT items"
        );

        let mut due_date_on_event = create_input(Some(WireItemType::Event));
        due_date_on_event.due_date = Some(SmithyDateTime::from_secs(1_000));
        assert_eq!(
            invalid_message(try_into_new_item(due_date_on_event).unwrap_err()),
            "dueDate is not valid on EVENT items"
        );

        let mut priority_on_template = create_input(Some(WireItemType::Template));
        priority_on_template.priority = Some(1);
        assert_eq!(
            invalid_message(try_into_new_item(priority_on_template).unwrap_err()),
            "priority is not valid on TEMPLATE items"
        );
    }

    /// `complete: true` on a non-Task is what `prl items done` sent for years while the
    /// server dropped it and the CLI printed success. It is now an error on both sides.
    #[test]
    fn completing_a_non_task_is_rejected() {
        let mut input = update_input(Some(WireItemType::Event));
        input.complete = true;
        assert_eq!(
            invalid_message(try_into_edit_item(input, ItemKind::Event).unwrap_err()),
            "complete is not valid on EVENT items"
        );
    }

    /// The deliberate exception, and the reason `reject_flag` exists separately from
    /// `reject_field`: `complete` is `@required` on `UpdateProjectItem` and on the MCP
    /// server's own `update_item` tool, so a caller renaming an Event has no way *not* to
    /// send it. `false` discards nothing, so it is accepted and ignored.
    #[test]
    fn a_false_completion_flag_still_lets_a_non_task_be_edited() {
        let mut input = update_input(Some(WireItemType::Event));
        input.complete = false;
        input.name = "renamed".to_string();
        let edit = try_into_edit_item(input, ItemKind::Event).expect("false discards nothing");
        assert_eq!(edit.name, "renamed");
        assert!(matches!(edit.kind, EditItemKind::Event(_)));
    }

    /// `itemType` is optional on update; the caller passes the resolved kind in. A request
    /// that omits it is checked against what the item already is, not against Task.
    #[test]
    fn an_omitted_item_type_is_checked_against_the_resolved_kind() {
        let mut input = update_input(None);
        input.points = Some(3);
        assert_eq!(
            invalid_message(try_into_edit_item(input, ItemKind::Event).unwrap_err()),
            "points is not valid on EVENT items"
        );

        let mut ok = update_input(None);
        ok.points = Some(3);
        assert!(try_into_edit_item(ok, ItemKind::Task).is_ok());
    }

    /// Two rejections relocated rather than introduced — both carry the wording they had when
    /// `Item::validate()` and `require_template_has_template_parent` raised them.
    #[test]
    fn relocated_rejections_keep_their_original_wording() {
        let mut both_anchors = create_input(Some(WireItemType::Task));
        both_anchors.parent_item_id = Some("parent".to_string());
        both_anchors.source_event_id = Some("event".to_string());
        assert_eq!(
            invalid_message(try_into_new_item(both_anchors).unwrap_err()),
            "an item cannot both have a parent and reference an event"
        );

        let root_template = create_input(Some(WireItemType::Template));
        assert_eq!(
            invalid_message(try_into_new_item(root_template).unwrap_err()),
            "item_type Template can only be set via the template creation flow"
        );
    }

    /// A well-formed request of each kind still converts, and an omitted `itemType` on create
    /// still means Task — `ItemKind`'s own `#[default]`.
    #[test]
    fn well_formed_requests_of_every_kind_still_convert() {
        assert!(matches!(
            try_into_new_item(create_input(None)).unwrap().kind,
            NewItemKind::Task(_)
        ));
        assert!(matches!(
            try_into_new_item(create_input(Some(WireItemType::Event)))
                .unwrap()
                .kind,
            NewItemKind::Event(_)
        ));
        assert!(matches!(
            try_into_new_item(create_input(Some(WireItemType::Simple)))
                .unwrap()
                .kind,
            NewItemKind::Simple(_)
        ));

        let mut template = create_input(Some(WireItemType::Template));
        template.parent_item_id = Some("root-template".to_string());
        assert!(matches!(
            try_into_new_item(template).unwrap().kind,
            NewItemKind::Template(_)
        ));
    }

    /// A Task request carrying every Task-only field survives intact — the conversion rejects
    /// what the kind cannot hold, and must not quietly narrow what it can.
    #[test]
    fn a_task_request_carries_every_task_field_through() {
        let mut input = create_input(Some(WireItemType::Task));
        input.parent_item_id = Some("parent".to_string());
        input.points = Some(5);
        input.priority = Some(2);
        input.assigned_to_user_id = Some("u1".to_string());
        input.complete = Some(true);
        input.due_offset_days = Some(-3);
        input.due_date = Some(SmithyDateTime::from_secs(1_000));
        input.has_due_time = Some(true);

        let NewItemKind::Task(task) = try_into_new_item(input).unwrap().kind else {
            panic!("expected a Task");
        };
        assert_eq!(task.anchor, TaskAnchor::Parent("parent".to_string()));
        assert_eq!(task.assignment.points, Some(5));
        assert_eq!(task.assignment.assigned_to_user_id.as_deref(), Some("u1"));
        assert_eq!(task.priority, Some(2));
        assert!(task.complete);
        assert_eq!(task.due_offset_days, Some(-3));
        assert!(task.schedule.has_due_time);
        assert!(task.schedule.due_date.is_some());
    }
}
