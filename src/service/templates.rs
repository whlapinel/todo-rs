use crate::domain::item::{Item, ItemType, Recurrence, Schedule, TemplateItem};
use crate::service::items::{ItemError, copy_children_as_template};
use crate::service::projects::require_project_member;
use crate::storage::sqlite::{ItemRepo, ProjectRepo, TeamRepo};
use std::sync::Arc;

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
    item.item_type = ItemType::Template(TemplateItem {
        parent_item_id: None,
        schedule,
        recurrence,
        event_type,
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
}

/// Edits a template's own fields — `name`, `description`, and `event_type`, the only
/// things the create form (`create_template` above) lets a caller set directly.
/// `schedule`/`recurrence` (only ever populated by copying a source item at creation
/// time) ride along unchanged.
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
    item.item_type = ItemType::Template(TemplateItem {
        parent_item_id: None,
        schedule,
        recurrence,
        event_type,
    });
    item.description = description;

    let template_id = repo.create(&item).await?;

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

    let mut item = current;
    item.name = params.name;
    item.description = params.description;
    if let ItemType::Template(t) = &mut item.item_type {
        t.event_type = params.event_type;
    }

    repo.update_by_project(&item).await?;
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
            },
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
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
                }),
                ..Item::default()
            })
        });
        items_mock
            .expect_update_by_project()
            .times(1)
            .returning(|_| Ok(()));

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
