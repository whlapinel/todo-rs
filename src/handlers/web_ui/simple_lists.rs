use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemType};
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::service::items::{self as item_service, ItemError};
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::{ItemRepo, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being Simple — this screen's forms hardcode
/// `itemType: SIMPLE` on every create/update (no Kind selector, mirroring `tasks.rs`'s
/// `require_task`), so a Task/Event item's id reaching one of these handlers must 404 rather
/// than render a form that would silently reclassify it back to Simple on save.
fn require_simple(item: Item) -> Result<Item, ItemError> {
    if item.item_type == ItemType::Simple {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Deliberately no date/scheduling/recurrence/offset fields anywhere in this module —
// `Item::validate` rejects all of those for `ItemType::Simple`, so there is nothing for a
// form on this screen to ever legitimately set.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SimpleItemForm {
    name: Option<String>,
    complete: Option<String>,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    /// Present only on the standalone `/simple-lists/new` page's forms — see `items.rs`'s
    /// identical field for the full rationale.
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

fn overlay_bool(form_value: &Option<String>, current: bool) -> bool {
    match form_value.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => current,
    }
}

fn create_params_from_form(
    user_id: &str,
    form: &SimpleItemForm,
) -> item_service::CreateItemParams {
    item_service::CreateItemParams {
        user_id: user_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        complete: form.complete.as_deref().map(|s| s == "true"),
        parent_item_id: non_empty(&form.parent_item_id),
        item_type: Some(ItemType::Simple),
        ..Default::default()
    }
}

fn update_params_from_form(
    user_id: &str,
    item_id: &str,
    current: &Item,
    form: &SimpleItemForm,
) -> item_service::UpdateItemParams {
    item_service::UpdateItemParams {
        user_id: user_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        complete: overlay_bool(&form.complete, current.complete),
        parent_item_id: current.parent_item_id.clone(),
        item_type: Some(ItemType::Simple),
        ..Default::default()
    }
}

// ---- templates --------------------------------------------------------------

#[derive(Template)]
#[template(path = "simple_lists/row.html")]
struct SimpleItemRow {
    id: String,
    name: String,
    complete: bool,
    has_children: bool,
    toggle_complete_json: String,
}

impl SimpleItemRow {
    fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            has_children: item.has_children,
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "simple_lists/detail_fields.html")]
struct SimpleItemDetailFields {
    id: String,
    name: String,
    complete: bool,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    just_saved: bool,
}

impl SimpleItemDetailFields {
    fn from_item(item: &Item, just_saved: bool) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
            just_saved,
        }
    }
}

/// Read-only counterpart to `SimpleItemDetailFields` — see `items.rs`'s `DetailView` for the
/// row-editing convention this mirrors (complete-toggle lives here too).
#[derive(Template)]
#[template(path = "simple_lists/detail_view.html")]
struct SimpleItemDetailView {
    id: String,
    complete: bool,
    toggle_complete_json: String,
}

impl SimpleItemDetailView {
    fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            complete: item.complete,
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "simple_lists/rows_fragment.html")]
struct SimpleItemRowsFragmentTemplate {
    rows: Vec<String>,
    empty_message: String,
}

#[derive(Template)]
#[template(path = "simple_lists/list_page.html")]
struct SimpleListsListPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "simple_lists/new_page.html")]
struct NewSimpleItemPageTemplate {
    show_complete: bool,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "simple_lists/detail_page.html")]
struct SimpleItemDetailPageTemplate {
    id: String,
    name: String,
    view: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "simple_lists/edit_page.html")]
struct SimpleItemEditPageTemplate {
    id: String,
    name: String,
    fields: String,
    nav_html: String,
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(items: &[Item], show_complete: bool) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .map(|i| SimpleItemRow::from_item(i).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// `repo.list` already scopes to top-level, non-Template items — this narrows further to
/// `Simple`. No sort key: unlike Tasks/Events there's no date field to order by, so this
/// falls back to whatever order the repo returns (same as the generic `items.rs` screen).
async fn list_simple_items(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
) -> Result<Vec<Item>, ItemError> {
    let mut items = repo.list(user_id).await.map_err(ItemError::from)?;
    items.retain(|i| i.item_type == ItemType::Simple);
    Ok(items)
}

async fn render_scope_fragment(
    repo: &Arc<dyn ItemRepo>,
    user_id: &str,
    parent_item_id: Option<&str>,
    show_complete: bool,
) -> Result<Html<String>, ItemError> {
    let (items, empty_message) = if let Some(parent_id) = parent_item_id {
        (
            repo.list_children(parent_id)
                .await
                .map_err(ItemError::from)?,
            "No sub-items yet.",
        )
    } else {
        (list_simple_items(repo, user_id).await?, "No items yet.")
    };
    let rows = render_rows(&items, parent_item_id.is_some() || show_complete)?;
    render(SimpleItemRowsFragmentTemplate {
        rows,
        empty_message: empty_message.to_string(),
    })
}

// ---- handlers -----------------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowCompleteQuery {
    show_complete: Option<String>,
}

pub async fn simple_lists_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let show_complete = q.show_complete.is_some();
    let items = list_simple_items(&repo, &auth_user.user_id).await?;
    let rows = render_rows(&items, show_complete)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::SimpleLists,
    )
    .await?;
    render(SimpleListsListPageTemplate {
        rows,
        show_complete,
        nav_html,
    })
}

