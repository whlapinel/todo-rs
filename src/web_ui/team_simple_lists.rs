use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use super::dashboard::{detail_url, list_url_for};
use super::nav::{self, ActiveContext, SidebarSection};
use crate::service::error::ItemError;
use crate::service::team_items::{
    self as team_item_service, require_active_member, CreateTeamItemParams, UpdateTeamItemContext,
    UpdateTeamItemParams,
};
use crate::service::teams as team_service;
use crate::storage::sqlite::{ActivityLogRepo, ItemRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being Simple, the same role
/// `simple_lists::require_simple` plays for the personal screen — this screen's forms
/// hardcode `itemType: SIMPLE` on every create/update (no Kind selector), so a Task/Event
/// team item's id reaching one of these handlers must 404 rather than silently reclassify
/// it back to Simple on save.
fn require_team_simple(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Simple {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Deliberately no date/scheduling/recurrence/offset/complete fields anywhere in this
// module — `Item::validate` rejects all of those (plus `complete: true`) for
// `ItemType::Simple`, so there is nothing for a form on this screen to ever legitimately
// set. Mirrors `simple_lists.rs`'s helper set. Simple items never carry assignment/points
// either (Task-only — see `service::team_items::build_item_type`), so this form has no
// fields for those.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamSimpleItemForm {
    name: Option<String>,
    description: Option<String>,
    parent_item_id: Option<String>,
    /// Set on the standalone `/team-simple-lists/:team_id/new` page's create forms and on
    /// every edit form's "Save and close" submission — see `tasks.rs`'s identical field for
    /// the full rationale.
    redirect: Option<String>,
}

fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn overlay_required_str(form_value: &Option<String>, current: &str) -> String {
    match form_value {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => current.to_string(),
    }
}

fn overlay_str(form_value: &Option<String>, current: Option<String>) -> Option<String> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
}

fn create_params_from_form(team_id: &str, form: &TeamSimpleItemForm) -> CreateTeamItemParams {
    CreateTeamItemParams {
        team_id: team_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        description: non_empty(&form.description),
        parent_item_id: non_empty(&form.parent_item_id),
        item_type: Some(ItemKind::Simple),
        ..Default::default()
    }
}

fn update_params_from_form(
    team_id: &str,
    item_id: &str,
    current: &Item,
    form: &TeamSimpleItemForm,
) -> UpdateTeamItemParams {
    UpdateTeamItemParams {
        team_id: team_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        description: overlay_str(&form.description, current.description.clone()),
        complete: false,
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(ItemKind::Simple),
        ..Default::default()
    }
}

// ---- templates --------------------------------------------------------------

#[derive(Template)]
#[template(path = "team_simple_lists/row.html")]
struct TeamSimpleItemRow {
    id: String,
    team_id: String,
    name: String,
    has_children: bool,
    /// See `tasks::TaskRow`'s identical field — populates this row's "subordinate under…"
    /// picker with its actual siblings (the other items rendered in the same list call).
    siblings: Vec<(String, String)>,
}

impl TeamSimpleItemRow {
    fn from_item(item: &Item, team_id: &str, siblings: &[&Item]) -> Self {
        Self {
            id: item.id.clone(),
            team_id: team_id.to_string(),
            name: item.name.clone(),
            has_children: item.has_children,
            siblings: siblings
                .iter()
                .filter(|s| s.id != item.id)
                .map(|s| (s.id.clone(), s.name.clone()))
                .collect(),
        }
    }
}

#[derive(Template)]
#[template(path = "team_simple_lists/detail_fields.html")]
struct TeamSimpleItemDetailFields {
    id: String,
    team_id: String,
    name: String,
    description: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    just_saved: bool,
}

impl TeamSimpleItemDetailFields {
    fn from_item(item: &Item, team_id: &str, just_saved: bool) -> Self {
        Self {
            id: item.id.clone(),
            team_id: team_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            just_saved,
        }
    }
}

#[derive(Template)]
#[template(path = "team_simple_lists/rows_fragment.html")]
struct TeamSimpleItemRowsFragmentTemplate {
    rows: Vec<String>,
    empty_message: String,
}

#[derive(Template)]
#[template(path = "team_simple_lists/list_page.html")]
struct TeamSimpleListsListPageTemplate {
    team_id: String,
    rows: Vec<String>,
    /// The viewer's own point balance on this team — see `service::teams::member_points`.
    points_label: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_simple_lists/new_page.html")]
struct NewTeamSimpleItemPageTemplate {
    team_id: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_simple_lists/detail_page.html")]
struct TeamSimpleItemDetailPageTemplate {
    id: String,
    team_id: String,
    name: String,
    description: Option<String>,
    is_top_level: bool,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "team_simple_lists/edit_page.html")]
struct TeamSimpleItemEditPageTemplate {
    id: String,
    team_id: String,
    name: String,
    fields: String,
    nav_html: String,
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(items: &[Item], team_id: &str) -> Result<Vec<String>, ItemError> {
    let all: Vec<&Item> = items.iter().collect();
    all.iter()
        .map(|i| TeamSimpleItemRow::from_item(i, team_id, &all).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// `repo.list_team_items` already scopes to top-level, non-Template items — this narrows
/// further to `Simple`. No sort key: unlike Tasks/Events there's no date field to order by,
/// mirroring `simple_lists.rs`'s `list_simple_items`.
async fn list_team_simple_items(
    repo: &Arc<dyn ItemRepo>,
    team_id: &str,
) -> Result<Vec<Item>, ItemError> {
    let mut items = repo
        .list_team_items(team_id, None)
        .await
        .map_err(ItemError::from)?;
    items.retain(|i| i.kind() == ItemKind::Simple);
    Ok(items)
}

/// The full sibling group (including the item itself) a given item belongs to — see
/// `tasks::sibling_group`'s identical rationale.
async fn sibling_group(
    repo: &Arc<dyn ItemRepo>,
    team_id: &str,
    parent_item_id: Option<&str>,
) -> Result<Vec<Item>, ItemError> {
    match parent_item_id {
        Some(pid) => repo.list_children(pid).await.map_err(ItemError::from),
        None => list_team_simple_items(repo, team_id).await,
    }
}

async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    team_id: &str,
    parent_item_id: Option<&str>,
) -> Result<Html<String>, ItemError> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            repo.list_children(parent_id)
                .await
                .map_err(ItemError::from)?,
            "No sub-items yet.",
        )
    } else {
        (
            list_team_simple_items(repo, team_id).await?,
            "No items yet.",
        )
    };
    let rows = render_rows(&items, team_id)?;
    render(TeamSimpleItemRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
}

// ---- handlers -----------------------------------------------------------------

pub async fn team_simple_lists_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let items = list_team_simple_items(&repo, &team_id).await?;
    let rows = render_rows(&items, &team_id)?;
    let points = team_service::member_points(&teams, &team_id, &auth_user.user_id).await?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(TeamSimpleListsListPageTemplate {
        team_id,
        rows,
        points_label: format!("{points} pts"),
        nav_html,
    })
}

pub async fn new_team_simple_item_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(NewTeamSimpleItemPageTemplate { team_id, nav_html })
}

