use crate::auth::AuthUser;
use crate::domain::item::ItemKind;
use crate::service::error::ItemError;
use crate::service::item_series::{self as event_series_service, CreateItemSeriesParams};
use crate::service::projects::{self as project_service};
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, TeamRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_event_series::templates::*;
use crate::web_ui::project_event_series::{combine_local_to_utc, non_empty, render, start_of_day};
use crate::web_ui::TzOffset;
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::http::header::HeaderName;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::sync::Arc;

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

pub async fn project_event_series_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let series = event_series_service::list_series_for_project(
        &projects,
        &teams,
        &event_series,
        &auth_user.user_id,
        &project_id,
    )
    .await?;
    let rows = series
        .iter()
        .map(|s| ProjectEventSeriesRow::from_series(s, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::EventSeries,
    )
    .await?;
    render(ProjectEventSeriesListPageTemplate {
        project_id,
        rows,
        nav_html,
    })
}

pub async fn new_project_event_series_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    project_service::get_project(&projects, &teams, &project_id, &auth_user.user_id).await?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        active_context(&project_id),
        SidebarSection::EventSeries,
    )
    .await?;
    render(NewProjectEventSeriesPageTemplate {
        project_id,
        nav_html,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEventSeriesForm {
    name: String,
    description: Option<String>,
    event_type: Option<String>,
    recurrence: String,
    anchor_date: String,
    anchor_time: Option<String>,
}

pub async fn create_project_event_series_form(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<CreateEventSeriesForm>,
) -> Result<Response, ItemError> {
    let anchor_date = combine_local_to_utc(
        form.anchor_date.trim(),
        form.anchor_time.as_deref(),
        tz,
        start_of_day(),
    )
    .ok_or_else(|| ItemError::Invalid("anchor date is required".to_string()))?;

    event_series_service::create_series(
        &projects,
        &teams,
        &event_series,
        &auth_user.user_id,
        CreateItemSeriesParams {
            project_id: project_id.clone(),
            name: form.name.trim().to_string(),
            description: non_empty(&form.description),
            event_type: non_empty(&form.event_type),
            recurrence: form.recurrence.trim().to_string(),
            anchor_date,
            // Hardcoded until Stage 7c adds a real item-type selector to this form —
            // the web UI only creates Event-typed series today.
            item_type: ItemKind::Event,
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
/// The `get_series` + project_id match check below is defense-in-depth for predictability,
/// not strictly required for security: `get_or_materialize_occurrence` already resolves
/// everything off `series.project_id` internally regardless of what's in the URL. But every
/// sibling project-scoped detail route 404s on a project_id/resource mismatch rather than
/// silently acting on a different project, and this route should behave the same way.
pub async fn materialize_project_event_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Response, ItemError> {
    let series = event_series_service::get_series(
        &projects,
        &teams,
        &event_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id {
        return Err(ItemError::NotFound);
    }

    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;

    let item = event_series_service::get_or_materialize_occurrence(
        &repo,
        &projects,
        &teams,
        &event_series,
        &auth_user.user_id,
        &series_id,
        occurrence_date,
    )
    .await?;

    Ok((
        [(
            HeaderName::from_static("hx-redirect"),
            format!("/web/projects/{project_id}/events/{}", item.id),
        )],
        Html(String::new()),
    )
        .into_response())
}

/// Stage 6 of docs/recurring-events-virtual-occurrences-rough-plan.md — the "Skip" button
/// wired onto virtual occurrences (dashboard list, dashboard calendar, events calendar; see
/// each screen's own row/entry template). Unlike materialize above, this never redirects —
/// callers target the occurrence's own row/entry element directly with `hx-swap="outerHTML"`
/// and an empty response removes it in place, since there's no detail page to send anyone to.
///
/// Same defense-in-depth project_id/series match check as materialize, for the same reason.
pub async fn skip_project_event_series_occurrence_form(
    Path((project_id, series_id, occurrence_ts)): Path<(String, String, i64)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn ItemSeriesRepo>>,
) -> Result<Response, ItemError> {
    let series = event_series_service::get_series(
        &projects,
        &teams,
        &event_series,
        &auth_user.user_id,
        &series_id,
    )
    .await?;
    if series.project_id != project_id {
        return Err(ItemError::NotFound);
    }

    let occurrence_date = DateTime::<Utc>::from_timestamp(occurrence_ts, 0)
        .ok_or_else(|| ItemError::Invalid("invalid occurrence timestamp".to_string()))?;

    event_series_service::skip_occurrence(&event_series, &series_id, occurrence_date).await?;

    Ok(Html(String::new()).into_response())
}
