use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::domain::project::Project;
use crate::service::attachments as attachments_service;
use crate::service::comments as comments_service;
use crate::service::error::ItemError;
use crate::service::item_dependencies::{self as item_dependencies_service};
use crate::service::item_series::{self as item_series_service};
use crate::service::project_items::{self as project_item_service, UpdateProjectItemParams};
use crate::service::projects::{self as project_service};
use crate::service::teams as team_service;
use crate::service::templates::{self as template_service, CreateProjectTemplateParams};
use crate::storage::attachment_store::AttachmentStore;
use crate::storage::sqlite::{
    ActivityLogRepo, AttachmentRepo, CommentRepo, ItemDependencyRepo, ItemRepo, ItemSeriesRepo,
    ProjectRepo, ReminderRepo, TeamRepo, UserRepo,
};
use crate::web_ui::TzOffset;
use crate::web_ui::list_filters::{ListFilterQuery, ListFilters};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_tasks::templates::*;
use crate::web_ui::project_tasks::{
    ProjectTaskForm, active_member_options, create_params_from_form, list_filters_from_parts,
    names_for, non_empty, overlay_due_date, overlay_has_due_time, overlay_scheduled_date,
    overlay_scheduled_end_date, parse_days_before_due, render, render_scope_fragment, require_task,
    sibling_group, update_params_from_form,
};
use askama::Template;
use axum::extract::{Extension, Form, Multipart, Path, Query};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn project_task_url(project_id: &str, item_id: &str) -> String {
    format!("/web/projects/{project_id}/tasks/{item_id}")
}

fn project_tasks_list_url(project_id: &str) -> String {
    format!("/web/projects/{project_id}/tasks")
}

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

/// Dispatches to the project-scoped (`team_items::resolve_offset_anchor_project`) or personal
/// (`items::resolve_offset_anchor`) anchor resolution depending on `project.team_id`, the same
/// way `update_project_item` dispatches the update itself (`src/service/project_items.rs`). A
/// team-backed project's items carry a `NULL` `user_id` column (they're scoped by `project_id`,
/// not `user_id`), so calling the personal, `user_id`-scoped `resolve_offset_anchor` against one
/// — as every call site here used to, unconditionally — makes its internal `repo.get(user_id,
/// parent_id)` look up the parent via `WHERE id = ? AND user_id = ?`, binding
/// `auth_user.user_id` against a row whose `user_id` is `NULL`: SQL's three-valued logic means
/// that comparison is never true, so `RepoError::NotFound` bubbles up as `ItemError::NotFound`
/// even though the item's own read/write had already succeeded. See the "completing an item
/// returns a not found error" bug in docs/issues_and_features.md — it only reproduced for a
/// team-backed sub-item (or an event-linked task), since `resolve_offset_anchor` only ever calls
/// `repo.get` when the item has a `parentItemId` or `sourceEventId` to resolve.
async fn resolve_task_anchor_date(
    repo: &Arc<dyn ItemRepo>,
    project: &Project,
    requester_user_id: &str,
    item: &Item,
) -> Result<Option<DateTime<Utc>>, ItemError> {
    match &project.team_id {
        Some(_) => {
            crate::service::team_items::resolve_offset_anchor_project(repo, &project.id, item).await
        }
        None => {
            Ok(crate::service::items::resolve_offset_anchor(repo, requester_user_id, item).await?)
        }
    }
}

/// See `NewProjectTaskPageTemplate::redirect_after_create`'s doc comment — `?redirect=1` on
/// the `GET .../tasks/new` (or `.../events/new`, shared with `project_events::handlers`) request
/// opens the dialog in "no list underneath" mode.
#[derive(serde::Deserialize)]
pub struct NewItemQuery {
    pub redirect: Option<String>,
}

/// `?select=1` (docs/issues_and_features.md's "Multi-select" item) puts the Tasks list into
/// select mode — a separate query struct (like `NewItemQuery` above) rather than a field on
/// `ListFilterQuery`, since this isn't a filter dimension shared with any other screen.
#[derive(serde::Deserialize, Default)]
pub struct SelectModeQuery {
    select: Option<String>,
}

pub async fn project_tasks_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ListFilterQuery>,
    Query(sq): Query<SelectModeQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let filters = ListFilters::from_query(q);
    let select_mode = sq.select.is_some();
    // Stage 10 gap 2: a Task series' current occurrence is range-independent (a pure
    // function of the cursor), so any degenerate range surfaces it — see
    // `list_virtual_occurrences_for_project_unchecked`'s backlog-exemption logic (which
    // `list_occurrence_states_for_project` mirrors — see its own doc comment).
    //
    // Stage B of docs/unify-virtual-materialized-occurrences-plan.md switched this from
    // `list_virtual_occurrences_for_project_unchecked` to `list_occurrence_states_for_project`
    // purely for the shared `ProjectOccurrence` type `ProjectTaskVirtualRow` now takes — the
    // `is_current` filter below means this view's actual visible behavior is unchanged: by
    // construction (`require_current_occurrence`'s self-heal, see `service::item_series`) the
    // series' current occurrence is never itself materialized or skipped, so the trailing
    // `!Materialized` filter never has anything to exclude here in practice. A just-skipped
    // occurrence stops being current the instant it's settled, so it (correctly) has nothing
    // to show in this current-only view — see the Calendar screens for where a
    // skipped occurrence's struck-through Unskip row actually appears.
    //
    // The actual assembly now lives in `super::list_task_rows_for_project`, shared with the
    // in-place checkbox/Skip/Unskip handlers so a mutation's response and this initial page
    // load never drift apart.
    //
    // Select mode (`select_mode`) renders a completely different, minimal row shape instead
    // (`super::render_select_rows` — see `templates::ProjectTaskSelectRow`'s doc comment for
    // why it doesn't reuse `list_task_rows_for_project`/`Row` at all) — virtual/series
    // occurrences are skipped entirely there since they have no real item id to select yet.
    let rows = if select_mode {
        let items = super::list_project_tasks(&repo, &project_id).await?;
        super::render_select_rows(
            &items,
            &filters,
            &auth_user.user_id,
            project.team_id.is_some(),
            tz,
        )?
    } else {
        super::list_task_rows_for_project(
            &repo,
            &teams,
            &users,
            &series,
            &project_id,
            project.team_id.as_deref(),
            &auth_user.user_id,
            &filters,
            tz,
            None,
            &item_dependencies,
        )
        .await?
    };
    let (points_label, assignee_options) = match &project.team_id {
        Some(team_id) => {
            let points = team_service::member_points(&teams, team_id, &auth_user.user_id).await?;
            let assignee_options =
                active_member_options(&teams, team_id, &auth_user.user_id).await?;
            (Some(format!("{points} pts")), assignee_options)
        }
        None => (None, Vec::new()),
    };
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTasksListPageTemplate {
        project_id,
        rows,
        show_complete: filters.show_complete,
        is_team_project: project.team_id.is_some(),
        assigned_to: filters.assigned_to.as_value(),
        assignee_options,
        due_date: filters.due_date.as_value().to_string(),
        schedule: filters.schedule.as_value().to_string(),
        recurring: filters.recurring,
        priority: filters.priority.as_value(),
        filters_query: filters.query_string(),
        points_label,
        select_mode,
        nav_html,
    })
}

pub async fn new_project_task_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ListFilterQuery>,
    Query(nq): Query<NewItemQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let is_team_project = project.team_id.is_some();
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(&teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    let filters = ListFilters::from_query(q);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(NewProjectTaskPageTemplate {
        project_id,
        show_complete: filters.show_complete,
        is_team_project,
        assignee_options,
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
        blank_due_date_input: String::new(),
        blank_due_time_input: String::new(),
        is_team_admin,
        blank_points_input: String::new(),
        blank_priority_input: String::new(),
        filters_query: filters.query_string(),
        redirect_after_create: nq.redirect.is_some(),
        nav_html,
    })
}

pub async fn project_task_detail_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let item = require_task(item)?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::from([(auth_user.user_id.clone(), "You".to_string())]),
    };
    let parent_link = resolve_parent_link(&repo, &project_id, &item).await?;
    let linked_event = resolve_linked_event(&repo, &project_id, &item).await?;
    let series_link = resolve_series_link(&series, &project_id, &item).await?;
    let depends_on =
        resolve_depends_on_links(&repo, &item_dependencies, &project_id, &item).await?;
    let item_reminders = reminders
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_comments = comments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_attachments = attachments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let view = ProjectTaskDetailView::from_item(
        &item,
        &project_id,
        project.team_id.is_some(),
        &names,
        tz,
        parent_link.clone(),
        linked_event,
        series_link,
        depends_on,
        item_reminders,
        item_comments,
        item_attachments,
        &auth_user.user_id,
        None,
        false,
    )
    .render()?;
    let dialog = ProjectTaskDetailDialog::new(
        &item.id,
        &project_id,
        &item.name,
        item.complete,
        view.clone(),
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskDetailPageTemplate {
        id: item.id,
        project_id,
        name: item.name,
        complete: item.complete,
        view,
        dialog,
        nav_html,
        parent_link,
    })
}