pub async fn team_simple_item_detail_page(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let item = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_team_simple(item)?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(TeamSimpleItemDetailPageTemplate {
        is_top_level: item.parent_item_id.is_none(),
        id: item.id,
        team_id,
        name: item.name,
        description: item.description.clone(),
        nav_html,
    })
}

pub async fn team_simple_item_edit_page(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let item = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_team_simple(item)?;
    let fields = TeamSimpleItemDetailFields::from_item(&item, &team_id, false).render()?;
    let nav_html = nav::build_nav_html(
        &teams,
        &auth_user.user_id,
        ActiveContext::Team(team_id.clone()),
        SidebarSection::SimpleLists,
    )
    .await?;
    render(TeamSimpleItemEditPageTemplate {
        id: item.id,
        team_id,
        name: item.name,
        fields,
        nav_html,
    })
}

pub async fn team_simple_item_children_fragment(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    // Ownership gate: list_children isn't scoped by team, so confirm the parent actually
    // belongs to this team before listing its children (mirrors team_tasks.rs's equivalent).
    repo.get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let children = repo
        .list_children(&item_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&children, &team_id)?;
    render(TeamSimpleItemRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

/// Redirect back to the team's simple-lists list (via the `hx-redirect` header) after a
/// create from the standalone `/team-simple-lists/:team_id/new` page. Mirrors
/// `simple_lists.rs::redirect_to_simple_lists`.
fn redirect_to_team_simple_lists(team_id: &str) -> Response {
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            format!("/web/team-simple-lists/{team_id}"),
        )],
        Html(String::new()),
    )
        .into_response()
}

