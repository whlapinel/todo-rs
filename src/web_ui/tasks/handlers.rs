use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::web_ui::tasks::{TaskForm, build_calendar_days, create_params_from_form, list_tasks, next_month, non_empty, prev_month, render, render_rows, render_scope_fragment, require_task, sibling_group, update_params_from_form};
use super::super::dashboard::{detail_url, list_url_for};
use super::super::nav::{self, ActiveContext, SidebarSection};
use super::super::{TzOffset, to_local};
use crate::service::items::{self as item_service, ItemError, top_level_anchor};
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::sync::Arc;
use super::templates::*;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowCompleteQuery {
    show_complete: Option<String>,
}

pub async fn tasks_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let show_complete = q.show_complete.is_some();
    let items = list_tasks(&repo, &auth_user.user_id).await?;
    let rows = render_rows(&items, show_complete, tz)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(TasksListPageTemplate {
        rows,
        show_complete,
        nav_html,
    })
}

pub async fn new_task_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(NewTaskPageTemplate {
        show_complete: q.show_complete.is_some(),
        blank_recurrence: None,
        blank_recurrence_basis: Some("SCHEDULED_DATE".to_string()),
        blank_due_offset_days_input: String::new(),
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
        nav_html,
    })
}

#[derive(serde::Deserialize)]
pub struct CalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
}

pub async fn tasks_calendar_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<CalendarQuery>,
) -> Result<Html<String>, ItemError> {
    let today = to_local(Utc::now(), tz).date_naive();
    let year = q.year.unwrap_or_else(|| today.year());
    let month = q
        .month
        .filter(|m| (1..=12).contains(m))
        .unwrap_or_else(|| today.month());

    let items = list_tasks(&repo, &auth_user.user_id).await?;
    let days = build_calendar_days(year, month, &items, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;

    render(TasksCalendarPageTemplate {
        month_label: NaiveDate::from_ymd_opt(year, month, 1)
            .unwrap()
            .format("%B %Y")
            .to_string(),
        month_iso: format!("{year:04}-{month:02}"),
        prev_year,
        prev_month,
        next_year,
        next_month,
        days,
        nav_html,
    })
}

pub async fn task_detail_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_task(item)?;
    let linked_event = resolve_linked_event(&repo, &auth_user.user_id, &item).await?;
    let view = TaskDetailView::from_item(&item, tz, linked_event).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(TaskDetailPageTemplate {
        id: item.id,
        name: item.name,
        complete: item.complete,
        view,
        nav_html,
    })
}

pub async fn task_edit_page(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let item = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let item = require_task(item)?;
    let fields = TaskDetailFields::from_item(&item, tz, false).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Tasks,
    )
    .await?;
    render(TaskEditPageTemplate {
        id: item.id,
        name: item.name,
        fields,
        nav_html,
    })
}