pub async fn new_simple_item_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::SimpleLists,
    )
    .await?;
    render(NewSimpleItemPageTemplate {
        show_complete: q.show_complete.is_some(),
        nav_html,
    })
}

pub async fn simple_item_detail_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_simple(item)?;
    let view = SimpleItemDetailView::from_item(&item).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::SimpleLists,
    )
    .await?;
    render(SimpleItemDetailPageTemplate {
        id: item.id,
        name: item.name,
        view,
        nav_html,
    })
}

pub async fn simple_item_edit_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_simple(item)?;
    let fields = SimpleItemDetailFields::from_item(&item, false).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::SimpleLists,
    )
    .await?;
    render(SimpleItemEditPageTemplate {
        id: item.id,
        name: item.name,
        fields,
        nav_html,
    })
}

pub async fn simple_item_children_fragment(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    // Ownership gate: list_children itself isn't scoped by user, so confirm the caller owns
    // the parent before listing its children.
    repo.get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let children = repo
        .list_children(&item_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&children, true)?;
    render(SimpleItemRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

/// Redirect back to the simple-lists list (via the `hx-redirect` header, same mechanism
/// `items.rs`'s `redirect_to_items` uses) after a create from the standalone
/// `/simple-lists/new` page.
fn redirect_to_simple_lists(show_complete: bool) -> Response {
    let location = if show_complete {
        "/web/simple-lists?showComplete=1".to_string()
    } else {
        "/web/simple-lists".to_string()
    };
    (
        [(
            axum::http::header::HeaderName::from_static("hx-redirect"),
            location,
        )],
        Html(String::new()),
    )
        .into_response()
}

pub async fn create_simple_item_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Form(form): Form<SimpleItemForm>,
) -> Result<Response, ItemError> {
    let show_complete = form.show_complete.is_some();
    let redirect = form.redirect.is_some();
    let params = create_params_from_form(&auth_user.user_id, &form);
    let parent_item_id = params.parent_item_id.clone();
    item_service::create_item(&repo, params).await?;
    if redirect {
        return Ok(redirect_to_simple_lists(show_complete));
    }
    Ok(
        render_scope_fragment(&repo, &auth_user.user_id, parent_item_id.as_deref(), show_complete)
            .await?
            .into_response(),
    )
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchForm {
    names: String,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    redirect: Option<String>,
}

pub async fn create_simple_items_batch(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Form(form): Form<BatchForm>,
) -> Result<Response, ItemError> {
    let parent_item_id = non_empty(&form.parent_item_id);
    for line in form.names.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let params = item_service::CreateItemParams {
            user_id: auth_user.user_id.clone(),
            name: name.to_string(),
            parent_item_id: parent_item_id.clone(),
            item_type: Some(ItemType::Simple),
            ..Default::default()
        };
        item_service::create_item(&repo, params).await?;
    }
    if form.redirect.is_some() {
        return Ok(redirect_to_simple_lists(form.show_complete.is_some()));
    }
    Ok(render_scope_fragment(
        &repo,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
    )
    .await?
    .into_response())
}

pub async fn update_simple_item_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Form(form): Form<SimpleItemForm>,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_simple(current)?;
    let params = update_params_from_form(&auth_user.user_id, &item_id, &current, &form);
    item_service::update_item(&repo, params).await?;

    // Unlike items.rs/tasks.rs, there's no "recurring item got replaced under a new id" case
    // to handle here — Item::validate rejects `recurrence` outright for ItemType::Simple, so
    // `next_recurrence` can never fire and the id above is guaranteed still valid.
    let updated = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let row = SimpleItemRow::from_item(&updated).render()?;
    let fields = SimpleItemDetailFields::from_item(&updated, true).render()?;
    let view = SimpleItemDetailView::from_item(&updated).render()?;
    Ok(Html(format!("{row}{fields}{view}")).into_response())
}

pub async fn delete_simple_item_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    require_simple(current)?;
    item_service::delete_item(&repo, &auth_user.user_id, &item_id).await?;
    Ok(Html(String::new()))
}

pub async fn save_simple_item_as_template(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    template_service::create_template(
        &repo,
        CreateTemplateParams {
            user_id: auth_user.user_id.clone(),
            name: item.name.clone(),
            source_item_id: Some(item_id),
            event_type: None,
        },
    )
    .await?;
    Ok(Html(
        r#"<span class="text-xs text-green-600">Saved</span>"#.to_string(),
    ))
}
