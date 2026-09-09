use crate::domain::item::{Item, ItemType, Recurrence, Schedule, TeamAssignment, TemplateItem};
use crate::service::items::{ItemError, copy_children_as_template};
use crate::service::projects::{
    require_project_admin, require_project_member, resolve_project_assignee,
};
use crate::storage::sqlite::{ItemRepo, ProjectRepo, TeamRepo};
use std::sync::Arc;

/// Resolves a root template's `TeamAssignment` (fixed) plus its rotation membership
/// from a create/update request — the template-level mirror of `item_series::
/// resolve_series_assignment`, sharing the identical mutual-exclusion, "explicitly
/// empty rotation is rejected", team-backed-project-only, and admin-gated-points rules
/// (root CLAUDE.md's Assignment rotation / Points sections). Unlike that function this
/// has no `item_type` axis to check — every caller here is already building a root
/// Template, and a non-root template child has no path that can reach this at all (see
/// `TemplateItem::team_assignment`'s doc comment).
async fn resolve_template_assignment_input(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    assigned_to_user_id: Option<String>,
    rotation_user_ids: Option<Vec<String>>,
    points: Option<i32>,
) -> Result<(Option<String>, Vec<String>, Option<i32>), ItemError> {
    if assigned_to_user_id.is_some() && rotation_user_ids.is_some() {
        return Err(ItemError::Invalid(
            "assignedToUserId and rotationUserIds are mutually exclusive".to_string(),
        ));
    }
    if let Some(ids) = &rotation_user_ids
        && ids.is_empty()
    {
        return Err(ItemError::Invalid(
            "rotationUserIds cannot be explicitly empty — omit it to clear the rotation"
                .to_string(),
        ));
    }
    if assigned_to_user_id.is_none() && rotation_user_ids.is_none() && points.is_none() {
        return Ok((None, Vec::new(), None));
    }
    let project = projects.get(project_id).await?;
    if project.team_id.is_none() {
        return Err(ItemError::Invalid(
            "assignedToUserId/rotationUserIds/points require a team-backed project".to_string(),
        ));
    }
    let resolved_assignee =
        resolve_project_assignee(projects, project_id, assigned_to_user_id).await?;
    let mut resolved_rotation = Vec::new();
    for user_id in rotation_user_ids.into_iter().flatten() {
        // `resolve_project_assignee` only returns `None` when given `None` — we always
        // pass `Some`, so the result is always `Some` too.
        if let Some(resolved) =
            resolve_project_assignee(projects, project_id, Some(user_id)).await?
        {
            resolved_rotation.push(resolved);
        }
    }
    let resolved_points = if points.is_some()
        && require_project_admin(projects, teams, project_id, requester_user_id)
            .await
            .is_ok()
    {
        points
    } else {
        None
    };
    Ok((resolved_assignee, resolved_rotation, resolved_points))
}

#[derive(Debug, Default)]
pub struct CreateTemplateParams {
    pub user_id: String,
    pub name: String,
    pub description: Option<String>,
    pub source_item_id: Option<String>,
    pub event_type: Option<String>,
    /// Set by `create_project_template`'s personal-project branch (Stage 5 of
    /// docs/team-id-removal-plan.md) so the new row is project-scoped from creation —
    /// left `None` by the legacy `json_api::templates::create_template` handler,
    /// matching that path's pre-Stage-5 behavior of never populating `project_id`.
    pub project_id: Option<String>,
    /// The root template's own offset, measured against a matching Event's anchor when the
    /// event-trigger fires (`copy_template_children_to_event`) — unlike every other offset in
    /// this codebase, this one may be positive ("N days after the event"), per
    /// `Item::validate()`'s scoped exception for a root Template. `None` if this template was
    /// never meant to auto-fire off an Event's date at all.
    pub due_offset_days: Option<i32>,
}