/// Renders a parent item's children as `TaskRow`s — children of any dedicated screen's item
/// (Task, Event, Simple) are always plain `Task`-typed, so this is reused directly by
/// `events.rs`/`team_events.rs` (which have no children concept of their own) rather than
/// duplicating `TaskRow`/its template there. Callers are responsible for their own
/// ownership gate before calling this (see `task_children_fragment` below).
pub(crate) async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    parent_item_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let children = repo
        .list_children(parent_item_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&children, true, tz)?;
    render(TaskRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

/// Renders every task that references `event_id` via `sourceEventId` as `TaskRow`s — the
/// reference-based counterpart to `render_children_fragment` above, used by an Event's own
/// "Linked tasks" section. Reuses `render_rows` (and so computes real siblings among the
/// linked tasks themselves), but that's harmless: `TaskRow`'s "Move under…" picker is hidden
/// unconditionally for a `sourceEventId`-linked row regardless of its siblings list (see
/// `TaskRow::is_source_event_linked` and `Item::validate`'s "can't have both a parent and an
/// event reference" rule) — subordinating one here would create exactly that conflict.
pub(crate) async fn render_source_event_fragment(
    repo: &Arc<dyn ItemRepo>,
    event_id: &str,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let tasks = repo
        .list_by_source_event(event_id)
        .await
        .map_err(ItemError::from)?;
    let rows = render_rows(&tasks, true, tz)?;
    render(TaskRowsFragmentTemplate {
        rows,
        empty_message: "No linked tasks yet.".to_string(),
    })
}

pub async fn task_children_fragment(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    // Ownership gate: list_children itself isn't scoped by user, so confirm the caller owns
    // the parent before listing its children.
    repo.get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    render_children_fragment(&repo, &item_id, tz).await
}

/// Redirect back to the tasks list (via the `hx-redirect` header, same mechanism
/// `items.rs`'s `redirect_to_items` uses) after a create from the standalone `/tasks/new`
/// page.
fn redirect_to_tasks(show_complete: bool) -> Response {
    let location = if show_complete {
        "/web/tasks?showComplete=1".to_string()
    } else {
        "/web/tasks".to_string()
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

pub async fn create_task_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TaskForm>,
) -> Result<Response, ItemError> {
    let show_complete = form.show_complete.is_some();
    let redirect = form.redirect.is_some();
    let params = create_params_from_form(&auth_user.user_id, &form, tz);
    let parent_item_id = params.parent_item_id.clone();
    item_service::create_item(&repo, params).await?;
    if redirect {
        return Ok(redirect_to_tasks(show_complete));
    }
    Ok(render_scope_fragment(
        &repo,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        show_complete,
        tz,
    )
    .await?
    .into_response())
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchForm {
    names: String,
    parent_item_id: Option<String>,
    show_complete: Option<String>,
    redirect: Option<String>,
}

pub async fn create_tasks_batch(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
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
            item_type: Some(ItemKind::Task),
            timezone_offset_minutes: Some(tz),
            ..Default::default()
        };
        item_service::create_item(&repo, params).await?;
    }
    if form.redirect.is_some() {
        return Ok(redirect_to_tasks(form.show_complete.is_some()));
    }
    Ok(render_scope_fragment(
        &repo,
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
        tz,
    )
    .await?
    .into_response())
}

pub async fn update_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<TaskForm>,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&auth_user.user_id, &item_id, &current, &form, tz);
    item_service::update_item(&repo, params).await?;

    match repo.get(&auth_user.user_id, &item_id).await {
        Ok(updated) if close => {
            let linked_event = resolve_linked_event(&repo, &auth_user.user_id, &updated).await?;
            let view = TaskDetailView::from_item(&updated, tz, linked_event).render()?;
            let nav_html = nav::build_nav_html(
                &team_repo,
                &auth_user.user_id,
                ActiveContext::Personal,
                SidebarSection::Tasks,
            )
            .await?;
            Ok(render(TaskDetailPageTemplate {
                id: updated.id.clone(),
                name: updated.name.clone(),
                complete: updated.complete,
                view,
                nav_html,
            })?
            .into_response())
        }
        Ok(updated) => {
            let siblings =
                sibling_group(&repo, &auth_user.user_id, updated.parent_item_id.as_deref()).await?;
            let siblings_ref: Vec<&Item> = siblings.iter().collect();
            let row = TaskRow::from_item(&updated, &siblings_ref, tz).render()?;
            let fields = TaskDetailFields::from_item(&updated, tz, true).render()?;
            let linked_event = resolve_linked_event(&repo, &auth_user.user_id, &updated).await?;
            let view = TaskDetailView::from_item(&updated, tz, linked_event).render()?;
            Ok(Html(format!("{row}{fields}{view}")).into_response())
        }
        // The task was recurring, just got marked complete, and the service layer replaced
        // it with a fresh successor under a new id (see service::items::update_item) — same
        // situation `items.rs`'s `update_item_form` handles, and the same fix: ask the client
        // to reload rather than guessing at the new id.
        Err(RepoError::NotFound) => Ok((
            [(
                axum::http::header::HeaderName::from_static("hx-refresh"),
                "true",
            )],
            Html(String::new()),
        )
            .into_response()),
        Err(e) => Err(ItemError::from(e)),
    }
}

pub async fn delete_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    require_task(current)?;
    item_service::delete_item(&repo, &auth_user.user_id, &item_id).await?;
    Ok(Html(String::new()))
}

