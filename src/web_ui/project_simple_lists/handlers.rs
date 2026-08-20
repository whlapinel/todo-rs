use crate::auth::AuthUser;
use crate::domain::item::Item;
use crate::service::error::ItemError;
use crate::service::project_items::{self as project_item_service};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_simple_lists::templates::*;
use crate::web_ui::project_simple_lists::{
    ProjectSimpleItemForm, create_params_from_form, list_project_simple_items, non_empty, render,
    render_scope_fragment, require_simple, sibling_group, update_params_from_form,
};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

pub async fn project_simple_lists_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let items = list_project_simple_items(&repo, &project_id).await?;
    let rows = render_rows_scoped(&items, &project_id)?;
    let points_label = match &project.team_id {
        Some(team_id) => {
            let points = team_service::member_points(&teams, team_id, &auth_user.user_id).await?;
            Some(format!("{points} pts"))
        }
        None => None,
    };
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(ProjectSimpleListsListPageTemplate {
        project_id,
        rows,
        points_label,
        nav_html,
    })
}

// Thin re-export so this module doesn't need `crate::web_ui::project_simple_lists::render_rows`
// imported alongside the identically-named local helpers above.
fn render_rows_scoped(items: &[Item], project_id: &str) -> Result<Vec<String>, ItemError> {
    crate::web_ui::project_simple_lists::render_rows(items, project_id)
}

pub async fn new_project_simple_item_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let _project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(NewProjectSimpleItemPageTemplate {
        project_id,
        nav_html,
    })
}

pub async fn project_simple_item_detail_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let item = require_simple(item)?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(ProjectSimpleItemDetailPageTemplate {
        is_top_level: item.parent_item_id.is_none(),
        id: item.id,
        project_id,
        name: item.name,
        description: item.description.clone(),
        nav_html,
    })
}

pub async fn project_simple_item_edit_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let item = require_simple(item)?;
    let fields = ProjectSimpleItemDetailFields::from_item(&item, &project_id, false).render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(ProjectSimpleItemEditPageTemplate {
        id: item.id,
        project_id,
        name: item.name,
        fields,
        nav_html,
    })
}

pub async fn project_simple_item_children_fragment(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    // Ownership gate: confirm the parent actually belongs to this project before listing its
    // children (mirrors project_tasks.rs's equivalent).
    project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let children = project_item_service::list_project_items_unchecked(
        &repo,
        &project_id,
        Some(item_id.clone()),
    )
    .await?;
    let rows = render_rows_scoped(&children, &project_id)?;
    render(ProjectSimpleItemRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

/// Redirect back to the project's simple-lists list (via the `hx-redirect` header) after a
/// create from the standalone `/projects/:project_id/simple-lists/new` page. Mirrors
/// `simple_lists::redirect_to_simple_lists`/`team_simple_lists::redirect_to_team_simple_lists`.
fn redirect_to_project_simple_lists(project_id: &str) -> Response {
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            format!("/web/projects/{project_id}/simple-lists"),
        )],
        Html(String::new()),
    )
        .into_response()
}

pub async fn create_project_simple_item_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<ProjectSimpleItemForm>,
) -> Result<Response, ItemError> {
    let redirect = form.redirect.is_some();
    let params = create_params_from_form(&project_id, &form);
    let parent_item_id = params.parent_item_id.clone();
    project_item_service::create_project_item(&repo, &projects, &teams, &auth_user.user_id, params)
        .await?;
    if redirect {
        return Ok(redirect_to_project_simple_lists(&project_id));
    }
    Ok(
        render_scope_fragment(&repo, &project_id, parent_item_id.as_deref())
            .await?
            .into_response(),
    )
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchForm {
    names: String,
    parent_item_id: Option<String>,
    redirect: Option<String>,
}

pub async fn create_project_simple_items_batch(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<BatchForm>,
) -> Result<Response, ItemError> {
    let parent_item_id = non_empty(&form.parent_item_id);
    for line in form.names.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let params = crate::service::project_items::CreateProjectItemParams {
            project_id: project_id.clone(),
            name: name.to_string(),
            parent_item_id: parent_item_id.clone(),
            item_type: Some(crate::domain::item::ItemKind::Simple),
            ..Default::default()
        };
        project_item_service::create_project_item(
            &repo,
            &projects,
            &teams,
            &auth_user.user_id,
            params,
        )
        .await?;
    }
    if form.redirect.is_some() {
        return Ok(redirect_to_project_simple_lists(&project_id));
    }
    Ok(
        render_scope_fragment(&repo, &project_id, parent_item_id.as_deref())
            .await?
            .into_response(),
    )
}

