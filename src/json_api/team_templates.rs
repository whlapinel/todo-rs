use super::{internal, not_found, to_sdk_item_type};
use crate::auth::AuthUser;
use crate::service::error::ItemError;
use crate::service::team_items::require_active_member;
use crate::service::templates::{self as template_service, CreateTeamTemplateParams};
use crate::storage::sqlite::{ItemRepo, ProjectRepo, TeamRepo};
use std::sync::Arc;
use todo_server_sdk::{error, input, output, server};

/// This legacy `CreateTeamTemplate` operation only carries `teamId`, not a
/// `projectId` (see `model/src/main/smithy/team.smithy`), but Stage 5 of
/// docs/team-id-removal-plan.md made `service::templates::create_team_template`
/// `project_id`-primary. Resolves the team's backing project via
/// `ProjectRepo::get_by_team` before delegating in — every team has had a backing
/// project since the Project abstraction shipped (see CLAUDE.md's Per-team roles
/// section), so `None` here would mean `input.team_id` doesn't actually name a real
/// team.
pub async fn create_team_template(
    input: input::CreateTeamTemplateInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(auth_user): server::Extension<AuthUser>,
) -> Result<output::CreateTeamTemplateOutput, error::CreateTeamTemplateError> {
    let project = projects
        .get_by_team(&input.team_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?
        .ok_or_else(|| error::CreateTeamTemplateError::from(not_found()))?;
    let template_id = template_service::create_team_template(
        &repo,
        &teams,
        &projects,
        CreateTeamTemplateParams {
            project_id: project.id,
            requester_user_id: auth_user.user_id,
            name: input.name,
            description: input.description,
            source_item_id: input.source_item_id,
            event_type: input.event_type,
        },
    )
    .await
    .map_err(|e| match e {
        ItemError::NotFound => error::CreateTeamTemplateError::from(not_found()),
        ItemError::Invalid(msg) | ItemError::Internal(msg) => {
            error::CreateTeamTemplateError::from(internal(msg))
        }
    })?;
    Ok(output::CreateTeamTemplateOutput { template_id })
}

/// Resolves `input.team_id` to its backing project via `ProjectRepo::get_by_team`
/// and calls the `project_id`-scoped `list_templates_by_project` — this legacy
/// `ListTeamTemplates` operation only carries `teamId` (see
/// `create_team_template`'s own doc comment above for why), and Stage 6 of
/// docs/team-id-removal-plan.md dropped `ItemRepo::list_team_templates`
/// (and the `items.team_id` column it queried) entirely.
pub async fn list_team_templates(
    input: input::ListTeamTemplatesInput,
    server::Extension(repo): server::Extension<Arc<dyn ItemRepo>>,
    server::Extension(teams): server::Extension<Arc<dyn TeamRepo>>,
    server::Extension(projects): server::Extension<Arc<dyn ProjectRepo>>,
    server::Extension(auth_user): server::Extension<AuthUser>,
) -> Result<output::ListTeamTemplatesOutput, error::ListTeamTemplatesError> {
    require_active_member(&teams, &input.team_id, &auth_user.user_id)
        .await
        .map_err(|e| match e {
            ItemError::NotFound => error::ListTeamTemplatesError::from(not_found()),
            ItemError::Invalid(msg) | ItemError::Internal(msg) => {
                error::ListTeamTemplatesError::from(internal(msg))
            }
        })?;
    let project = projects
        .get_by_team(&input.team_id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?
        .ok_or_else(|| error::ListTeamTemplatesError::from(not_found()))?;
    let items = repo
        .list_templates_by_project(&project.id)
        .await
        .map_err(|e| internal(format!("{e:?}")))?;
    let items = items
        .into_iter()
        .map(|i| todo_server_sdk::model::ItemSummary {
            item_id: Some(i.id.clone()),
            name: Some(i.name.clone()),
            description: i.description.clone(),
            due_date: None,
            scheduled_date: None,
            scheduled_end_date: None,
            complete: Some(i.complete()),
            has_due_time: Some(i.has_due_time()),
            has_scheduled_time: Some(i.has_scheduled_time()),
            has_end_time: Some(i.has_end_time()),
            parent_item_id: i.parent_item_id.clone(),
            has_children: Some(i.has_children),
            item_type: Some(to_sdk_item_type(i.kind())),
            event_type: i.event_type(),
            due_offset_days: i.due_offset_days(),
            assigned_to_user_id: None,
            source_event_id: None,
        })
        .collect();
    Ok(output::ListTeamTemplatesOutput { items })
}
