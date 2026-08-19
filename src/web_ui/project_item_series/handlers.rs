use crate::auth::AuthUser;
use crate::domain::item::ItemKind;
use crate::json_api::item_series::list_item_series_for_project;
use crate::service::error::ItemError;
use crate::service::item_series::{
    self as item_series_service, CreateItemSeriesParams, UpdateItemSeriesParams,
};
use crate::service::projects::{self as project_service};
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_item_series::templates::*;
use crate::web_ui::project_item_series::{
    combine_local_to_utc, end_of_day, non_empty, render, start_of_day,
};
use crate::web_ui::project_tasks::{active_member_options, names_for};
use crate::web_ui::{TzOffset, hx_redirect, to_local};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::http::HeaderMap;
use axum::http::header::HeaderName;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

/// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — Skip/Unskip act on a
/// single occurrence but are wired identically onto five different screens (Tasks list/
/// calendar, Dashboard list/calendar, Events calendar), each showing a different slice of
/// the project. Rather than teaching this generic series-scoped route how to render a
/// fragment matching whichever screen the request came from, this redirects back to
/// `HX-Current-URL` — the URL htmx automatically sends on every request — so the origin
/// screen simply re-renders itself with the mutation already applied. Falls back to the
/// series list if the header is somehow absent (a non-htmx request).
fn redirect_to_current_page(headers: &HeaderMap, project_id: &str) -> Response {
    let location = headers
        .get("hx-current-url")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| format!("/web/projects/{project_id}/series"));
    hx_redirect(location)
}

pub async fn project_item_series_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let series = item_series_service::list_series_for_project(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &project_id,
    )
    .await?;
    let template_names: HashMap<String, String> = repo
        .list_templates_by_project(&project_id)
        .await?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let member_names = match &project.team_id {
        Some(team_id) => names_for(&teams, team_id, &auth_user.user_id).await?,
        None => HashMap::new(),
    };
    let rows = series
        .iter()
        .map(|s| {
            let template_name = s
                .template_item_id
                .as_ref()
                .and_then(|id| template_names.get(id).cloned());
            let assignee_name = s
                .assigned_to_user_id
                .as_ref()
                .and_then(|id| member_names.get(id).cloned());
            ProjectItemSeriesRow::from_series(s, tz, template_name, assignee_name).render()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::ItemSeries,
    )
    .await?;
    render(ProjectItemSeriesListPageTemplate {
        project_id,
        rows,
        nav_html,
    })
}

