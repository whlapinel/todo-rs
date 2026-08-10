use crate::domain::item::{Item, ItemType, Recurrence, Schedule};
use crate::service::items::{copy_children_as_template, ItemError};
use crate::service::team_items::require_active_member;
use crate::storage::sqlite::{ItemRepo, TeamRepo};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct CreateTemplateParams {
    pub user_id: String,
    pub name: String,
    pub source_item_id: Option<String>,
    pub event_type: Option<String>,
}

/// Moved from `json_api::templates::create_template`.
pub async fn create_template(
    repo: &Arc<dyn ItemRepo>,
    params: CreateTemplateParams,
) -> Result<String, ItemError> {
    let mut item = Item::new_user_item(&params.user_id, &params.name);
    let mut schedule = Schedule::default();
    let mut recurrence = Recurrence::default();
    let mut event_type = None;

    let source_id = params.source_item_id;
    if let Some(source_id) = &source_id {
        let source = repo.get(&params.user_id, source_id).await?;
        if matches!(source.item_type, ItemType::Simple) {
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
        // deadline intentionally not copied — templates have no dates
    }
    if params.event_type.is_some() {
        event_type = params.event_type;
    }
    item.item_type = ItemType::Template {
        schedule,
        recurrence,
        event_type,
    };

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
    pub event_type: Option<String>,
}

/// Edits a template's own fields — `name` and `event_type`, the only two things the
/// create form (`create_template` above) lets a caller set directly. `schedule`/`recurrence`
/// (only ever populated by copying a source item at creation time) ride along unchanged.
pub async fn update_template(
    repo: &Arc<dyn ItemRepo>,
    params: UpdateTemplateParams,
) -> Result<(), ItemError> {
    let current = repo.get(&params.user_id, &params.template_id).await?;
    if !matches!(current.item_type, ItemType::Template { .. }) {
        return Err(ItemError::Invalid("item is not a template".to_string()));
    }

    let mut item = current;
    item.name = params.name;
    if let ItemType::Template { event_type, .. } = &mut item.item_type {
        *event_type = params.event_type;
    }

    repo.update(&item).await?;
    Ok(())
}

#[derive(Debug, Default)]
pub struct CreateTeamTemplateParams {
    pub team_id: String,
    pub requester_user_id: String,
    pub name: String,
    pub source_item_id: Option<String>,
    pub event_type: Option<String>,
}

/// Team-scoped twin of `create_template` above. Reuses `copy_children_as_template`
/// unchanged — it already just `child.clone()`s before overwriting template-specific
/// fields, so it carries over whichever of `user_id`/`team_id` the source subtree had.
pub async fn create_team_template(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    params: CreateTeamTemplateParams,
) -> Result<String, ItemError> {
    require_active_member(teams, &params.team_id, &params.requester_user_id).await?;

    let mut item = Item::new_team_item(&params.team_id, &params.name);
    let mut schedule = Schedule::default();
    let mut recurrence = Recurrence::default();
    let mut event_type = None;

    let source_id = params.source_item_id;
    if let Some(source_id) = &source_id {
        // get_team_item (not get) confirms the source item actually belongs to this team.
        let source = repo.get_team_item(&params.team_id, source_id).await?;
        if matches!(source.item_type, ItemType::Simple) {
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
        // deadline intentionally not copied — templates have no dates
    }
    if params.event_type.is_some() {
        event_type = params.event_type;
    }
    item.item_type = ItemType::Template {
        schedule,
        recurrence,
        event_type,
    };

    let template_id = repo.create(&item).await?;

    if let Some(source_id) = &source_id {
        copy_children_as_template(repo, source_id, &template_id).await?;
    }

    Ok(template_id)
}

#[derive(Debug, Default)]
pub struct UpdateTeamTemplateParams {
    pub team_id: String,
    pub requester_user_id: String,
    pub template_id: String,
    pub name: String,
    pub event_type: Option<String>,
}

/// Team-scoped twin of `update_template` above.
pub async fn update_team_template(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    params: UpdateTeamTemplateParams,
) -> Result<(), ItemError> {
    require_active_member(teams, &params.team_id, &params.requester_user_id).await?;

    let current = repo.get_team_item(&params.team_id, &params.template_id).await?;
    if !matches!(current.item_type, ItemType::Template { .. }) {
        return Err(ItemError::Invalid("item is not a template".to_string()));
    }

    let mut item = current;
    item.name = params.name;
    if let ItemType::Template { event_type, .. } = &mut item.item_type {
        *event_type = params.event_type;
    }

    repo.update_team_item(&item).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
                item.parent_item_id.is_none() && matches!(item.item_type, ItemType::Template { .. })
            })
            .times(1)
            .returning(|_| Ok("tpl1".to_string()));

        mock.expect_list_children()
            .withf(|parent_id: &str| parent_id == "src")
            .times(1)
            .returning(|_| {
                Ok(vec![Item {
                    id: "child1".to_string(),
                    parent_item_id: Some("src".to_string()),
                    name: "Pack boxes".to_string(),
                    item_type: ItemType::Task {
                        schedule: Schedule::default(),
                        recurrence: Recurrence {
                            due_offset_days: Some(-3),
                            ..Recurrence::default()
                        },
                        team_assignment: None,
                    },
                    ..Item::default()
                }])
            });

        mock.expect_create()
            .withf(|item: &Item| {
                item.parent_item_id.as_deref() == Some("tpl1")
                    && matches!(item.item_type, ItemType::Template { .. })
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
                source_item_id: Some("src".to_string()),
                event_type: None,
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
                    item_type: ItemType::Simple,
                    ..Item::default()
                })
            });

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);

        let result = create_template(
            &repo,
            CreateTemplateParams {
                user_id: "u1".to_string(),
                name: "Groceries".to_string(),
                source_item_id: Some("src".to_string()),
                event_type: None,
            },
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn create_team_template_rejects_simple_source() {
        use crate::storage::sqlite::MockTeamRepo;

        let mut mock = MockItemRepo::new();
        mock.expect_get_team_item()
            .withf(|team_id: &str, item_id: &str| team_id == "team1" && item_id == "src")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "src".to_string(),
                    team_id: Some("team1".to_string()),
                    name: "Groceries".to_string(),
                    item_type: ItemType::Simple,
                    ..Item::default()
                })
            });

        let mut teams = MockTeamRepo::new();
        teams
            .expect_member_status()
            .withf(|team_id: &str, user_id: &str| team_id == "team1" && user_id == "u1")
            .times(1)
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams);

        let result = create_team_template(
            &repo,
            &teams,
            CreateTeamTemplateParams {
                team_id: "team1".to_string(),
                requester_user_id: "u1".to_string(),
                name: "Groceries".to_string(),
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
                    item_type: ItemType::Template {
                        schedule: Schedule::default(),
                        recurrence: Recurrence::default(),
                        event_type: None,
                    },
                    ..Item::default()
                })
            });

        mock.expect_update()
            .withf(|item: &Item| {
                item.id == "tpl1"
                    && item.name == "New name"
                    && matches!(&item.item_type, ItemType::Template { event_type, .. } if event_type.as_deref() == Some("rain"))
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
                    item_type: ItemType::Task {
                        schedule: Schedule::default(),
                        recurrence: Recurrence::default(),
                        team_assignment: None,
                    },
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
                event_type: None,
            },
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn update_team_template_changes_name_and_event_type() {
        use crate::storage::sqlite::MockTeamRepo;

        let mut mock = MockItemRepo::new();

        mock.expect_get_team_item()
            .withf(|team_id: &str, item_id: &str| team_id == "team1" && item_id == "tpl1")
            .times(1)
            .returning(|_, _| {
                Ok(Item {
                    id: "tpl1".to_string(),
                    team_id: Some("team1".to_string()),
                    name: "Old name".to_string(),
                    item_type: ItemType::Template {
                        schedule: Schedule::default(),
                        recurrence: Recurrence::default(),
                        event_type: None,
                    },
                    ..Item::default()
                })
            });

        mock.expect_update_team_item()
            .withf(|item: &Item| {
                item.id == "tpl1"
                    && item.name == "New name"
                    && matches!(&item.item_type, ItemType::Template { event_type, .. } if event_type.as_deref() == Some("rain"))
            })
            .times(1)
            .returning(|_| Ok(()));

        let mut teams = MockTeamRepo::new();
        teams
            .expect_member_status()
            .withf(|team_id: &str, user_id: &str| team_id == "team1" && user_id == "u1")
            .times(1)
            .returning(|_, _| Ok(Some("ACTIVE".to_string())));

        let repo: Arc<dyn ItemRepo> = Arc::new(mock);
        let teams: Arc<dyn TeamRepo> = Arc::new(teams);

        update_team_template(
            &repo,
            &teams,
            UpdateTeamTemplateParams {
                team_id: "team1".to_string(),
                requester_user_id: "u1".to_string(),
                template_id: "tpl1".to_string(),
                name: "New name".to_string(),
                event_type: Some("rain".to_string()),
            },
        )
        .await
        .expect("should update team template");
    }
}