/// Moved from `json_api::templates::create_template`.
pub async fn create_template(
    repo: &Arc<dyn ItemRepo>,
    params: CreateTemplateParams,
) -> Result<String, ItemError> {
    let mut item = Item::new_user_item(&params.user_id, &params.name);
    item.project_id = params.project_id.clone();
    let mut schedule = Schedule::default();
    let mut recurrence = Recurrence::default();
    let mut event_type = None;
    let mut description = params.description.clone();

    let source_id = params.source_item_id;
    if let Some(source_id) = &source_id {
        let source = repo.get(&params.user_id, source_id).await?;
        if matches!(source.item_type, ItemType::Simple(_)) {
            return Err(ItemError::Invalid(
                "Simple list items cannot be saved as templates".to_string(),
            ));
        }
        recurrence = Recurrence {
            pattern: source.recurrence_pattern(),
            basis: source.recurrence_basis(),
            due_offset_days: source.due_offset_days(),
        };
        schedule.has_due_time = source.has_due_time();
        event_type = source.event_type();
        item.name = source.name;
        if description.is_none() {
            description = source.description.clone();
        }
        // deadline intentionally not copied — templates have no dates
    }
    if params.event_type.is_some() {
        event_type = params.event_type;
    }
    // Only overwrite a `source_item_id` copy's own offset when the caller actually supplied
    // one — mirrors `event_type`'s identical guard just above, for the identical reason: the
    // "save an item as a template" callers (`web_ui::project_tasks`/`project_events`) always
    // pass `None` here, and shouldn't silently clear whatever the source item's own offset was.
    if params.due_offset_days.is_some() {
        recurrence.due_offset_days = params.due_offset_days;
    }
    item.item_type = ItemType::Template(TemplateItem {
        parent_item_id: None,
        schedule,
        recurrence,
        event_type,
        // Assignment/points are a team-backed-project-only concept (root CLAUDE.md's
        // Points section) — a personal template has no field to accept one in the
        // first place, matching `CreateTemplateParams` here carrying no such field.
        team_assignment: None,
    });
    item.description = description;

    let template_id = repo.create(&item).await?;

    if let Some(source_id) = &source_id {
        copy_children_as_template(repo, source_id, &template_id).await?;
    }

    Ok(template_id)
}

#[derive(Debug, Default)]
pub struct UpdateTemplateParams {
    pub user_id: String,
    pub template_id: String,
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    /// See `CreateTemplateParams::due_offset_days`. Follows `event_type`'s direct-overwrite,
    /// no-service-layer-merge convention — every update construction site must round-trip the
    /// current value explicitly to preserve it.
    pub due_offset_days: Option<i32>,
}

/// Edits a template's own fields — `name`, `description`, `event_type`, and now
/// `due_offset_days`, the only things the create form (`create_template` above) lets a caller
/// set directly. `schedule` (only ever populated by copying a source item at creation time)
/// rides along unchanged.
pub async fn update_template(
    repo: &Arc<dyn ItemRepo>,
    params: UpdateTemplateParams,
) -> Result<(), ItemError> {
    let current = repo.get(&params.user_id, &params.template_id).await?;
    if !matches!(current.item_type, ItemType::Template(_)) {
        return Err(ItemError::Invalid("item is not a template".to_string()));
    }

    let mut item = current;
    item.name = params.name;
    item.description = params.description;
    if let ItemType::Template(t) = &mut item.item_type {
        t.event_type = params.event_type;
        t.recurrence.due_offset_days = params.due_offset_days;
    }

    repo.update(&item).await?;
    Ok(())
}

#[derive(Debug, Default)]
pub struct CreateTeamTemplateParams {
    pub project_id: String,
    pub requester_user_id: String,
    pub name: String,
    pub description: Option<String>,
    pub source_item_id: Option<String>,
    pub event_type: Option<String>,
    /// See `CreateTemplateParams::due_offset_days`.
    pub due_offset_days: Option<i32>,
    /// The fixed assignee a root template's instantiation stamps onto the new
    /// top-level task it creates (`service::items::resolve_template_assignment`).
    /// Mutually exclusive with `rotation_user_ids` — see
    /// `resolve_template_assignment_input`. `None` here means "no fixed assignee",
    /// not necessarily "no assignment at all" (a rotation might still apply).
    pub assigned_to_user_id: Option<String>,
    /// The rotating alternative to `assigned_to_user_id` — see
    /// `resolve_template_assignment_input`'s doc comment for the full mutual-exclusion/
    /// empty-rejection rules, which mirror `item_series`'s identical field exactly.
    pub rotation_user_ids: Option<Vec<String>>,
    /// Team-backed-project-only, admin-gated exactly like `TeamAssignment::points`
    /// elsewhere — see `resolve_template_assignment_input`.
    pub points: Option<i32>,
}