pub async fn create_team_simple_item_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<TeamSimpleItemForm>,
) -> Result<Response, ItemError> {
    let redirect = form.redirect.is_some();
    let params = create_params_from_form(&team_id, &form);
    let parent_item_id = params.parent_item_id.clone();
    team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;
    if redirect {
        return Ok(redirect_to_team_simple_lists(&team_id));
    }
    Ok(
        render_scope_fragment(&repo, &team_id, parent_item_id.as_deref())
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

pub async fn create_team_simple_items_batch(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<BatchForm>,
) -> Result<Response, ItemError> {
    let parent_item_id = non_empty(&form.parent_item_id);
    for line in form.names.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let params = CreateTeamItemParams {
            team_id: team_id.clone(),
            name: name.to_string(),
            parent_item_id: parent_item_id.clone(),
            item_type: Some(ItemKind::Simple),
            ..Default::default()
        };
        team_item_service::create_team_item(&repo, &teams, &auth_user.user_id, params).await?;
    }
    if form.redirect.is_some() {
        return Ok(redirect_to_team_simple_lists(&team_id));
    }
    Ok(
        render_scope_fragment(&repo, &team_id, parent_item_id.as_deref())
            .await?
            .into_response(),
    )
}

pub async fn update_team_simple_item_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Form(form): Form<TeamSimpleItemForm>,
) -> Result<Response, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let current = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_team_simple(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&team_id, &item_id, &current, &form);
    team_item_service::update_team_item(
        &repo,
        &UpdateTeamItemContext {
            teams: teams.clone(),
            activity_log,
        },
        &auth_user.user_id,
        params,
    )
    .await?;

    // Unlike team_tasks.rs/team_events.rs, there's no "recurring item got replaced under a
    // new id" case to handle here — Item::validate rejects `recurrence` outright for
    // ItemType::Simple, so `next_recurrence` can never fire and the id above is guaranteed
    // still valid.
    let updated = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    if close {
        let nav_html = nav::build_nav_html(
            &teams,
            &auth_user.user_id,
            ActiveContext::Team(team_id.clone()),
            SidebarSection::SimpleLists,
        )
        .await?;
        return Ok(render(TeamSimpleItemDetailPageTemplate {
            is_top_level: updated.parent_item_id.is_none(),
            id: updated.id.clone(),
            team_id,
            name: updated.name.clone(),
            description: updated.description.clone(),
            nav_html,
        })?
        .into_response());
    }
    let siblings =
        sibling_group(&repo, &team_id, updated.parent_item_id.as_deref()).await?;
    let siblings_ref: Vec<&Item> = siblings.iter().collect();
    let row = TeamSimpleItemRow::from_item(&updated, &team_id, &siblings_ref).render()?;
    let fields = TeamSimpleItemDetailFields::from_item(&updated, &team_id, true).render()?;
    Ok(Html(format!("{row}{fields}")).into_response())
}

pub async fn delete_team_simple_item_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    require_team_simple(current)?;
    team_item_service::delete_team_item(&repo, &teams, &auth_user.user_id, &team_id, &item_id)
        .await?;
    Ok(Html(String::new()))
}

/// Reparent-only update, every other field round-tripped from `current` — see
/// `simple_lists::reparent_params`. No offset recompute needed here either: Simple items can
/// never have a `dueOffsetDays` to begin with.
fn reparent_params(
    team_id: &str,
    item_id: &str,
    current: &Item,
    new_parent_item_id: Option<String>,
) -> UpdateTeamItemParams {
    UpdateTeamItemParams {
        team_id: team_id.to_string(),
        item_id: item_id.to_string(),
        name: current.name.clone(),
        description: current.description.clone(),
        complete: false,
        parent_item_id: new_parent_item_id,
        item_type: Some(ItemKind::Simple),
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
pub async fn promote_team_simple_item_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
) -> Result<Response, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let current = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_team_simple(current)?;
    let Some(parent_id) = current.parent_item_id.clone() else {
        return Err(ItemError::Invalid(
            "item has no parent to promote from".to_string(),
        ));
    };
    let parent = repo
        .get_team_item(&team_id, &parent_id)
        .await
        .map_err(ItemError::from)?;
    let grandparent = match parent.parent_item_id {
        Some(gp_id) => Some(
            repo.get_team_item(&team_id, &gp_id)
                .await
                .map_err(ItemError::from)?,
        ),
        None => None,
    };
    let params = reparent_params(
        &team_id,
        &item_id,
        &current,
        grandparent.as_ref().map(|gp| gp.id.clone()),
    );
    team_item_service::update_team_item(
        &repo,
        &UpdateTeamItemContext {
            teams: teams.clone(),
            activity_log,
        },
        &auth_user.user_id,
        params,
    )
    .await?;

    let location = match &grandparent {
        Some(gp) => detail_url(gp),
        None => list_url_for(ItemKind::Simple, Some(&team_id)),
    };
    Ok(hx_redirect(location))
}

#[derive(serde::Deserialize, Debug)]
pub struct SubordinateForm {
    new_parent_id: String,
}

/// Subordinates a sibling to become a child of another sibling — see
/// `tasks::subordinate_task_form`.
pub async fn subordinate_team_simple_item_form(
    Path((team_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Form(form): Form<SubordinateForm>,
) -> Result<Response, ItemError> {
    require_active_member(&teams, &team_id, &auth_user.user_id).await?;
    let current = repo
        .get_team_item(&team_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_team_simple(current)?;
    let new_parent = repo
        .get_team_item(&team_id, &form.new_parent_id)
        .await
        .map_err(ItemError::from)?;
    if new_parent.parent_item_id != current.parent_item_id {
        return Err(ItemError::Invalid(
            "target is not a sibling of this item".to_string(),
        ));
    }
    let params = reparent_params(&team_id, &item_id, &current, Some(new_parent.id.clone()));
    team_item_service::update_team_item(
        &repo,
        &UpdateTeamItemContext {
            teams: teams.clone(),
            activity_log,
        },
        &auth_user.user_id,
        params,
    )
    .await?;
    Ok(hx_redirect(detail_url(&new_parent)))
}