/// Builds the params for a reparent-only update — every other field round-tripped unchanged
/// from `current`, same convention `dashboard.rs`'s checkbox-toggle handlers already use for
/// their own single-field updates. Shared by `promote_task_form`/`subordinate_task_form`.
///
/// A reparent changes which top-level item a `due_offset_days`-bearing item's own due date is
/// measured from, so `due_date`/`due_offset_days` aren't simply round-tripped like everything
/// else here: `offset_anchor` (the new top-level ancestor's own anchor — see
/// `top_level_anchor`, which callers must resolve *before* calling this, since walking the
/// parent chain needs `repo` and this function is deliberately kept sync) becomes the new
/// offset root, and the due date is recomputed against it — the same `deadline_from_offset`
/// math `sync_offset_children` uses to keep a child's due date pinned to its ancestor, applied
/// once up front here since a plain reparent doesn't touch `current`'s own anchor and so would
/// never trigger `update_item`'s own automatic sync otherwise. `new_parent_item_id: None`
/// (promoting all the way to top level) clears the offset entirely — there's no longer
/// anything to offset from, and top-level items don't carry one — but leaves the item's
/// last-known due date alone as its own now-independent deadline rather than blanking it. An
/// item with no offset in the first place round-trips its due date unchanged either way,
/// matching `sync_offset_children`'s own "manually-dated child is left alone" rule. If
/// `offset_anchor` is `None` (the new top-level ancestor has no due/scheduled date of its own),
/// the due date is cleared (`None`) rather than left stale, since there's nothing left to
/// compute it against.
fn reparent_params(
    user_id: &str,
    item_id: &str,
    current: &Item,
    new_parent_item_id: Option<String>,
    offset_anchor: Option<DateTime<Utc>>,
    tz: i32,
) -> item_service::UpdateItemParams {
    let (due_date, due_offset_days) = match (current.due_offset_days(), &new_parent_item_id) {
        (None, _) => (current.due_date(), None),
        (Some(_), Some(_)) => (
            offset_anchor.and_then(|anchor| current.deadline_from_offset(anchor, tz)),
            current.due_offset_days(),
        ),
        (Some(_), None) => (current.due_date(), None),
    };
    item_service::UpdateItemParams {
        user_id: user_id.to_string(),
        item_id: item_id.to_string(),
        name: current.name.clone(),
        description: current.description.clone(),
        due_date,
        scheduled_date: current.scheduled_date(),
        scheduled_end_date: current.scheduled_end_date(),
        complete: current.complete,
        recurrence: current.recurrence_pattern(),
        recurrence_basis: current.recurrence_basis(),
        has_due_time: Some(current.has_due_time()),
        has_scheduled_time: Some(current.has_scheduled_time()),
        has_end_time: Some(current.has_end_time()),
        parent_item_id: new_parent_item_id,
        item_type: Some(current.kind()),
        event_type: current.event_type(),
        due_offset_days,
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
    }
}

/// Tells the client to navigate to `location` via a fresh GET — same `hx-redirect` mechanism
/// `redirect_to_tasks` above uses, reused here because the destination of a promote/subordinate
/// can be any of the eight dedicated screens (whichever type the new parent turns out to be),
/// not just this one — see `dashboard::detail_url`/`list_url_for`.
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

/// Promotes a child item to a sibling of its own parent (i.e. moves it up one level in the
/// hierarchy) — a no-picker convenience action since the destination is fully determined by
/// the item's current position. Rejects items that have no parent to promote from.
pub async fn promote_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let Some(parent_id) = current.parent_item_id.clone() else {
        return Err(ItemError::Invalid(
            "item has no parent to promote from".to_string(),
        ));
    };
    let parent = repo
        .get(&auth_user.user_id, &parent_id)
        .await
        .map_err(ItemError::from)?;
    let grandparent = match parent.parent_item_id {
        Some(gp_id) => Some(
            repo.get(&auth_user.user_id, &gp_id)
                .await
                .map_err(ItemError::from)?,
        ),
        None => None,
    };
    let offset_anchor = match &grandparent {
        Some(gp) => top_level_anchor(&repo, &auth_user.user_id, gp).await?,
        None => None,
    };
    let params = reparent_params(
        &auth_user.user_id,
        &item_id,
        &current,
        grandparent.as_ref().map(|gp| gp.id.clone()),
        offset_anchor,
        tz,
    );
    item_service::update_item(&repo, params).await?;

    let location = match &grandparent {
        Some(gp) => detail_url(gp),
        None => list_url_for(current.kind(), None),
    };
    Ok(hx_redirect(location))
}

#[derive(serde::Deserialize, Debug)]
pub struct SubordinateForm {
    new_parent_id: String,
}

/// Subordinates a sibling to become a child of another sibling — `new_parent_id` must
/// currently share this item's own `parent_item_id` (enforced below), so this can only ever
/// move an item within its existing sibling group, never to an arbitrary item elsewhere in
/// the tree (and, by construction, can never create a cycle — a sibling can't already be this
/// item's own ancestor or descendant).
pub async fn subordinate_task_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<SubordinateForm>,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_task(current)?;
    let new_parent = repo
        .get(&auth_user.user_id, &form.new_parent_id)
        .await
        .map_err(ItemError::from)?;
    if new_parent.parent_item_id != current.parent_item_id {
        return Err(ItemError::Invalid(
            "target is not a sibling of this item".to_string(),
        ));
    }
    let offset_anchor = top_level_anchor(&repo, &auth_user.user_id, &new_parent).await?;
    let params = reparent_params(
        &auth_user.user_id,
        &item_id,
        &current,
        Some(new_parent.id.clone()),
        offset_anchor,
        tz,
    );
    item_service::update_item(&repo, params).await?;
    Ok(hx_redirect(detail_url(&new_parent)))
}

pub async fn save_task_as_template(
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
            description: None,
            source_item_id: Some(item_id),
            event_type: None,
        },
    )
    .await?;
    Ok(Html(
        r#"<span class="text-xs text-green-600">Saved</span>"#.to_string(),
    ))
}