pub async fn new_project_item_series_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let templates = repo
        .list_templates_by_project(&project_id)
        .await?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let is_team_project = project.team_id.is_some();
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(&teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::ItemSeries,
    )
    .await?;
    render(NewProjectItemSeriesPageTemplate {
        project_id,
        nav_html,
        templates,
        is_team_project,
        assignee_options,
        is_team_admin,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateItemSeriesForm {
    name: String,
    description: Option<String>,
    item_type: String,
    event_type: Option<String>,
    recurrence: String,
    anchor_date: String,
    anchor_time: Option<String>,
    /// A `<select>` of "" (schedule, the default) / "COMPLETION" / "DUE_DATE" — see
    /// `ItemSeries::basis`'s doc comment. A blank selection submits as `Some("")`;
    /// normalized to `None` via `non_empty` at each call site below, same convention as
    /// `template_item_id`.
    basis: Option<String>,
    /// Blank `<option value="">` submits as `Some("")` — `non_empty` (this module)
    /// normalizes that to `None`, same convention as `description`/`event_type`.
    template_item_id: Option<String>,
    /// Only present/honored server-side on a team-backed project, Task-typed series —
    /// see `service::item_series::resolve_series_assignment`'s own gate.
    assigned_to_user_id: Option<String>,
    /// Same team-project/Task-series-only caveat as `assigned_to_user_id`; additionally
    /// silently dropped server-side unless the requester is that project's admin.
    points: Option<String>,
}

fn parse_item_type(raw: &str) -> Result<ItemKind, ItemError> {
    match raw {
        "TASK" => Ok(ItemKind::Task),
        "EVENT" => Ok(ItemKind::Event),
        _ => Err(ItemError::Invalid(
            "itemType must be TASK or EVENT".to_string(),
        )),
    }
}

/// A due-date-basis series' anchor is a due date, which defaults a bare (no-time) input
/// to end-of-day — matching `due_date`'s own convention (CLAUDE.md's Scheduled start/end
/// section) rather than `scheduled_date`'s start-of-day default that every other basis
/// still uses.
fn anchor_default_time(basis: &Option<String>) -> chrono::NaiveTime {
    if basis.as_deref() == Some("DUE_DATE") {
        end_of_day()
    } else {
        start_of_day()
    }
}

pub async fn create_project_item_series_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<CreateItemSeriesForm>,
) -> Result<Response, ItemError> {
    let basis = non_empty(&form.basis);
    let anchor_date = combine_local_to_utc(
        form.anchor_date.trim(),
        form.anchor_time.as_deref(),
        tz,
        anchor_default_time(&basis),
    )
    .ok_or_else(|| ItemError::Invalid("anchor date is required".to_string()))?;
    let item_type = parse_item_type(&form.item_type)?;

    item_series_service::create_series(
        &repo,
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        CreateItemSeriesParams {
            project_id: project_id.clone(),
            name: form.name.trim().to_string(),
            description: non_empty(&form.description),
            event_type: non_empty(&form.event_type),
            recurrence: form.recurrence.trim().to_string(),
            anchor_date,
            item_type,
            basis,
            template_item_id: non_empty(&form.template_item_id),
            assigned_to_user_id: non_empty(&form.assigned_to_user_id),
            points: form
                .points
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse().ok()),
        },
    )
    .await?;

    Ok((
        [(
            HeaderName::from_static("hx-redirect"),
            format!("/web/projects/{project_id}/series"),
        )],
        Html(String::new()),
    )
        .into_response())
}

/// Stage 5 of docs/recurring-events-virtual-occurrences-rough-plan.md — the "click a virtual
/// occurrence" affordance. Materializes `(series_id, occurrence_ts)` into a real item (or
/// returns the existing one if already materialized) and redirects to its detail page.
///
/// The redirect target depends on the materialized item's own kind, via
/// `project_dashboard::detail_url` — Stage 7c added Task-typed series, so this route is no
/// longer Event-only, and reusing the dashboard's existing kind-to-URL mapping keeps this in
/// sync with every other screen's own routing rather than hand-rolling a narrower one here.
///
/// The `get_series` + project_id match check below is defense-in-depth for predictability,
/// not strictly required for security: `get_or_materialize_occurrence` already resolves
/// everything off `series.project_id` internally regardless of what's in the URL. But every
/// sibling project-scoped detail route 404s on a project_id/resource mismatch rather than
/// silently acting on a different project, and this route should behave the same way.
pub async fn materialize_project_item_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Response, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id {
        return Err(ItemError::NotFound);
    }

    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;

    let item = item_series_service::get_or_materialize_occurrence(
        &repo,
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
        tz,
    )
    .await?;

    Ok((
        [(
            HeaderName::from_static("hx-redirect"),
            crate::web_ui::project_dashboard::detail_url(&item, &project_id),
        )],
        Html(String::new()),
    )
        .into_response())
}

/// Stage 6 of docs/recurring-events-virtual-occurrences-rough-plan.md — the "Skip" button
/// wired onto occurrences (Tasks list/calendar, Dashboard list/calendar, Events calendar; see
/// each screen's own row/entry template). As of Stage B of
/// `docs/unify-virtual-materialized-occurrences-plan.md`, this works whether the occurrence
/// is still virtual or already materialized (`skip_or_delete_series_occurrence` deletes the
/// materialized item first, if there is one) and redirects back to the calling screen
/// (`redirect_to_current_page`) instead of returning an empty in-place-removal response, since
/// a skipped occurrence now stays visible (struck through, with an Unskip button) rather than
/// disappearing.
///
/// Same defense-in-depth project_id/series match check as materialize, for the same reason.
pub async fn skip_project_item_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
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
    if series.project_id != project_id {
        return Err(ItemError::NotFound);
    }

    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;

    item_series_service::skip_or_delete_series_occurrence(
        &repo,
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
        tz,
    )
    .await?;

    Ok(redirect_to_current_page(&headers, &project_id))
}