/// Team-scoped twin of `create_template` above. Reuses `copy_children_as_template`
/// unchanged — it already just `child.clone()`s before overwriting template-specific
/// fields, so it carries over whichever of `user_id`/`project_id` the source subtree
/// had.
///
/// Rewritten in Stage 5 of docs/team-id-removal-plan.md to be `project_id`-primary,
/// mirroring `team_items::create_team_item`'s own Stage 4 rewrite. Stage 6 dropped the
/// `items.team_id` dual-write this function used to need (`ItemRepo::list_team_templates`,
/// the only reader of that column, was removed once `json_api::team_templates::
/// list_team_templates` was repointed at `list_templates_by_project`).
pub async fn create_team_template(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    projects: &Arc<dyn ProjectRepo>,
    params: CreateTeamTemplateParams,
) -> Result<String, ItemError> {
    require_project_member(
        projects,
        teams,
        &params.project_id,
        &params.requester_user_id,
    )
    .await?;

    let mut item = Item::new_project_item(&params.project_id, &params.name);
    let mut schedule = Schedule::default();
    let mut recurrence = Recurrence::default();
    let mut event_type = None;
    let mut description = params.description.clone();

    let source_id = params.source_item_id;
    if let Some(source_id) = &source_id {
        // get_by_project (not get) confirms the source item actually belongs to this project.
        let source = repo.get_by_project(&params.project_id, source_id).await?;
        if matches!(source.item_type, ItemType::Simple(_)) {
            return Err(ItemError::Invalid(
                "Simple list items cannot be saved as templates".to_string(),
            ));
        }
        recurrence = Recurrence {
            pattern: source.recurrence_pattern(),
            basis: source.recurrence_basis(),
            due_offset_days: source.due_offset_days(),
        };
        schedule.has_due_time = source.has_due_time();
        event_type = source.event_type();
        item.name = source.name;
        if description.is_none() {
            description = source.description.clone();
        }
        // deadline intentionally not copied — templates have no dates
    }
    if params.event_type.is_some() {
        event_type = params.event_type;
    }
    // Only overwrite a `source_item_id` copy's own offset when the caller actually supplied
    // one — mirrors `event_type`'s identical guard just above, for the identical reason: the
    // "save an item as a template" callers (`web_ui::project_tasks`/`project_events`) always
    // pass `None` here, and shouldn't silently clear whatever the source item's own offset was.
    if params.due_offset_days.is_some() {
        recurrence.due_offset_days = params.due_offset_days;
    }
    let (assigned_to_user_id, rotation_user_ids, points) = resolve_template_assignment_input(
        projects,
        teams,
        &params.project_id,
        &params.requester_user_id,
        params.assigned_to_user_id,
        params.rotation_user_ids,
        params.points,
    )
    .await?;
    item.item_type = ItemType::Template(TemplateItem {
        parent_item_id: None,
        schedule,
        recurrence,
        event_type,
        team_assignment: if assigned_to_user_id.is_some() || points.is_some() {
            Some(TeamAssignment {
                assigned_to_user_id,
                points,
            })
        } else {
            None
        },
    });
    item.description = description;

    let template_id = repo.create(&item).await?;

    if !rotation_user_ids.is_empty() {
        repo.set_template_rotation_members(&template_id, &rotation_user_ids)
            .await?;
    }

    if let Some(source_id) = &source_id {
        copy_children_as_template(repo, source_id, &template_id).await?;
    }

    Ok(template_id)
}

#[derive(Debug, Default)]
pub struct UpdateTeamTemplateParams {
    pub project_id: String,
    pub requester_user_id: String,
    pub template_id: String,
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    /// See `CreateTemplateParams::due_offset_days`.
    pub due_offset_days: Option<i32>,
    /// See `CreateTeamTemplateParams::assigned_to_user_id`. Follows the same
    /// direct-overwrite, no-service-layer-merge convention as `event_type`/
    /// `due_offset_days` above — every update call site must round-trip the current
    /// value explicitly to preserve it.
    pub assigned_to_user_id: Option<String>,
    /// See `CreateTeamTemplateParams::rotation_user_ids`.
    pub rotation_user_ids: Option<Vec<String>>,
    /// See `CreateTeamTemplateParams::points`.
    pub points: Option<i32>,
}