pub async fn update_project_simple_item_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn crate::storage::sqlite::ActivityLogRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Form(form): Form<ProjectSimpleItemForm>,
) -> Result<Response, ItemError> {
    let current = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let current = require_simple(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&project_id, &item_id, &current, &form);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &event_series,
        &auth_user.user_id,
        params,
    )
    .await?;

    // Unlike project_tasks.rs/project_events.rs, there's no "recurring item got replaced
    // under a new id" case to handle here — Item::validate rejects `recurrence` outright for
    // ItemType::Simple, so `next_recurrence` can never fire and the id above is guaranteed
    // still valid.
    let updated =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    if close {
        let nav_html = nav::build_nav_html(
            &projects,
            &auth_user.user_id,
            active_context(&project_id),
            SidebarSection::SimpleLists,
        )
        .await?;
        return Ok(render(ProjectSimpleItemDetailPageTemplate {
            is_top_level: updated.parent_item_id.is_none(),
            id: updated.id.clone(),
            project_id,
            name: updated.name.clone(),
            description: updated.description.clone(),
            nav_html,
        })?
        .into_response());
    }
    let siblings = sibling_group(&repo, &project_id, updated.parent_item_id.as_deref()).await?;
    let siblings_ref: Vec<&Item> = siblings.iter().collect();
    let row = crate::web_ui::project_simple_lists::templates::ProjectSimpleItemRow::from_item(
        &updated,
        &project_id,
        &siblings_ref,
    )
    .render()?;
    let fields = ProjectSimpleItemDetailFields::from_item(&updated, &project_id, true).render()?;
    Ok(Html(format!("{row}{fields}")).into_response())
}

pub async fn delete_project_simple_item_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    require_simple(current)?;
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &event_series,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    Ok(Html(String::new()))
}

/// Reparent-only update, every other field round-tripped from `current` — see
/// `project_tasks::reparent_params` for the general shape (no offset recompute needed here:
/// `Item::validate` rejects `dueOffsetDays`/any date field outright for `ItemType::Simple`,
/// so a Simple item can never have one to begin with).
fn reparent_params(
    project_id: &str,
    item_id: &str,
    current: &Item,
    new_parent_item_id: Option<String>,
) -> crate::service::project_items::UpdateProjectItemParams {
    crate::service::project_items::UpdateProjectItemParams {
        project_id: project_id.to_string(),
        item_id: item_id.to_string(),
        name: current.name.clone(),
        description: current.description.clone(),
        complete: false,
        parent_item_id: new_parent_item_id,
        item_type: Some(crate::domain::item::ItemKind::Simple),
        ..Default::default()
    }
}

fn hx_redirect(location: String) -> Response {
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            location,
        )],
        Html(String::new()),
    )
        .into_response()
}

/// Promotes a child item to a sibling of its own parent — see `tasks::promote_task_form`.
pub async fn promote_project_simple_item_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn crate::storage::sqlite::ActivityLogRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Response, ItemError> {
    let target = project_item_service::resolve_promotion_target(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let current = require_simple(target.current)?;
    let grandparent = target.grandparent;
    let params = reparent_params(
        &project_id,
        &item_id,
        &current,
        grandparent.as_ref().map(|gp| gp.id.clone()),
    );
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &event_series,
        &auth_user.user_id,
        params,
    )
    .await?;

    let location = match &grandparent {
        Some(gp) => format!("/web/projects/{project_id}/simple-lists/{}", gp.id),
        None => format!("/web/projects/{project_id}/simple-lists"),
    };
    Ok(hx_redirect(location))
}

#[derive(serde::Deserialize, Debug)]
pub struct SubordinateForm {
    new_parent_id: String,
}

/// Subordinates a sibling to become a child of another sibling — see
/// `tasks::subordinate_task_form`.
pub async fn subordinate_project_simple_item_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn crate::storage::sqlite::ActivityLogRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Form(form): Form<SubordinateForm>,
) -> Result<Response, ItemError> {
    let target = project_item_service::resolve_subordination_target(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
        &form.new_parent_id,
    )
    .await?;
    let current = require_simple(target.current)?;
    let new_parent = target.new_parent;
    let params = reparent_params(&project_id, &item_id, &current, Some(new_parent.id.clone()));
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &event_series,
        &auth_user.user_id,
        params,
    )
    .await?;
    Ok(hx_redirect(format!(
        "/web/projects/{project_id}/simple-lists/{}",
        new_parent.id
    )))
}