/// Posts a new comment, optionally with a single attached file — root CLAUDE.md's
/// Attachments section: a file always belongs to a comment, so this one multipart form
/// (`body` text field + optional `file` field) is the only way the web UI creates either.
/// `Multipart` must be the last extractor argument (it consumes the request body
/// directly). Re-renders the whole read-only detail card (`#item-{id}-view`)'s comments
/// section, the same target/swap the card's own complete-toggle checkbox already uses
/// (`detail_view.html`), rather than inventing a narrower fragment.
pub async fn create_project_task_comment_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    Extension(attachment_store): Extension<Arc<dyn AttachmentStore>>,
    TzOffset(tz): TzOffset,
    mut multipart: Multipart,
) -> Result<Html<String>, ItemError> {
    let mut body = String::new();
    let mut file = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ItemError::Invalid(format!("invalid upload: {e}")))?
    {
        match field.name() {
            Some("body") => {
                body = field
                    .text()
                    .await
                    .map_err(|e| ItemError::Invalid(format!("invalid upload: {e}")))?;
            }
            Some("file") => {
                // An empty/unselected `<input type="file">` still submits a field with
                // no filename — treat that the same as "no file", not an empty upload.
                if let Some(filename) = field.file_name().map(str::to_string) {
                    if !filename.is_empty() {
                        let content_type = field
                            .content_type()
                            .map(str::to_string)
                            .unwrap_or_else(|| "application/octet-stream".to_string());
                        let bytes = field
                            .bytes()
                            .await
                            .map_err(|e| ItemError::Invalid(format!("invalid upload: {e}")))?
                            .to_vec();
                        if !bytes.is_empty() {
                            file = Some((filename, content_type, bytes));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    comments_service::create_comment_with_attachment(
        &comments,
        &attachments,
        &attachment_store,
        &repo,
        &projects,
        &teams,
        &project_id,
        &item_id,
        &auth_user.user_id,
        &body,
        file,
    )
    .await?;

    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let item = require_task(item)?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::from([(auth_user.user_id.clone(), "You".to_string())]),
    };
    let parent_link = resolve_parent_link(&repo, &project_id, &item).await?;
    let linked_event = resolve_linked_event(&repo, &project_id, &item).await?;
    let series_link = resolve_series_link(&series, &project_id, &item).await?;
    let depends_on =
        resolve_depends_on_links(&repo, &item_dependencies, &project_id, &item).await?;
    let item_reminders = reminders
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_comments = comments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_attachments = attachments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    render(ProjectTaskDetailView::from_item(
        &item,
        &project_id,
        project.team_id.is_some(),
        &names,
        tz,
        parent_link,
        linked_event,
        series_link,
        depends_on,
        item_reminders,
        item_comments,
        item_attachments,
        &auth_user.user_id,
        None,
        true,
    ))
}

/// Shared tail for the three comment-editing routes below (edit-toggle/save/delete) —
/// re-fetches everything `ProjectTaskDetailView::from_item` needs and renders it, exactly
/// like `create_project_task_comment_form`'s own tail, just factored out so those three
/// don't each duplicate ~30 lines of it. `editing_comment_id` is `Some` only for the
/// edit-toggle GET; the save/delete routes always pass `None`, returning to plain view
/// mode.
#[allow(clippy::too_many_arguments)]
async fn render_project_task_comments_view(
    project_id: &str,
    item_id: &str,
    auth_user: &AuthUser,
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
    reminders: &Arc<dyn ReminderRepo>,
    comments: &Arc<dyn CommentRepo>,
    attachments: &Arc<dyn AttachmentRepo>,
    tz: i32,
    editing_comment_id: Option<&str>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(projects, teams, project_id, &auth_user.user_id).await?;
    let item = project_item_service::get_project_item_unchecked(repo, project_id, item_id).await?;
    let item = require_task(item)?;
    let names = match &project.team_id {
        Some(team_id) => names_for(teams, team_id, &auth_user.user_id).await?,
        None => HashMap::from([(auth_user.user_id.clone(), "You".to_string())]),
    };
    let parent_link = resolve_parent_link(repo, project_id, &item).await?;
    let linked_event = resolve_linked_event(repo, project_id, &item).await?;
    let series_link = resolve_series_link(series, project_id, &item).await?;
    let depends_on = resolve_depends_on_links(repo, item_dependencies, project_id, &item).await?;
    let item_reminders = reminders
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_comments = comments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_attachments = attachments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    render(ProjectTaskDetailView::from_item(
        &item,
        project_id,
        project.team_id.is_some(),
        &names,
        tz,
        parent_link,
        linked_event,
        series_link,
        depends_on,
        item_reminders,
        item_comments,
        item_attachments,
        &auth_user.user_id,
        editing_comment_id,
        true,
    ))
}

/// `GET .../comments/{comment_id}/edit` — toggles one comment into edit mode within the
/// comments block (`detail_view.html`'s `c.editing` branch). Author-only
/// (`comments_service::require_comment_author`); a non-author hitting this URL directly
/// gets rejected before anything renders, even though the UI never shows the Edit link to
/// them in the first place (`CommentLine::is_own`).
pub async fn edit_project_task_comment_form(
    Path((project_id, item_id, comment_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    comments_service::require_comment_author(
        &comments,
        &projects,
        &teams,
        &project_id,
        &item_id,
        &comment_id,
        &auth_user.user_id,
    )
    .await?;
    render_project_task_comments_view(
        &project_id,
        &item_id,
        &auth_user,
        &repo,
        &projects,
        &teams,
        &series,
        &item_dependencies,
        &reminders,
        &comments,
        &attachments,
        tz,
        Some(&comment_id),
    )
    .await
}

/// `GET .../comments` — re-renders the comments block in plain (non-editing) view. This is
/// what an in-progress edit's Cancel button hits (`detail_view.html`), rather than the full
/// `GET .../tasks/{id}` page route: that route's response also embeds a second copy of this
/// same comments block inside `ProjectTaskDetailDialog` (its confirm-complete dialog carries
/// a clone of the whole read-only view for its own use), so an `hx-select="#item-{id}-comments"`
/// against it matches two elements and both get swapped in — the comment appearing duplicated
/// on Cancel, even though a page reload shows only one (`list_for_item` was never touched).
/// This route's response has no dialog at all, so the selector only ever matches once.
pub async fn cancel_project_task_comment_edit_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    render_project_task_comments_view(
        &project_id,
        &item_id,
        &auth_user,
        &repo,
        &projects,
        &teams,
        &series,
        &item_dependencies,
        &reminders,
        &comments,
        &attachments,
        tz,
        None,
    )
    .await
}

#[derive(serde::Deserialize)]
pub struct UpdateCommentForm {
    pub body: String,
}

/// `PUT .../comments/{comment_id}` — saves an edit (author-only,
/// `comments_service::update_comment`), then re-renders the comments block back in plain
/// view mode.
pub async fn update_project_task_comment_form(
    Path((project_id, item_id, comment_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<UpdateCommentForm>,
) -> Result<Html<String>, ItemError> {
    comments_service::update_comment(
        &comments,
        &projects,
        &teams,
        &project_id,
        &item_id,
        &comment_id,
        &auth_user.user_id,
        &form.body,
    )
    .await?;
    render_project_task_comments_view(
        &project_id,
        &item_id,
        &auth_user,
        &repo,
        &projects,
        &teams,
        &series,
        &item_dependencies,
        &reminders,
        &comments,
        &attachments,
        tz,
        None,
    )
    .await
}

/// `DELETE .../comments/{comment_id}` — author-only (`comments_service::delete_comment`),
/// then re-renders the comments block, same tail as the save route above.
pub async fn delete_project_task_comment_form(
    Path((project_id, item_id, comment_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    comments_service::delete_comment(
        &comments,
        &projects,
        &teams,
        &project_id,
        &item_id,
        &comment_id,
        &auth_user.user_id,
    )
    .await?;
    render_project_task_comments_view(
        &project_id,
        &item_id,
        &auth_user,
        &repo,
        &projects,
        &teams,
        &series,
        &item_dependencies,
        &reminders,
        &comments,
        &attachments,
        tz,
        None,
    )
    .await
}

/// Streams an attachment's raw bytes back. An image gets `Content-Disposition: inline`
/// so the read-only view's `<img src>` can render it directly (root CLAUDE.md's
/// Attachments section — "show images in the UI"); anything else gets `attachment`, so
/// the browser downloads/saves it instead of trying to display or navigate to it. Either
/// way the view's link to this route carries `hx-boost="false"` so htmx never intercepts
/// it as a boosted page navigation and tries to swap the bytes into `#page`.
pub async fn download_project_task_attachment(
    Path((project_id, item_id, attachment_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    Extension(attachment_store): Extension<Arc<dyn AttachmentStore>>,
) -> Result<Response, ItemError> {
    let (attachment, bytes) = attachments_service::download_attachment(
        &attachments,
        &attachment_store,
        &projects,
        &teams,
        &project_id,
        &item_id,
        &attachment_id,
        &auth_user.user_id,
    )
    .await?;

    let disposition = if attachment.content_type.starts_with("image/") {
        format!("inline; filename=\"{}\"", attachment.filename)
    } else {
        format!("attachment; filename=\"{}\"", attachment.filename)
    };
    Ok((
        [
            (http::header::CONTENT_TYPE, attachment.content_type),
            (http::header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}

/// Mirrors `create_project_task_attachment_form`'s tail exactly (same full-view
/// re-render, same `hx-select="#item-{id}-attachments"` client-side extraction) after
/// deleting instead of uploading.
pub async fn delete_project_task_attachment_form(
    Path((project_id, item_id, attachment_id)): Path<(String, String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    Extension(attachment_store): Extension<Arc<dyn AttachmentStore>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    attachments_service::delete_attachment(
        &attachments,
        &attachment_store,
        &projects,
        &teams,
        &project_id,
        &item_id,
        &attachment_id,
        &auth_user.user_id,
    )
    .await?;

    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let item = require_task(item)?;
    let names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::from([(auth_user.user_id.clone(), "You".to_string())]),
    };
    let parent_link = resolve_parent_link(&repo, &project_id, &item).await?;
    let linked_event = resolve_linked_event(&repo, &project_id, &item).await?;
    let series_link = resolve_series_link(&series, &project_id, &item).await?;
    let depends_on =
        resolve_depends_on_links(&repo, &item_dependencies, &project_id, &item).await?;
    let item_reminders = reminders
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_comments = comments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    let item_attachments = attachments
        .list_for_item(&item.id)
        .await
        .map_err(ItemError::from)?;
    render(ProjectTaskDetailView::from_item(
        &item,
        &project_id,
        project.team_id.is_some(),
        &names,
        tz,
        parent_link,
        linked_event,
        series_link,
        depends_on,
        item_reminders,
        item_comments,
        item_attachments,
        &auth_user.user_id,
        None,
        true,
    ))
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditItemQuery {
    /// Set only by the item's own read-only detail page's Edit link (`detail_page.html`) — see
    /// `ProjectTaskDetailFields.via_full_page`'s doc comment for why the edit form's save
    /// target differs in that context. Absent (the default) for a list row's "⋮ Edit"/detail
    /// dialog's Edit button, which keeps targeting the row in place.
    redirect: Option<String>,
}

pub async fn project_task_edit_page(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Query(edit_q): Query<EditItemQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let item =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let item = require_task(item)?;
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(&teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    let (depends_on_options, depends_on_item_ids) =
        depends_on_picker_data(&repo, &item_dependencies, &project_id, &item).await?;
    let anchor_date = resolve_task_anchor_date(&repo, &project, &auth_user.user_id, &item).await?;
    let fields = ProjectTaskDetailFields::from_item(
        &item,
        &project_id,
        project.team_id.is_some(),
        assignee_options,
        is_team_admin,
        tz,
        false,
        edit_q.redirect.is_some(),
        depends_on_options,
        depends_on_item_ids,
        anchor_date,
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskEditPageTemplate {
        name: item.name,
        fields,
        nav_html,
    })
}

/// Stage C of `docs/unify-virtual-materialized-occurrences-plan.md` — the Task-flavored half
/// of the deferred-materialization detail page, called by
/// `project_item_series::handlers::occurrence_detail_page` once it's dispatched on
/// `series.item_type == Task` and confirmed the occurrence isn't already materialized (that
/// case redirects to the real `/tasks/{id}` page instead of calling this — see that
/// function's doc comment). Renders read-only, with no side effect: never calls
/// `get_or_materialize_occurrence`.
pub(crate) async fn render_series_occurrence_detail_page(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    item_series: &Arc<dyn ItemSeriesRepo>,
    auth_user: &AuthUser,
    project_id: &str,
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    is_skipped: bool,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(projects, teams, project_id, &auth_user.user_id).await?;
    let names = match &project.team_id {
        Some(team_id) => names_for(teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let is_current = current_occurrence_is(series, occurrence_date, tz)?;
    // Stage 4 of docs/assignment-rotation-plan.md: this occurrence's own resolved
    // assignee (fixed, or this calendar position's rotation member) — not the series'
    // raw `assigned_to_user_id`, which is `None` for a rotating series.
    let resolved_assignee_id =
        item_series_service::resolve_occurrence_assignee(item_series, series, occurrence_date, tz)
            .await?;
    let view = ProjectTaskSeriesOccurrenceView::from_series(
        series,
        occurrence_date,
        project_id,
        project.team_id.is_some(),
        &names,
        resolved_assignee_id,
        is_skipped,
        is_current,
        tz,
    )
    .render()?;
    let occurrence_ts = occurrence_date.timestamp();
    let dialog = ProjectTaskSeriesOccurrenceDetailDialog::new(
        project_id,
        &series.id,
        occurrence_ts,
        &series.name,
        is_skipped,
        view,
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        projects,
        &auth_user.user_id,
        active_context(project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskSeriesOccurrenceDetailPageTemplate {
        name: series.name.clone(),
        dialog,
        nav_html,
    })
}

/// The Task-flavored half of the deferred-materialization edit page — see
/// `render_series_occurrence_detail_page`'s doc comment for the dispatch this is called from.
/// Also no side effect: prefilled from `series`/`occurrence_date` directly, not a real `Item`.
pub(crate) async fn render_series_occurrence_edit_page(
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    item_series: &Arc<dyn ItemSeriesRepo>,
    auth_user: &AuthUser,
    project_id: &str,
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz: i32,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(projects, teams, project_id, &auth_user.user_id).await?;
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(projects, teams, project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    // Stage 4 of docs/assignment-rotation-plan.md — prefill the select with this
    // occurrence's actually-resolved assignee (fixed, or this calendar position's
    // rotation member), not the series' raw `assigned_to_user_id`. Without this, an
    // unmodified Save on a rotating occurrence's edit form would silently overwrite the
    // just-materialized correct assignee with "Unassigned" (`overlay_str` always applies
    // whatever the select submits — see `update_params_from_form`).
    let resolved_assignee_id =
        item_series_service::resolve_occurrence_assignee(item_series, series, occurrence_date, tz)
            .await?;
    let fields = ProjectTaskSeriesOccurrenceFields::from_series(
        series,
        occurrence_date,
        project_id,
        project.team_id.is_some(),
        assignee_options,
        resolved_assignee_id,
        is_team_admin,
        tz,
    )
    .render()?;
    let nav_html = nav::build_nav_html(
        projects,
        &auth_user.user_id,
        active_context(project_id),
        SidebarSection::Tasks,
    )
    .await?;
    render(ProjectTaskSeriesOccurrenceEditPageTemplate {
        name: series.name.clone(),
        fields,
        nav_html,
    })
}

/// Whether `occurrence_date` is `series`'s current occurrence — same check
/// `service::item_series::require_current_occurrence` makes internally, duplicated here
/// (rather than exposed from that module) since it's purely a display concern on this page,
/// not a mutation gate. `Event`-typed series have no cursor/current concept, so always
/// `false` there — this is only ever called from the Task-flavored render path above anyway.
fn current_occurrence_is(
    series: &crate::domain::item_series::ItemSeries,
    occurrence_date: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Result<bool, ItemError> {
    let rule = crate::domain::recurrence::parse(&series.recurrence).map_err(ItemError::Invalid)?;
    Ok(
        item_series_service::current_occurrence_date(series, &rule, tz_offset_minutes)
            == occurrence_date,
    )
}

/// Stage C — materializes the occurrence (if not already) and applies the edit in one step,
/// the shared PUT target for both the checkbox on
/// `render_series_occurrence_detail_page`'s view (`hx-vals='{"complete": "true"}'`) and the
/// full edit form on `render_series_occurrence_edit_page` (same `ProjectTaskForm` shape either
/// way — a checkbox toggle is just a form submission with only `complete` set). Always
/// redirects to the now-real item's canonical `/tasks/{id}` page — there's no meaningful
/// in-place fragment to return to once the surrounding page's whole premise (a still-virtual
/// occurrence) has changed.
pub async fn update_project_task_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskForm>,
) -> Result<Response, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Task {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = item_series_service::get_or_materialize_occurrence(
        &repo,
        &projects,
        &teams,
        &item_series,
        &reminders,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
        tz,
    )
    .await?;
    let params = update_params_from_form(&project_id, &item.id, &item, &form, tz);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &item_series,
        &reminders,
        &item_dependencies,
        &auth_user.user_id,
        params,
    )
    .await?;
    Ok(hx_redirect(project_task_url(&project_id, &item.id)))
}

/// See `project_item_series::handlers::redirect_to_current_page`'s identical rationale —
/// duplicated per that module's own precedent rather than shared.
fn redirect_to_current_page(headers: &HeaderMap, project_id: &str) -> Response {
    let location = headers
        .get("hx-current-url")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| project_tasks_list_url(project_id));
    hx_redirect(location)
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OccurrenceRowActionQuery {
    /// `"tasks-list"`, `"all-tasks"` — set only by `ProjectTaskVirtualRow`/
    /// `AllProjectsTaskVirtualRow`'s `from_occurrence` when rendering for that screen's own flat
    /// list (never the calendar day panel). Every other caller of this same route falls back to
    /// the pre-existing `redirect_to_current_page` behavior below.
    view: Option<String>,
    show_complete: Option<String>,
    /// Stage 2 of `docs/list-filtering-plan.md`: `ProjectTaskVirtualRow`/`AllProjectsTaskVirtualRow`'s
    /// own `from_occurrence` bakes the full active filter set (not just `showComplete`) into this
    /// route's URL, whichever `view` is set — so `view=tasks-list`'s/`view=all-tasks`'s rebuild of
    /// `#items-list` applies the same filters the surrounding page load did.
    assigned_to: Option<String>,
    due_date: Option<String>,
    schedule: Option<String>,
    recurring: Option<String>,
    priority: Option<String>,
    /// Only meaningful alongside `view=all-tasks` — the cross-project-only `project` filter
    /// dimension `all_projects_tasks::AllProjectsTasksQuery` carries, absent from `ListFilters`
    /// itself (see that type's own doc comment). Ignored by every other `view`.
    project: Option<String>,
}

/// The row-checkbox counterpart to Skip/Unskip (`project_item_series::handlers`) — completes a
/// Task-series occurrence directly from a list/calendar row's checkbox, whether it's still
/// virtual or already materialized doesn't matter to the caller: materializes it first if
/// needed (`get_or_materialize_occurrence`, a no-op if already materialized), then completes it
/// via the exact same `update_project_item` path a real item's own checkbox already uses — so
/// cursor validation (`item_series::require_current_occurrence`), points, and activity logging
/// all apply identically. Task-typed series only — `Item::validate` rejects `complete: true`
/// for Events, so this route is never wired onto an Event occurrence's row.
///
/// See the archived "extend confirm-then-fade to virtual occurrences" entry (2026-08-21): when
/// `view=tasks-list` (baked into the URL by `ProjectTaskVirtualRow::from_occurrence`), this
/// rebuilds the whole `#items-list` in place via `list_task_rows_for_project` instead of
/// `HX-Redirect`-ing the whole page, giving the completing row its own `Row`-style
/// confirm-then-fade-away treatment (`just_completed_item_id`) — needed because completing a
/// series' current occurrence can advance its cursor to a new current occurrence, which a
/// single-row swap could never surface. Any other caller (nothing today, but this route
/// predates the `view` param and its own doc history — see the calendar day panel's
/// `in_list_view: false` rows) keeps the original whole-page redirect.
pub async fn complete_project_item_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<OccurrenceRowActionQuery>,
    headers: HeaderMap,
) -> Result<Response, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Task {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = item_series_service::get_or_materialize_occurrence(
        &repo,
        &projects,
        &teams,
        &item_series,
        &reminders,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
        tz,
    )
    .await?;
    let form = ProjectTaskForm {
        complete: Some("true".to_string()),
        ..Default::default()
    };
    let params = update_params_from_form(&project_id, &item.id, &item, &form, tz);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &item_series,
        &reminders,
        &item_dependencies,
        &auth_user.user_id,
        params,
    )
    .await?;

    if q.view.as_deref() == Some("tasks-list") {
        let project =
            project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id)
                .await?;
        let filters = list_filters_from_parts(
            &q.show_complete,
            &q.assigned_to,
            &q.due_date,
            &q.schedule,
            &q.recurring,
            &q.priority,
        );
        let rows = super::list_task_rows_for_project(
            &repo,
            &teams,
            &users,
            &item_series,
            &project_id,
            project.team_id.as_deref(),
            &auth_user.user_id,
            &filters,
            tz,
            Some(item.id.as_str()),
            &item_dependencies,
        )
        .await?;
        return Ok(Html(super::items_list_inner_html(&rows)).into_response());
    }
    if q.view.as_deref() == Some("all-tasks") {
        let filters = list_filters_from_parts(
            &q.show_complete,
            &q.assigned_to,
            &q.due_date,
            &q.schedule,
            &q.recurring,
            &q.priority,
        );
        let rows = crate::web_ui::all_projects_tasks::list_all_projects_task_rows(
            &repo,
            &projects,
            &users,
            &teams,
            &item_series,
            &auth_user.user_id,
            &filters,
            q.project.as_deref(),
            tz,
            Some(item.id.as_str()),
        )
        .await?;
        return Ok(Html(super::items_list_inner_html(&rows)).into_response());
    }
    Ok(redirect_to_current_page(&headers, &project_id))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskSeriesOccurrenceChildForm {
    name: String,
    due_offset_days: Option<String>,
}

/// `GET` counterpart to `create_project_task_series_occurrence_child_form` below — opens the
/// "Add sub-item" dialog for a still-virtual/skipped occurrence, prefilled from nothing but the
/// series' name (no side effect: doesn't materialize just to render a dialog whose own POST
/// already handles that). Added alongside `docs/issues_and_features.md`'s "all row actions
/// should be available and will auto-materialize a virtual row if taken" item — the POST route
/// this opens into existed since Stage C of `docs/unify-virtual-materialized-occurrences-plan.md`
/// but had no UI entry point until now.
pub async fn get_project_task_series_occurrence_add_child_dialog(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Html<String>, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Task {
        return Err(ItemError::NotFound);
    }
    render(ProjectTaskSeriesOccurrenceAddChildDialog {
        parent_name: series.name.clone(),
        post_create_url: format!(
            "/web/projects/{project_id}/series/{series_id}/occurrences/{occurrence_ts}/task-children"
        ),
    })
}

/// Stage C — "adding a sub-item" to a still-virtual occurrence: materializes it first, then
/// creates the child underneath the resulting real item, then redirects to that item's
/// canonical `/tasks/{id}` page (mirrors `update_project_task_series_occurrence_form`'s own
/// redirect rationale — there's nothing left to render in place of the virtual page).
pub async fn create_project_task_series_occurrence_child_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskSeriesOccurrenceChildForm>,
) -> Result<Response, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id || series.item_type != ItemKind::Task {
        return Err(ItemError::NotFound);
    }
    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;
    let item = item_series_service::get_or_materialize_occurrence(
        &repo,
        &projects,
        &teams,
        &item_series,
        &reminders,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
        tz,
    )
    .await?;
    let params = crate::service::project_items::CreateProjectItemParams {
        project_id: project_id.clone(),
        name: form.name,
        parent_item_id: Some(item.id.clone()),
        item_type: Some(ItemKind::Task),
        due_offset_days: form
            .due_offset_days
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(parse_days_before_due),
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    };
    project_item_service::create_project_item(
        &repo,
        &projects,
        &teams,
        &reminders,
        &auth_user.user_id,
        params,
    )
    .await?;
    Ok(hx_redirect(project_task_url(&project_id, &item.id)))
}

/// Renders a parent item's children as `Row`s — see `tasks::render_children_fragment`'s
/// identical rationale, project-scoped. Callers are responsible for their own membership gate
/// before calling this (see `project_task_children_fragment`).
///
/// Delegates the per-row "Blocked by ..." badge and `children_html` in-place-expand treatment
/// (see `Row::children_html`'s doc comment — otherwise a grandchild-bearing sub-item's
/// name-click would fall through to `detail_via_dialog` instead of toggling, diverging from how
/// the same item behaves when its row is rendered elsewhere) to `super::render_sibling_rows`,
/// shared with `render_source_event_fragment` below and with this same panel's own
/// create-refresh (`super::render_scope_fragment`) so the three can't drift out of sync again.
pub(crate) async fn render_children_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    team_id: Option<&str>,
    parent_item_id: &str,
    requester_user_id: &str,
    tz: i32,
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
) -> Result<Html<String>, ItemError> {
    let children = project_item_service::list_project_items_unchecked(
        repo,
        project_id,
        Some(parent_item_id.to_string()),
    )
    .await?;
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let visible: Vec<&Item> = children.iter().collect();
    let rows = super::render_sibling_rows(
        repo,
        &visible,
        project_id,
        &names,
        tz,
        team_id.is_some(),
        true,
        item_dependencies,
    )
    .await?;
    render(ProjectTaskRowsFragmentTemplate {
        rows,
        empty_message: "No sub-items yet.".to_string(),
    })
}

/// Renders every task that references `event_id` via `sourceEventId` as `Row`s, scoped to
/// `project_id` — the project-scoped counterpart of `tasks::render_source_event_fragment`/
/// `team_tasks::render_source_event_fragment`, called by `project_events`'s "Linked tasks"
/// section (Events have no children of their own — see `project_events::require_event`'s
/// doc comment — so there's no `project_events`-owned Task-row renderer to put this in).
///
/// Same `super::render_sibling_rows` delegation as `render_children_fragment` above — a linked
/// task can still have its own sub-items (`add_child_url` doesn't require `source_event_id` to
/// be unset), so its row needs the same `children_html` inlining to toggle rather than dialog.
/// A linked task can also depend on another linked task under the same event, since
/// `service::item_dependencies` scopes "sibling" to same `parent_item_id` and every top-level
/// linked task here shares `None` — though a dependency pointing at a top-level task that isn't
/// itself linked to this event won't resolve to a name here, `visible` being only this event's
/// own linked-tasks subset rather than the full top-level sibling group; same limitation the
/// flat Tasks list already has for any dependency a `ListFilters` hides from its own `visible`.
pub(crate) async fn render_source_event_fragment(
    repo: &Arc<dyn ItemRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    team_id: Option<&str>,
    event_id: &str,
    requester_user_id: &str,
    tz: i32,
    item_dependencies: &Arc<dyn ItemDependencyRepo>,
) -> Result<Html<String>, ItemError> {
    let tasks =
        project_item_service::list_project_event_children_unchecked(repo, project_id, event_id)
            .await?;
    let names = match team_id {
        Some(team_id) => names_for(teams, team_id, requester_user_id).await?,
        None => HashMap::new(),
    };
    let visible: Vec<&Item> = tasks.iter().collect();
    let rows = super::render_sibling_rows(
        repo,
        &visible,
        project_id,
        &names,
        tz,
        team_id.is_some(),
        true,
        item_dependencies,
    )
    .await?;
    render(ProjectTaskRowsFragmentTemplate {
        rows,
        empty_message: "No linked tasks yet.".to_string(),
    })
}

pub async fn project_task_children_fragment(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    // Ownership gate: confirm the parent actually belongs to this project before listing its
    // children (mirrors tasks.rs's equivalent).
    project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    render_children_fragment(
        &repo,
        &teams,
        &project_id,
        project.team_id.as_deref(),
        &item_id,
        &auth_user.user_id,
        tz,
        &item_dependencies,
    )
    .await
}

/// Redirect back to the project's tasks list (via the `hx-redirect` header) after a create
/// from the standalone `/projects/:project_id/tasks/new` page. Mirrors
/// `tasks::redirect_to_tasks`/`team_tasks::redirect_to_team_tasks`. `filters_query` is the
/// opaque `ListFilters::query_string()` fragment the calling form round-tripped (see
/// `ProjectTaskForm::filters_query`) — appended as-is, empty means every filter was already at
/// its default.
fn redirect_to_project_tasks(project_id: &str, filters_query: &str) -> Response {
    let location = if filters_query.is_empty() {
        project_tasks_list_url(project_id)
    } else {
        format!("/web/projects/{project_id}/tasks?{filters_query}")
    };
    hx_redirect(location)
}

pub async fn create_project_task_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<ProjectTaskForm>,
) -> Result<Response, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let show_complete = form.show_complete.is_some();
    let redirect = form.redirect.is_some();
    let filters_query = form.filters_query.clone().unwrap_or_default();
    let return_to = form.return_to.clone();
    let params = create_params_from_form(&project_id, &form, tz);
    let parent_item_id = params.parent_item_id.clone();
    project_item_service::create_project_item(
        &repo,
        &projects,
        &teams,
        &reminders,
        &auth_user.user_id,
        params,
    )
    .await?;
    if redirect {
        return Ok(
            match super::sanitize_return_to(return_to.as_deref(), &project_id) {
                Some(url) => hx_redirect(url),
                None => redirect_to_project_tasks(&project_id, &filters_query),
            },
        );
    }
    Ok(render_scope_fragment(
        &repo,
        &teams,
        &project_id,
        project.team_id.as_deref(),
        &auth_user.user_id,
        parent_item_id.as_deref(),
        show_complete,
        tz,
        &item_dependencies,
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
    /// See `ProjectTaskForm::filters_query`'s identical rationale — an opaque, pre-encoded
    /// `ListFilters::query_string()` fragment, not individual `ListFilterQuery` fields.
    filters_query: Option<String>,
    redirect: Option<String>,
    /// See `ProjectTaskForm::return_to`'s identical rationale — `AddChildDialog`'s "Add
    /// multiple at once" batch form carries the same hidden field.
    return_to: Option<String>,
}

pub async fn create_project_tasks_batch(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchForm>,
) -> Result<Response, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
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
            item_type: Some(ItemKind::Task),
            timezone_offset_minutes: Some(tz),
            ..Default::default()
        };
        project_item_service::create_project_item(
            &repo,
            &projects,
            &teams,
            &reminders,
            &auth_user.user_id,
            params,
        )
        .await?;
    }
    if form.redirect.is_some() {
        return Ok(
            match super::sanitize_return_to(form.return_to.as_deref(), &project_id) {
                Some(url) => hx_redirect(url),
                None => redirect_to_project_tasks(
                    &project_id,
                    form.filters_query.as_deref().unwrap_or(""),
                ),
            },
        );
    }
    Ok(render_scope_fragment(
        &repo,
        &teams,
        &project_id,
        project.team_id.as_deref(),
        &auth_user.user_id,
        parent_item_id.as_deref(),
        form.show_complete.is_some(),
        tz,
        &item_dependencies,
    )
    .await?
    .into_response())
}

// ---- multi-select batch actions -------------------------------------------------------
//
// docs/issues_and_features.md's "Multi-select" item, project_tasks' list page only for this
// first pass. Selection itself is pure client-side state (base.html's selectedItemIds Set) —
// these routes only see the ids a batch dialog's hidden `itemIds` field carried, comma-joined
// (same axum-0.6-Form-can't-deserialize-repeated-keys workaround as
// `ProjectTaskForm::depends_on_item_ids`). Each handler applies its one field's worth of
// change to every selected item via the existing `update_project_item`, so admin/points
// gating, the completed-item edit lock, and offset-driven due-date recompute all apply exactly
// as they do for a single-item edit.

fn parse_item_ids(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Loads and membership-checks every selected item, rejecting up front on an empty selection or
/// any id that doesn't resolve to a Task in this project — the same checked
/// `project_items::get_project_item` + `require_task` a single-item route already uses, looped.
async fn load_batch_items(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    project_id: &str,
    requester_user_id: &str,
    item_ids: &[String],
) -> Result<Vec<Item>, ItemError> {
    if item_ids.is_empty() {
        return Err(ItemError::Invalid("No items selected.".to_string()));
    }
    let mut items = Vec::with_capacity(item_ids.len());
    for id in item_ids {
        let item = project_item_service::get_project_item(
            repo,
            projects,
            teams,
            project_id,
            requester_user_id,
            id,
        )
        .await?;
        items.push(require_task(item)?);
    }
    Ok(items)
}

/// "Set due/schedule dates" only applies when every selected item is top-level — a sub-item's
/// own due date is offset-driven, not independently set (see root CLAUDE.md's Recurrence
/// section on `due_offset_days`).
fn require_all_top_level(items: &[Item]) -> Result<(), ItemError> {
    if items.iter().any(|i| i.parent_item_id.is_some()) {
        return Err(ItemError::Invalid(
            "Set due/schedule dates only applies when every selected task is top-level."
                .to_string(),
        ));
    }
    Ok(())
}

/// "Set offset" only applies to sub-items that all share the same top-level parent.
fn require_same_parent_subitems(items: &[Item]) -> Result<(), ItemError> {
    fn invalid() -> ItemError {
        ItemError::Invalid(
            "Set offset only applies to sub-items of the same top-level task.".to_string(),
        )
    }
    let first = items
        .first()
        .and_then(|i| i.parent_item_id.clone())
        .ok_or_else(invalid)?;
    if items
        .iter()
        .any(|i| i.parent_item_id.as_deref() != Some(first.as_str()))
    {
        return Err(invalid());
    }
    Ok(())
}

/// Round-trips every `UpdateProjectItemParams` field from `item` unchanged — the batch-actions
/// counterpart of `reparent_params` below, used as a base each batch handler overlays just its
/// own field(s) onto.
fn identity_params(project_id: &str, item: &Item, tz: i32) -> UpdateProjectItemParams {
    UpdateProjectItemParams {
        project_id: project_id.to_string(),
        item_id: item.id.clone(),
        name: item.name.clone(),
        description: item.description.clone(),
        due_date: item.due_date(),
        scheduled_date: item.scheduled_date(),
        scheduled_end_date: item.scheduled_end_date(),
        complete: item.complete,
        has_due_time: Some(item.has_due_time()),
        has_scheduled_time: Some(item.has_scheduled_time()),
        has_end_time: Some(item.has_end_time()),
        parent_item_id: item.parent_item_id.clone(),
        item_type: Some(item.kind()),
        event_type: item.event_type(),
        due_offset_days: item.due_offset_days(),
        assigned_to_user_id: item.assigned_to_user_id(),
        source_event_id: item.source_event_id(),
        timezone_offset_minutes: Some(tz),
        points: item.points(),
        priority: item.priority(),
        depends_on_item_ids: None,
    }
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchPriorityForm {
    item_ids: String,
    /// Blank clears priority — this is the one dedicated field this dialog has, so unlike the
    /// dates dialog's fields below there's no "leave unchanged" state to preserve.
    priority: Option<String>,
    filters_query: Option<String>,
}

pub async fn batch_set_priority_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchPriorityForm>,
) -> Result<Response, ItemError> {
    let item_ids = parse_item_ids(&form.item_ids);
    let items = load_batch_items(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_ids,
    )
    .await?;
    let new_priority = non_empty(&form.priority).and_then(|s| s.parse::<i32>().ok());
    for item in &items {
        let mut params = identity_params(&project_id, item, tz);
        params.priority = new_priority;
        project_item_service::update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            &auth_user.user_id,
            params,
        )
        .await?;
    }
    Ok(redirect_to_project_tasks(
        &project_id,
        form.filters_query.as_deref().unwrap_or(""),
    ))
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchDatesForm {
    item_ids: String,
    /// A blank date here (and in the two pairs below) means "leave this item's own value
    /// unchanged" — unlike a single-item edit form's fields, where blank explicitly clears.
    /// This one dialog bundles due date + scheduled start + scheduled end, so a user setting
    /// just one of the three shouldn't wipe the others.
    due_date: Option<String>,
    due_time: Option<String>,
    scheduled_date: Option<String>,
    scheduled_time: Option<String>,
    scheduled_end_date: Option<String>,
    scheduled_end_time: Option<String>,
    filters_query: Option<String>,
}

pub async fn batch_set_dates_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchDatesForm>,
) -> Result<Response, ItemError> {
    let item_ids = parse_item_ids(&form.item_ids);
    let items = load_batch_items(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_ids,
    )
    .await?;
    require_all_top_level(&items)?;
    let due_date_field = non_empty(&form.due_date);
    let scheduled_date_field = non_empty(&form.scheduled_date);
    let scheduled_end_date_field = non_empty(&form.scheduled_end_date);
    for item in &items {
        let mut params = identity_params(&project_id, item, tz);
        if let Some(date) = due_date_field.clone() {
            params.due_date = overlay_due_date(&Some(date), &form.due_time, tz, item.due_date());
            params.has_due_time = Some(overlay_has_due_time(&form.due_time, item.has_due_time()));
        }
        if let Some(date) = scheduled_date_field.clone() {
            params.scheduled_date = overlay_scheduled_date(
                &Some(date),
                &form.scheduled_time,
                tz,
                item.scheduled_date(),
            );
            params.has_scheduled_time = Some(overlay_has_due_time(
                &form.scheduled_time,
                item.has_scheduled_time(),
            ));
        }
        if let Some(date) = scheduled_end_date_field.clone() {
            params.scheduled_end_date = overlay_scheduled_end_date(
                &Some(date),
                &form.scheduled_end_time,
                tz,
                item.scheduled_end_date(),
            );
            params.has_end_time = Some(overlay_has_due_time(
                &form.scheduled_end_time,
                item.has_end_time(),
            ));
        }
        project_item_service::update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            &auth_user.user_id,
            params,
        )
        .await?;
    }
    Ok(redirect_to_project_tasks(
        &project_id,
        form.filters_query.as_deref().unwrap_or(""),
    ))
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchOffsetForm {
    item_ids: String,
    /// Blank clears the offset — the one dedicated field this dialog has, same convention as
    /// `BatchPriorityForm::priority`.
    due_offset_days: Option<String>,
    filters_query: Option<String>,
}

pub async fn batch_set_offset_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchOffsetForm>,
) -> Result<Response, ItemError> {
    let item_ids = parse_item_ids(&form.item_ids);
    let items = load_batch_items(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_ids,
    )
    .await?;
    require_same_parent_subitems(&items)?;
    let new_offset = non_empty(&form.due_offset_days).and_then(|s| parse_days_before_due(&s));
    for item in &items {
        let mut params = identity_params(&project_id, item, tz);
        params.due_offset_days = new_offset;
        project_item_service::update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            &auth_user.user_id,
            params,
        )
        .await?;
    }
    Ok(redirect_to_project_tasks(
        &project_id,
        form.filters_query.as_deref().unwrap_or(""),
    ))
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BatchAssigneeForm {
    item_ids: String,
    /// Blank unassigns — the one dedicated field this dialog has, same convention as
    /// `BatchPriorityForm::priority`. Non-admin submissions are silently preserved unchanged by
    /// `update_project_item`'s own admin gate (see root CLAUDE.md's Points section), same as a
    /// single-item Assign dialog.
    assigned_to_user_id: Option<String>,
    filters_query: Option<String>,
}

pub async fn batch_set_assignee_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<BatchAssigneeForm>,
) -> Result<Response, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    if project.team_id.is_none() {
        return Err(ItemError::Invalid(
            "Set assignee only applies to team-backed projects.".to_string(),
        ));
    }
    let item_ids = parse_item_ids(&form.item_ids);
    let items = load_batch_items(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_ids,
    )
    .await?;
    let new_assignee = non_empty(&form.assigned_to_user_id);
    for item in &items {
        let mut params = identity_params(&project_id, item, tz);
        params.assigned_to_user_id = new_assignee.clone();
        project_item_service::update_project_item(
            &repo,
            &projects,
            &teams,
            &activity_log,
            &series,
            &reminders,
            &item_dependencies,
            &auth_user.user_id,
            params,
        )
        .await?;
    }
    Ok(redirect_to_project_tasks(
        &project_id,
        form.filters_query.as_deref().unwrap_or(""),
    ))
}

pub async fn update_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Extension(comments): Extension<Arc<dyn CommentRepo>>,
    Extension(attachments): Extension<Arc<dyn AttachmentRepo>>,
    TzOffset(tz): TzOffset,
    Query(view_q): Query<super::RowViewQuery>,
    Form(form): Form<ProjectTaskForm>,
) -> Result<Response, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let current =
        project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await?;
    let current = require_task(current)?;
    let close = form.redirect.is_some();
    let row_view = super::normalize_row_view(view_q);
    let params = update_params_from_form(&project_id, &item_id, &current, &form, tz);
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &reminders,
        &item_dependencies,
        &auth_user.user_id,
        params,
    )
    .await?;

    match project_item_service::get_project_item_unchecked(&repo, &project_id, &item_id).await {
        Ok(updated) if close => {
            let names = match &project.team_id {
                Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
                None => HashMap::from([(auth_user.user_id.clone(), "You".to_string())]),
            };
            let parent_link = resolve_parent_link(&repo, &project_id, &updated).await?;
            let linked_event = resolve_linked_event(&repo, &project_id, &updated).await?;
            let series_link = resolve_series_link(&series, &project_id, &updated).await?;
            let depends_on =
                resolve_depends_on_links(&repo, &item_dependencies, &project_id, &updated).await?;
            let item_reminders = reminders
                .list_for_item(&updated.id)
                .await
                .map_err(ItemError::from)?;
            let item_comments = comments
                .list_for_item(&updated.id)
                .await
                .map_err(ItemError::from)?;
            let item_attachments = attachments
                .list_for_item(&updated.id)
                .await
                .map_err(ItemError::from)?;
            let view = ProjectTaskDetailView::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                &names,
                tz,
                parent_link.clone(),
                linked_event,
                series_link,
                depends_on,
                item_reminders,
                item_comments,
                item_attachments,
                &auth_user.user_id,
                None,
                false,
            )
            .render()?;
            let dialog = ProjectTaskDetailDialog::new(
                &updated.id,
                &project_id,
                &updated.name,
                updated.complete,
                view.clone(),
            )
            .render()?;
            let nav_html = nav::build_nav_html(
                &projects,
                &auth_user.user_id,
                active_context(&project_id),
                SidebarSection::Tasks,
            )
            .await?;
            Ok(render(ProjectTaskDetailPageTemplate {
                id: updated.id.clone(),
                project_id: project_id.clone(),
                name: updated.name.clone(),
                complete: updated.complete,
                view,
                dialog,
                nav_html,
                parent_link,
            })?
            .into_response())
        }
        Ok(updated) => {
            let names = match &project.team_id {
                Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
                None => HashMap::from([(auth_user.user_id.clone(), "You".to_string())]),
            };
            let siblings =
                sibling_group(&repo, &project_id, updated.parent_item_id.as_deref()).await?;
            let siblings_ref: Vec<&Item> = siblings.iter().collect();
            let skip_url =
                item_series_service::skip_url_for_item(&series, &updated, &project_id).await?;
            // Confirmation/auto-dismiss only apply to the completing transition (not
            // un-completing, and not a plain field edit that leaves `complete` unchanged) —
            // see Row's doc comments. `show_complete` here is whatever the checkbox's own
            // `hx-vals` last sent (baked in when this row was originally rendered by a list
            // load, per row.html) — the only way the server can know what the requester's
            // current "Show completed" toggle is set to.
            let show_complete = form.show_complete.is_some();
            let just_completed = !current.complete && updated.complete;
            let confirmation = just_completed.then(|| "Completed".to_string());
            let dismiss_after_ms = (just_completed && !show_complete).then_some(1800u32);
            let parent_link = resolve_parent_link(&repo, &project_id, &updated).await?;
            // Reschedule/Assign saved from a calendar row (`view` set) re-render via that
            // screen's own `calendar_row` overlay (type badge/parent name/project name, plus
            // its calendar-scoped `complete_url`) instead of the plain `ProjectTaskRow` shape —
            // see `RowViewQuery`'s doc comment. A plain edit/reschedule/assign never shifts a
            // series cursor, so a single-row swap (not a whole-list rebuild, unlike
            // `complete_project_item_series_occurrence_form`) is always correct here.
            let parent_name = parent_link.as_ref().map(|(name, _)| name.clone());
            // Mirrors `project_calendar`/`main_calendar`'s own `children_html_for` — a
            // Reschedule/Assign save never changes an item's children, so this just re-derives
            // the same in-place-expansion subtree those screens' own row builders would.
            let children_html = if updated.has_children {
                let descendants = super::render_expandable_children(
                    &repo,
                    &updated.id,
                    &project_id,
                    &names,
                    show_complete,
                    tz,
                    &HashMap::new(),
                    project.team_id.is_some(),
                    1,
                    Some(&item_dependencies),
                )
                .await?;
                // has_children counts filtered-out children too — see
                // render_expandable_children's own call sites for the same guard.
                (!descendants.is_empty()).then_some(descendants)
            } else {
                None
            };
            let row = match row_view.as_deref() {
                Some("project-calendar") => crate::web_ui::project_calendar::calendar_row(
                    &updated,
                    parent_name,
                    &project_id,
                    &names,
                    project.team_id.is_some(),
                    tz,
                    skip_url,
                    show_complete,
                    confirmation,
                    dismiss_after_ms,
                    children_html,
                )?,
                Some("main-calendar") => crate::web_ui::main_calendar::calendar_row(
                    &updated,
                    parent_name,
                    &project_id,
                    &project.name,
                    &names,
                    project.team_id.is_some(),
                    tz,
                    skip_url,
                    confirmation,
                    dismiss_after_ms,
                    children_html,
                )?,
                Some("all-tasks") => crate::web_ui::all_projects_tasks::all_projects_task_row(
                    &updated,
                    &project_id,
                    &project.name,
                    &names,
                    project.team_id.is_some(),
                    tz,
                    skip_url,
                    show_complete,
                    confirmation,
                    dismiss_after_ms,
                    children_html,
                )?,
                _ => {
                    let mut row = ProjectTaskRow::from_item(
                        &updated,
                        &project_id,
                        &names,
                        &siblings_ref,
                        tz,
                        skip_url,
                        project.team_id.is_some(),
                        show_complete,
                        confirmation,
                        dismiss_after_ms,
                        // This single-row rebuild (after an edit/checkbox toggle) has no
                        // batched occurrence-state query on hand the way the full list render
                        // does — see `Row::series_current`'s doc comment on this gap.
                        None,
                    );
                    // Without this, a row with children swapped back in after an edit falls
                    // through to row.html's dialog/navigation name-click branch instead of
                    // toggleChildren() — see `Row::children_html`'s doc comment.
                    row.children_html = children_html;
                    let dep_map = item_dependencies
                        .list_for_items(&[updated.id.clone()])
                        .await?;
                    let (names, label, links_html) = super::render_blocked_by(
                        &project_id,
                        super::blocked_by_names_for(&updated, &siblings_ref, &dep_map),
                    )?;
                    row.blocked_by_names = names;
                    row.blocked_by_label = label;
                    row.blocked_by_links_html = links_html;
                    row.expanded_row = row.expanded_row || !row.blocked_by_names.is_empty();
                    row.render()?
                }
            };
            let (assignee_options, is_team_admin) = match &project.team_id {
                Some(team_id) => (
                    active_member_options(&teams, team_id, &auth_user.user_id).await?,
                    project_service::is_project_admin(
                        &projects,
                        &teams,
                        &project_id,
                        &auth_user.user_id,
                    )
                    .await,
                ),
                None => (Vec::new(), false),
            };
            let (depends_on_options, depends_on_item_ids) =
                depends_on_picker_data(&repo, &item_dependencies, &project_id, &updated).await?;
            let anchor_date =
                resolve_task_anchor_date(&repo, &project, &auth_user.user_id, &updated).await?;
            let fields = ProjectTaskDetailFields::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                assignee_options,
                is_team_admin,
                tz,
                true,
                false,
                depends_on_options,
                depends_on_item_ids,
                anchor_date,
            )
            .render()?;
            let linked_event = resolve_linked_event(&repo, &project_id, &updated).await?;
            let series_link = resolve_series_link(&series, &project_id, &updated).await?;
            let depends_on =
                resolve_depends_on_links(&repo, &item_dependencies, &project_id, &updated).await?;
            let item_reminders = reminders
                .list_for_item(&updated.id)
                .await
                .map_err(ItemError::from)?;
            let item_comments = comments
                .list_for_item(&updated.id)
                .await
                .map_err(ItemError::from)?;
            let item_attachments = attachments
                .list_for_item(&updated.id)
                .await
                .map_err(ItemError::from)?;
            let view = ProjectTaskDetailView::from_item(
                &updated,
                &project_id,
                project.team_id.is_some(),
                &names,
                tz,
                parent_link,
                linked_event,
                series_link,
                depends_on,
                item_reminders,
                item_comments,
                item_attachments,
                &auth_user.user_id,
                None,
                false,
            )
            .render()?;
            Ok(Html(format!("{row}{fields}{view}")).into_response())
        }
        Err(e) => Err(e),
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeleteItemQuery {
    /// Set only by the item's own read-only detail page's Delete button
    /// (`detail_page.html`) — the row-level "⋮" delete already lives on the list page and
    /// swaps its own row out in place; the detail page has no list to swap into, so it needs
    /// a full-page redirect back to the list instead.
    redirect: Option<String>,
}

pub async fn delete_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    Query(q): Query<DeleteItemQuery>,
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
    require_task(current)?;
    project_item_service::delete_project_item(
        &repo,
        &projects,
        &teams,
        &series,
        &reminders,
        &item_dependencies,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    if q.redirect.is_some() {
        return Ok((
            [(
                axum::http::header::HeaderName::from_static("hx-redirect"),
                project_tasks_list_url(&project_id),
            )],
            Html(String::new()),
        )
            .into_response());
    }
    Ok(Html(String::new()).into_response())
}

pub async fn duplicate_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Response, ItemError> {
    let item = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    require_task(item)?;
    project_item_service::duplicate_project_item(
        &repo,
        &projects,
        &teams,
        &auth_user.user_id,
        &project_id,
        &item_id,
    )
    .await?;
    let location = project_tasks_list_url(&project_id);
    Ok(hx_redirect(location))
}

/// Reparent-only update, every other field round-tripped from `current` — see
/// `tasks::reparent_params`/`team_tasks::reparent_params` for the full offset-recompute
/// rationale (identical here, just against `UpdateProjectItemParams`).
fn reparent_params(
    project_id: &str,
    item_id: &str,
    current: &Item,
    new_parent_item_id: Option<String>,
    offset_anchor: Option<DateTime<Utc>>,
    tz: i32,
) -> UpdateProjectItemParams {
    let (due_date, due_offset_days) = match (current.due_offset_days(), &new_parent_item_id) {
        (None, _) => (current.due_date(), None),
        (Some(_), Some(_)) => (
            offset_anchor.and_then(|anchor| current.deadline_from_offset(anchor, tz)),
            current.due_offset_days(),
        ),
        (Some(_), None) => (current.due_date(), None),
    };
    UpdateProjectItemParams {
        project_id: project_id.to_string(),
        item_id: item_id.to_string(),
        name: current.name.clone(),
        description: current.description.clone(),
        due_date,
        scheduled_date: current.scheduled_date(),
        scheduled_end_date: current.scheduled_end_date(),
        complete: current.complete,
        has_due_time: Some(current.has_due_time()),
        has_scheduled_time: Some(current.has_scheduled_time()),
        has_end_time: Some(current.has_end_time()),
        parent_item_id: new_parent_item_id,
        item_type: Some(current.kind()),
        event_type: current.event_type(),
        due_offset_days,
        assigned_to_user_id: current.assigned_to_user_id(),
        source_event_id: current.source_event_id(),
        timezone_offset_minutes: Some(tz),
        points: current.points(),
        priority: current.priority(),
        depends_on_item_ids: None,
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

/// Opens the "Move" dialog — see `templates::MoveDialog`'s doc comment for the unified promote/
/// subordinate rationale. `parent` is fetched unchecked since membership was already verified by
/// `get_project_item` above; a since-deleted parent (unlikely — nothing deletes a parent out from
/// under an in-flight dialog open, but `resolve_parent_link` treats it as possible elsewhere)
/// would surface as a `NotFound` here, which is an acceptable failure mode for opening a dialog.
pub async fn get_move_task_dialog(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let task = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let task = require_task(task)?;
    let parent = match &task.parent_item_id {
        Some(pid) => Some(repo.get_by_project(&project_id, pid).await?),
        None => None,
    };
    let siblings = sibling_group(&repo, &project_id, task.parent_item_id.as_deref()).await?;
    render(MoveDialog::new(
        &task,
        parent.as_ref(),
        &siblings,
        &project_id,
    ))
}

#[derive(serde::Deserialize, Debug)]
pub struct MoveForm {
    target: String,
}

/// Reparents this item per `form.target` — either `MOVE_TARGET_PARENT` ("promote": reparent onto
/// this item's own grandparent) or another item's id ("subordinate": reparent under that sibling)
/// — replacing what used to be two separate routes/handlers (`promote`/`subordinate`) now that
/// `MoveDialog` presents both as one picker. The redirect always lands back on this project's
/// Tasks list (never a moved-to parent's own detail page — the list already shows children
/// in place, and per-item detail pages are being retired as a navigation target).
pub async fn move_project_task_form(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(activity_log): Extension<Arc<dyn ActivityLogRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    Extension(reminders): Extension<Arc<dyn ReminderRepo>>,
    Extension(item_dependencies): Extension<Arc<dyn ItemDependencyRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<MoveForm>,
) -> Result<Response, ItemError> {
    item_dependencies_service::assert_movable(&item_dependencies, &item_id).await?;
    let (current, new_parent_item_id, offset_anchor) = if form.target == MOVE_TARGET_PARENT {
        let target = project_item_service::resolve_promotion_target(
            &repo,
            &projects,
            &teams,
            &project_id,
            &auth_user.user_id,
            &item_id,
        )
        .await?;
        (
            require_task(target.current)?,
            target.grandparent.map(|gp| gp.id),
            target.offset_anchor,
        )
    } else {
        let target = project_item_service::resolve_subordination_target(
            &repo,
            &projects,
            &teams,
            &project_id,
            &auth_user.user_id,
            &item_id,
            &form.target,
        )
        .await?;
        (
            require_task(target.current)?,
            Some(target.new_parent.id),
            target.offset_anchor,
        )
    };
    let params = reparent_params(
        &project_id,
        &item_id,
        &current,
        new_parent_item_id,
        offset_anchor,
        tz,
    );
    project_item_service::update_project_item(
        &repo,
        &projects,
        &teams,
        &activity_log,
        &series,
        &reminders,
        &item_dependencies,
        &auth_user.user_id,
        params,
    )
    .await?;
    Ok(hx_redirect(project_tasks_list_url(&project_id)))
}

pub async fn save_project_task_as_template(
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
    template_service::create_project_template(
        &repo,
        &projects,
        &teams,
        &auth_user.user_id,
        CreateProjectTemplateParams {
            project_id,
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

pub async fn get_reschedule_task(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<super::RowViewQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let task = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let task = require_task(task)?;
    let view = super::normalize_row_view(q);
    if task.is_offset_driven() {
        let anchor_date =
            resolve_task_anchor_date(&repo, &project, &auth_user.user_id, &task).await?;
        render(OffsetRescheduleDialog::from_task(
            &task,
            &project_id,
            tz,
            anchor_date,
            view.as_deref(),
        ))
    } else {
        render(RescheduleDialog::from_task(
            &task,
            &project_id,
            tz,
            view.as_deref(),
        ))
    }
}

pub async fn get_quick_assign_task(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<super::RowViewQuery>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let task = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let task = require_task(task)?;
    let assignee_options = match &project.team_id {
        Some(team_id) => active_member_options(&teams, team_id, &auth_user.user_id).await?,
        None => Vec::new(),
    };
    let view = super::normalize_row_view(q);
    render(QuickAssignDialog::from_task(
        &task,
        &project_id,
        assignee_options,
        view.as_deref(),
    ))
}

pub async fn get_add_child_task(
    Path((project_id, item_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    headers: HeaderMap,
) -> Result<Html<String>, ItemError> {
    let task = project_item_service::get_project_item(
        &repo,
        &projects,
        &teams,
        &project_id,
        &auth_user.user_id,
        &item_id,
    )
    .await?;
    let task = require_task(task)?;
    let current_url = headers.get("hx-current-url").and_then(|v| v.to_str().ok());
    render(AddChildDialog::new(&task, &project_id, current_url))
}

#[cfg(test)]
mod resolve_task_anchor_date_tests {
    use super::*;
    use crate::domain::item::{ItemType, Recurrence, Schedule, TaskItem};
    use crate::storage::sqlite::MockItemRepo;
    use chrono::TimeZone;

    fn team_project() -> Project {
        Project {
            id: "proj1".to_string(),
            name: "Team Project".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: Some("team1".to_string()),
        }
    }

    fn personal_project() -> Project {
        Project {
            id: "proj1".to_string(),
            name: "Personal Project".to_string(),
            owner_user_id: "owner1".to_string(),
            team_id: None,
        }
    }

    fn due_at(ts: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(ts, 0).unwrap()
    }

    fn task_with_due_date(id: &str, user_id: Option<&str>, due_date: DateTime<Utc>) -> Item {
        Item {
            id: id.to_string(),
            user_id: user_id.map(str::to_string),
            project_id: Some("proj1".to_string()),
            item_type: ItemType::Task(TaskItem {
                schedule: Schedule {
                    due_date: Some(due_date),
                    has_due_time: false,
                    scheduled_date: None,
                    has_scheduled_time: false,
                    scheduled_end_date: None,
                    has_end_time: false,
                },
                recurrence: Recurrence::default(),
                team_assignment: None,
                source_event_id: None,
                priority: None,
            }),
            ..Item::default()
        }
    }

    /// Regression test for the "completing an item returns a not found error" bug
    /// (docs/issues_and_features.md): a team-backed project's items are stored with a `NULL`
    /// `user_id` (see the row this was diagnosed from — a real team-project sub-item copied
    /// from prod). Resolving a sub-item's offset anchor must go through the project-scoped
    /// lookup (`get_by_project`), never the personal, `user_id`-scoped `get` — binding a real
    /// requester id against a `NULL` `user_id` column would never match and would surface as a
    /// spurious `NotFound`, even though the item's own read/write already succeeded.
    #[tokio::test]
    async fn team_backed_project_resolves_anchor_via_project_scoped_lookup() {
        let mut items = MockItemRepo::new();
        items
            .expect_get_by_project()
            .withf(|project_id, id| project_id == "proj1" && id == "parent1")
            .returning(|_, id| Ok(task_with_due_date(id, None, due_at(1_000))));
        items.expect_get().never();

        let repo: Arc<dyn ItemRepo> = Arc::new(items);
        let child = Item {
            id: "child1".to_string(),
            user_id: None,
            project_id: Some("proj1".to_string()),
            parent_item_id: Some("parent1".to_string()),
            ..Item::default()
        };

        let anchor = resolve_task_anchor_date(&repo, &team_project(), "requester1", &child)
            .await
            .expect("should resolve anchor");
        assert_eq!(anchor, Some(due_at(1_000)));
    }

    #[tokio::test]
    async fn personal_project_resolves_anchor_via_user_scoped_lookup() {
        let mut items = MockItemRepo::new();
        items
            .expect_get()
            .withf(|user_id, id| user_id == "owner1" && id == "parent1")
            .returning(|_, id| Ok(task_with_due_date(id, Some("owner1"), due_at(2_000))));
        items.expect_get_by_project().never();

        let repo: Arc<dyn ItemRepo> = Arc::new(items);
        let child = Item {
            id: "child1".to_string(),
            user_id: Some("owner1".to_string()),
            project_id: Some("proj1".to_string()),
            parent_item_id: Some("parent1".to_string()),
            ..Item::default()
        };

        let anchor = resolve_task_anchor_date(&repo, &personal_project(), "owner1", &child)
            .await
            .expect("should resolve anchor");
        assert_eq!(anchor, Some(due_at(2_000)));
    }
}