/// Team-scoped twin of `update_template` above. Rewritten in Stage 5 of
/// docs/team-id-removal-plan.md to be `project_id`-primary — unlike
/// `create_team_template` above, no `team_id` dual-write is needed here:
/// `update_by_project`'s `UPDATE` statement never touches the `team_id` column at all,
/// the same finding `team_items::update_team_item`'s own Stage 4 rewrite made.
pub async fn update_team_template(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    projects: &Arc<dyn ProjectRepo>,
    params: UpdateTeamTemplateParams,
) -> Result<(), ItemError> {
    require_project_member(
        projects,
        teams,
        &params.project_id,
        &params.requester_user_id,
    )
    .await?;

    let current = repo
        .get_by_project(&params.project_id, &params.template_id)
        .await?;
    if !matches!(current.item_type, ItemType::Template(_)) {
        return Err(ItemError::Invalid("item is not a template".to_string()));
    }

    let (assigned_to_user_id, rotation_user_ids, points) = resolve_template_assignment_input(
        projects,
        teams,
        &params.project_id,
        &params.requester_user_id,
        params.assigned_to_user_id,
        params.rotation_user_ids,
        params.points,
    )
    .await?;

    let mut item = current;
    item.name = params.name;
    item.description = params.description;
    if let ItemType::Template(t) = &mut item.item_type {
        t.event_type = params.event_type;
        t.recurrence.due_offset_days = params.due_offset_days;
        // Direct-overwrite, same convention as `event_type`/`due_offset_days` above —
        // omitting either field on an update clears it rather than preserving the
        // current value.
        t.team_assignment = if assigned_to_user_id.is_some() || points.is_some() {
            Some(TeamAssignment {
                assigned_to_user_id,
                points,
            })
        } else {
            None
        };
    }

    repo.update_by_project(&item).await?;
    // Full-replace regardless of whether rotation_user_ids is empty, mirroring
    // `item_series::update_series` — so switching a template from rotating back to
    // fixed (or to neither) actually clears its prior members.
    repo.set_template_rotation_members(&params.template_id, &rotation_user_ids)
        .await?;
    Ok(())
}

/// Stage B5d's project-scoped read path. Rewritten in Stage 5 of
/// docs/team-id-removal-plan.md to call the `project_id`-scoped, `TEMPLATE`-filtered
/// `list_templates_by_project` (added in Stage 1) directly. Before this stage,
/// `create_template`/`create_team_template` never populated `Item::project_id` at all
/// (neither routed through `items::create_item`/`team_items::create_team_item` — see
/// docs/project-abstraction-plan.md's stage B5d notes), so this function had to resolve
/// `project_id` down to the owning `user_id`/`team_id` and delegate to the legacy
/// user_id/team_id-keyed list methods instead. As of this stage, both creation paths
/// set `project_id` directly on the row at creation time (see `create_template`'s own
/// optional `project_id` param and `create_team_template`'s `Item::new_project_item`),
/// so a direct project-scoped list is always correct.
pub async fn list_project_templates(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
) -> Result<Vec<Item>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;
    Ok(repo.list_templates_by_project(project_id).await?)
}

#[derive(Debug, Default)]
pub struct CreateProjectTemplateParams {
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub source_item_id: Option<String>,
    pub event_type: Option<String>,
    /// See `CreateTemplateParams::due_offset_days`.
    pub due_offset_days: Option<i32>,
    /// See `CreateTeamTemplateParams::assigned_to_user_id`. Dropped on the personal
    /// (team-less) branch below — a personal template has no field to accept one.
    pub assigned_to_user_id: Option<String>,
    /// See `CreateTeamTemplateParams::rotation_user_ids`.
    pub rotation_user_ids: Option<Vec<String>>,
    /// See `CreateTeamTemplateParams::points`.
    pub points: Option<i32>,
}

/// Stage B5d's project-scoped create path — same "resolve project_id down to user_id/
/// team_id, delegate to the existing function" shape `service::project_items::
/// create_project_item` already established for real items. As of Stage 5 of
/// docs/team-id-removal-plan.md, both branches set `project_id` on the new row as part
/// of the single `repo.create` call inside `create_template`/`create_team_template` —
/// the second write this function used to need, purely to backfill `project_id` onto a
/// row that had none, is gone.
pub async fn create_project_template(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    params: CreateProjectTemplateParams,
) -> Result<String, ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    let project = projects.get(&params.project_id).await?;
    let template_id = match &project.team_id {
        Some(_) => {
            create_team_template(
                repo,
                teams,
                projects,
                CreateTeamTemplateParams {
                    project_id: params.project_id.clone(),
                    requester_user_id: requester_user_id.to_string(),
                    name: params.name,
                    description: params.description,
                    source_item_id: params.source_item_id,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    assigned_to_user_id: params.assigned_to_user_id,
                    rotation_user_ids: params.rotation_user_ids,
                    points: params.points,
                },
            )
            .await?
        }
        None => {
            create_template(
                repo,
                CreateTemplateParams {
                    user_id: project.owner_user_id.clone(),
                    name: params.name,
                    description: params.description,
                    source_item_id: params.source_item_id,
                    event_type: params.event_type,
                    project_id: Some(params.project_id.clone()),
                    due_offset_days: params.due_offset_days,
                },
            )
            .await?
        }
    };
    Ok(template_id)
}