/// Stage B of `docs/unify-virtual-materialized-occurrences-plan.md` — the "Unskip" button
/// wired onto skipped occurrences everywhere `skip_project_item_series_occurrence_form`'s
/// Skip button is (see that handler's doc comment for the redirect rationale, which this
/// mirrors exactly). Un-materializing was never part of Skip's un-doing here — Skip already
/// leaves a materialized occurrence un-materialized before marking it exdate, so there's
/// nothing for Unskip to re-materialize; it only clears the exdate marker (and, for a
/// Task-typed series, restores the cursor) via `item_series_service::unskip_occurrence`.
pub async fn unskip_project_item_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
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
    if series.project_id != project_id {
        return Err(ItemError::NotFound);
    }

    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;

    item_series_service::unskip_occurrence(&item_series, &series_id, occurrence_date, tz).await?;

    Ok(redirect_to_current_page(&headers, &project_id))
}

pub async fn edit_project_item_series_page(
    Path((project_id, series_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let series = item_series_service::get_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id {
        return Err(ItemError::NotFound);
    }
    let project =
        project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let templates = repo
        .list_templates_by_project(&project_id)
        .await?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let is_team_project = project.team_id.is_some();
    let (assignee_options, is_team_admin) = match &project.team_id {
        Some(team_id) => (
            active_member_options(&teams, team_id, &auth_user.user_id).await?,
            project_service::is_project_admin(&projects, &teams, &project_id, &auth_user.user_id)
                .await,
        ),
        None => (Vec::new(), false),
    };
    let local_anchor = to_local(series.anchor_date, tz);
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::ItemSeries,
    )
    .await?;
    render(EditProjectItemSeriesPageTemplate {
        project_id,
        series_id,
        nav_html,
        name: series.name,
        description: series.description.unwrap_or_default(),
        is_task: series.item_type == ItemKind::Task,
        recurrence: series.recurrence,
        basis: series.basis.unwrap_or_default(),
        anchor_date: local_anchor.format("%Y-%m-%d").to_string(),
        anchor_time: local_anchor.format("%H:%M").to_string(),
        templates,
        template_item_id: series.template_item_id,
        is_team_project,
        assignee_options,
        assigned_to_user_id: series.assigned_to_user_id,
        is_team_admin,
        points: series.points,
    })
}

pub async fn update_project_item_series_form(
    Path((project_id, series_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<CreateItemSeriesForm>,
) -> Result<Response, ItemError> {
    let basis = non_empty(&form.basis);
    let anchor_date = combine_local_to_utc(
        form.anchor_date.trim(),
        form.anchor_time.as_deref(),
        tz,
        anchor_default_time(&basis),
    )
    .ok_or_else(|| ItemError::Invalid("anchor date is required".to_string()))?;
    let item_type = parse_item_type(&form.item_type)?;

    item_series_service::update_series(
        &repo,
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
        UpdateItemSeriesParams {
            name: form.name.trim().to_string(),
            description: non_empty(&form.description),
            event_type: non_empty(&form.event_type),
            recurrence: form.recurrence.trim().to_string(),
            anchor_date,
            item_type,
            basis,
            template_item_id: non_empty(&form.template_item_id),
            assigned_to_user_id: non_empty(&form.assigned_to_user_id),
            points: form
                .points
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse().ok()),
        },
    )
    .await?;

    Ok((
        [(
            HeaderName::from_static("hx-redirect"),
            format!("/web/projects/{project_id}/series"),
        )],
        Html(String::new()),
    )
        .into_response())
}

/// Orphan, not cascade — see `item_series_service::delete_series`'s doc comment. Already-
/// materialized occurrences survive as plain standalone items; only the series row and its
/// `item_occurrences` rows go away.
pub async fn delete_project_item_series_form(
    Path((_project_id, series_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Html<String>, ItemError> {
    item_series_service::delete_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    Ok(Html(String::new()))
}

pub async fn duplicate_project_item_series_form(
    Path((_project_id, series_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(item_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Response, ItemError> {
    item_series_service::duplicate_series(
        &projects,
        &teams,
        &item_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    let location = format!("/web/projects/{_project_id}/series");
    Ok(hx_redirect(location))
}