#[derive(Debug, Default)]
pub struct UpdateProjectTemplateParams {
    pub project_id: String,
    pub template_id: String,
    pub name: String,
    pub description: Option<String>,
    pub event_type: Option<String>,
    /// See `CreateTemplateParams::due_offset_days`.
    pub due_offset_days: Option<i32>,
    /// See `CreateProjectTemplateParams::assigned_to_user_id`.
    pub assigned_to_user_id: Option<String>,
    /// See `CreateProjectTemplateParams::rotation_user_ids`.
    pub rotation_user_ids: Option<Vec<String>>,
    /// See `CreateProjectTemplateParams::points`.
    pub points: Option<i32>,
}

/// Stage B5d's project-scoped update path — same delegation shape as
/// `create_project_template`.
pub async fn update_project_template(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    params: UpdateProjectTemplateParams,
) -> Result<(), ItemError> {
    require_project_member(projects, teams, &params.project_id, requester_user_id).await?;
    let project = projects.get(&params.project_id).await?;
    match project.team_id {
        Some(_) => {
            update_team_template(
                repo,
                teams,
                projects,
                UpdateTeamTemplateParams {
                    project_id: params.project_id,
                    requester_user_id: requester_user_id.to_string(),
                    template_id: params.template_id,
                    name: params.name,
                    description: params.description,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                    assigned_to_user_id: params.assigned_to_user_id,
                    rotation_user_ids: params.rotation_user_ids,
                    points: params.points,
                },
            )
            .await
        }
        None => {
            update_template(
                repo,
                UpdateTemplateParams {
                    user_id: project.owner_user_id,
                    template_id: params.template_id,
                    name: params.name,
                    description: params.description,
                    event_type: params.event_type,
                    due_offset_days: params.due_offset_days,
                },
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{SimpleItem, TaskItem};
    use crate::domain::team::TeamRole;
    use crate::storage::sqlite::MockItemRepo;

    #[tokio::test]
    async fn create_template_from_source_copies_its_children() {
        let mut mock = MockItemRepo::new();

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "src")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "src".to_string(),
                    user_id: Some("u1".to_string()),
                    name: "Move house".to_string(),
                    ..Item::default()
                })
            });

        mock.expect_create()
            .withf(|item: &Item| {
                item.parent_item_id().is_none() && matches!(item.item_type, ItemType::Template(_))
            })
            .times(1)
            .returning(|_| Ok("tpl1".to_string()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "src")
            .times(1)
            .returning(|_| {
                Ok(vec![Item {
                    id: "child1".to_string(),
                    name: "Pack boxes".to_string(),
                    item_type: ItemType::Task(TaskItem {
                        parent_item_id: Some("src".to_string()),
                        schedule: Schedule::default(),
                        recurrence: Recurrence {
                            due_offset_days: Some(-3),
                            ..Recurrence::default()
                        },
                        team_assignment: None,
                        source_event_id: None,
                        source_template_id: None,
                        priority: None,
                        complete: false,
                        series_id: None,
                    }),
                    ..Item::default()
                }])
            });

        mock.expect_create()
            .withf(|item: &Item| {
                item.parent_item_id().as_deref() == Some("tpl1")
                    && matches!(item.item_type, ItemType::Template(_))
                    && item.due_offset_days() == Some(-3)
            })
            .times(1)
            .returning(|_| Ok("child-tpl1".to_string()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "child1")
            .times(1)
            .returning(|_| Ok(vec![]));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let template_id = create_template(
            &repo,
            CreateTemplateParams {
                user_id: "u1".to_string(),
                name: "Move house".to_string(),
                description: None,
                source_item_id: Some("src".to_string()),
                event_type: None,
                project_id: None,
                due_offset_days: None,
            },
        )
        .await
        .expect("should create template with copied children");

        assert_eq!(template_id, "tpl1");
    }

    #[tokio::test]
    async fn create_template_rejects_simple_source() {
        let mut mock = MockItemRepo::new();

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "src")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "src".to_string(),
                    user_id: Some("u1".to_string()),
                    name: "Groceries".to_string(),
                    item_type: ItemType::Simple(SimpleItem::default()),
                    ..Item::default()
                })
            });

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let result = create_template(
            &repo,
            CreateTemplateParams {
                user_id: "u1".to_string(),
                name: "Groceries".to_string(),
                description: None,
                source_item_id: Some("src".to_string()),
                event_type: None,
                project_id: None,
                due_offset_days: None,
            },
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_team_template_with_a_fixed_assignee_sets_it_on_the_template() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.assigned_to_user_id().as_deref() == Some("alice"))
            .times(1)
            .returning(|_| Ok("tpl1".to_string()));

        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(shared_project()));
        projects
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);

        create_team_template(
            &repo,
            &teams,
            &projects,
            CreateTeamTemplateParams {
                project_id: "p1".to_string(),
                requester_user_id: "member1".to_string(),
                name: "Chore".to_string(),
                description: None,
                source_item_id: None,
                event_type: None,
                due_offset_days: None,
                assigned_to_user_id: Some("alice".to_string()),
                rotation_user_ids: None,
                points: None,
            },
        )
        .await
        .expect("should create team template with a fixed assignee");
    }

    #[tokio::test]
    async fn create_team_template_with_a_rotation_persists_the_membership() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| item.assigned_to_user_id().is_none())
            .times(1)
            .returning(|_| Ok("tpl1".to_string()));
        items_mock
            .expect_set_template_rotation_members()
            .withf(|template_id: &str, ids: &[String]| {
                template_id == "tpl1" && ids == ["alice".to_string(), "bob".to_string()]
            })
            .times(1)
            .returning(|_, _| Ok(()));

        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(shared_project()));
        projects
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);

        create_team_template(
            &repo,
            &teams,
            &projects,
            CreateTeamTemplateParams {
                project_id: "p1".to_string(),
                requester_user_id: "member1".to_string(),
                name: "Chore".to_string(),
                description: None,
                source_item_id: None,
                event_type: None,
                due_offset_days: None,
                assigned_to_user_id: None,
                rotation_user_ids: Some(vec!["alice".to_string(), "bob".to_string()]),
                points: None,
            },
        )
        .await
        .expect("should create team template with a rotation");
    }

    #[tokio::test]
    async fn create_team_template_rejects_simple_source() {
        let mut mock = MockItemRepo::new();
        mock.expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "src")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "src".to_string(),
                    project_id: Some("p1".to_string()),
                    name: "Groceries".to_string(),
                    item_type: ItemType::Simple(SimpleItem::default()),
                    ..Item::default()
                })
            });

        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(shared_project()));
        projects
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);

        let result = create_team_template(
            &repo,
            &teams,
            &projects,
            CreateTeamTemplateParams {
                project_id: "p1".to_string(),
                requester_user_id: "u1".to_string(),
                name: "Groceries".to_string(),
                description: None,
                source_item_id: Some("src".to_string()),
                event_type: None,
                due_offset_days: None,
                assigned_to_user_id: None,
                rotation_user_ids: None,
                points: None,
            },
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_template_changes_name_and_event_type() {
        let mut mock = MockItemRepo::new();

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "tpl1")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "tpl1".to_string(),
                    user_id: Some("u1".to_string()),
                    name: "Old name".to_string(),
                    item_type: ItemType::Template(TemplateItem {
                        parent_item_id: None,
                        schedule: Schedule::default(),
                        recurrence: Recurrence::default(),
                        event_type: None,
                        team_assignment: None,
                    }),
                    ..Item::default()
                })
            });

        mock.expect_update()
            .withf(|item: &Item| {
                item.id == "tpl1"
                    && item.name == "New name"
                    && matches!(&item.item_type, ItemType::Template(t) if t.event_type.as_deref() == Some("rain"))
            })
            .times(1)
            .returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        update_template(
            &repo,
            UpdateTemplateParams {
                user_id: "u1".to_string(),
                template_id: "tpl1".to_string(),
                name: "New name".to_string(),
                description: None,
                event_type: Some("rain".to_string()),
                due_offset_days: None,
            },
        )
        .await
        .expect("should update template");
    }

    #[tokio::test]
    async fn update_template_rejects_non_template_item() {
        let mut mock = MockItemRepo::new();

        mock.expect_get()
            .withf(|user_id: &str, item_id: &str| user_id == "u1" && item_id == "item1")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "item1".to_string(),
                    user_id: Some("u1".to_string()),
                    name: "A task".to_string(),
                    item_type: ItemType::Task(TaskItem {
                        parent_item_id: None,
                        schedule: Schedule::default(),
                        recurrence: Recurrence::default(),
                        team_assignment: None,
                        source_event_id: None,
                        source_template_id: None,
                        priority: None,
                        complete: false,
                        series_id: None,
                    }),
                    ..Item::default()
                })
            });

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let result = update_template(
            &repo,
            UpdateTemplateParams {
                user_id: "u1".to_string(),
                template_id: "item1".to_string(),
                name: "New name".to_string(),
                description: None,
                event_type: None,
                due_offset_days: None,
            },
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_team_template_changes_name_and_event_type() {
        let mut mock = MockItemRepo::new();

        mock.expect_get_by_project()
            .withf(|project_id: &str, item_id: &str| project_id == "p1" && item_id == "tpl1")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "tpl1".to_string(),
                    project_id: Some("p1".to_string()),
                    name: "Old name".to_string(),
                    item_type: ItemType::Template(TemplateItem {
                        parent_item_id: None,
                        schedule: Schedule::default(),
                        recurrence: Recurrence::default(),
                        event_type: None,
                        team_assignment: None,
                    }),
                    ..Item::default()
                })
            });

        mock.expect_update_by_project()
            .withf(|item: &Item| {
                item.id == "tpl1"
                    && item.name == "New name"
                    && matches!(&item.item_type, ItemType::Template(t) if t.event_type.as_deref() == Some("rain"))
            })
            .times(1)
            .returning(|_| Ok(()));
        mock.expect_set_template_rotation_members()
            .withf(|template_id: &str, ids: &[String]| template_id == "tpl1" && ids.is_empty())
            .times(1)
            .returning(|_, _| Ok(()));

        let mut projects = MockProjectRepo::new();
        projects.expect_get().returning(|_| Ok(shared_project()));
        projects
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects);

        update_team_template(
            &repo,
            &teams,
            &projects,
            UpdateTeamTemplateParams {
                project_id: "p1".to_string(),
                requester_user_id: "u1".to_string(),
                template_id: "tpl1".to_string(),
                name: "New name".to_string(),
                description: None,
                event_type: Some("rain".to_string()),
                due_offset_days: None,
                assigned_to_user_id: None,
                rotation_user_ids: None,
                points: None,
            },
        )
        .await
        .expect("should update team template");
    }

    use crate::domain::project::Project;
    use crate::storage::sqlite::{MockProjectRepo, MockTeamRepo};

    fn personal_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Personal".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: None,
        }
    }

    fn shared_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Shared".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: Some("team1".to_string()),
        }
    }

    #[tokio::test]
    async fn resolve_template_assignment_input_rejects_mutually_exclusive_fields() {
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = resolve_template_assignment_input(
            &projects,
            &teams,
            "p1",
            "owner1",
            Some("alice".to_string()),
            Some(vec!["bob".to_string()]),
            None,
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn resolve_template_assignment_input_rejects_explicitly_empty_rotation() {
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = resolve_template_assignment_input(
            &projects,
            &teams,
            "p1",
            "owner1",
            None,
            Some(vec![]),
            None,
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn resolve_template_assignment_input_short_circuits_when_nothing_requested() {
        // No mock expectations at all — proves this never even fetches the project when
        // every field is omitted, same as `resolve_series_assignment`'s identical guard.
        let projects: Arc<dyn ProjectRepo> = Arc::new(MockProjectRepo::new());
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result =
            resolve_template_assignment_input(&projects, &teams, "p1", "owner1", None, None, None)
                .await
                .unwrap();

        assert_eq!(result, (None, Vec::new(), None));
    }

    #[tokio::test]
    async fn resolve_template_assignment_input_rejects_a_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = resolve_template_assignment_input(
            &projects,
            &teams,
            "p1",
            "owner1",
            Some("alice".to_string()),
            None,
            None,
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn resolve_template_assignment_input_resolves_a_fixed_assignee_on_a_team_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let (assigned_to_user_id, rotation, points) = resolve_template_assignment_input(
            &projects,
            &teams,
            "p1",
            "owner1",
            Some("alice".to_string()),
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(assigned_to_user_id.as_deref(), Some("alice"));
        assert!(rotation.is_empty());
        assert_eq!(points, None);
    }

    #[tokio::test]
    async fn resolve_template_assignment_input_drops_points_for_a_non_admin() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let (_, _, points) = resolve_template_assignment_input(
            &projects,
            &teams,
            "p1",
            "requester1",
            None,
            None,
            Some(10),
        )
        .await
        .unwrap();

        assert_eq!(points, None);
    }

    #[tokio::test]
    async fn resolve_template_assignment_input_keeps_points_for_an_admin() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Admin)));
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let (_, _, points) = resolve_template_assignment_input(
            &projects,
            &teams,
            "p1",
            "admin1",
            None,
            None,
            Some(10),
        )
        .await
        .unwrap();

        assert_eq!(points, Some(10));
    }

    #[tokio::test]
    async fn list_project_templates_calls_list_templates_by_project_on_personal_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_list_templates_by_project()
            .withf(|project_id: &str| project_id == "p1")
            .times(1)
            .returning(|_| Ok(vec![]));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        list_project_templates(&repo, &projects, &teams, "p1", "owner1")
            .await
            .expect("should list personal templates");
    }

    #[tokio::test]
    async fn list_project_templates_calls_list_templates_by_project_on_shared_project() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_list_templates_by_project()
            .withf(|project_id: &str| project_id == "p1")
            .times(1)
            .returning(|_| Ok(vec![]));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        list_project_templates(&repo, &projects, &teams, "p1", "member1")
            .await
            .expect("should list team templates");
    }

    #[tokio::test]
    async fn list_project_templates_rejects_non_member() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let repo: Arc<dyn ItemRepo> = Arc::new(MockItemRepo::new());
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let result = list_project_templates(&repo, &projects, &teams, "p1", "not-owner").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_project_template_delegates_to_personal_creation() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| {
                item.user_id.as_deref() == Some("owner1")
                    && item.project_id.as_deref() == Some("p1")
                    && matches!(item.item_type, ItemType::Template(_))
            })
            .times(1)
            .returning(|_| Ok("tpl1".to_string()));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let template_id = create_project_template(
            &repo,
            &projects,
            &teams,
            "owner1",
            CreateProjectTemplateParams {
                project_id: "p1".to_string(),
                name: "Move house".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create personal project template");

        assert_eq!(template_id, "tpl1");
    }

    #[tokio::test]
    async fn create_project_template_delegates_to_team_creation() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_create()
            .withf(|item: &Item| {
                item.project_id.as_deref() == Some("p1")
                    && matches!(item.item_type, ItemType::Template(_))
            })
            .times(1)
            .returning(|_| Ok("tpl1".to_string()));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        let template_id = create_project_template(
            &repo,
            &projects,
            &teams,
            "member1",
            CreateProjectTemplateParams {
                project_id: "p1".to_string(),
                name: "Onboard hire".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should create team project template");

        assert_eq!(template_id, "tpl1");
    }

    #[tokio::test]
    async fn update_project_template_delegates_to_personal_update() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(personal_project()));
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get().returning(|_, _| {
            Ok(Item {
                id: "tpl1".to_string(),
                user_id: Some("owner1".to_string()),
                name: "Old name".to_string(),
                item_type: ItemType::Template(TemplateItem {
                    parent_item_id: None,
                    schedule: Schedule::default(),
                    recurrence: Recurrence::default(),
                    event_type: None,
                    team_assignment: None,
                }),
                ..Item::default()
            })
        });
        items_mock.expect_update().times(1).returning(|_| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        update_project_template(
            &repo,
            &projects,
            &teams,
            "owner1",
            UpdateProjectTemplateParams {
                project_id: "p1".to_string(),
                template_id: "tpl1".to_string(),
                name: "New name".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should update personal project template");
    }

    #[tokio::test]
    async fn update_project_template_delegates_to_team_update() {
        let mut projects_mock = MockProjectRepo::new();
        projects_mock
            .expect_get()
            .returning(|_| Ok(shared_project()));
        projects_mock
            .expect_member_role()
            .returning(|_, _| Ok(Some(TeamRole::Member)));
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get_by_project().returning(|_, _| {
            Ok(Item {
                id: "tpl1".to_string(),
                project_id: Some("p1".to_string()),
                name: "Old name".to_string(),
                item_type: ItemType::Template(TemplateItem {
                    parent_item_id: None,
                    schedule: Schedule::default(),
                    recurrence: Recurrence::default(),
                    event_type: None,
                    team_assignment: None,
                }),
                ..Item::default()
            })
        });
        items_mock
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));
        items_mock
            .expect_set_template_rotation_members()
            .returning(|_, _| Ok(()));

        let repo: Arc<dyn ItemRepo> = Arc::new(items_mock);
        let projects: Arc<dyn ProjectRepo> = Arc::new(projects_mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(MockTeamRepo::new());

        update_project_template(
            &repo,
            &projects,
            &teams,
            "member1",
            UpdateProjectTemplateParams {
                project_id: "p1".to_string(),
                template_id: "tpl1".to_string(),
                name: "New name".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("should update team project template");
    }
}
