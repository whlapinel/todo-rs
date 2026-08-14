use crate::auth::AuthUser;
use crate::service::error::ItemError;
use crate::service::event_series::{self as event_series_service, CreateEventSeriesParams};
use crate::service::projects::{self as project_service};
use crate::storage::sqlite::{EventSeriesRepo, ProjectRepo, TeamRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_event_series::templates::*;
use crate::web_ui::project_event_series::{combine_local_to_utc, non_empty, render, start_of_day};
use crate::web_ui::TzOffset;
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::http::header::HeaderName;
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

fn active_context(project_id: &str) -> ActiveContext {
    ActiveContext::Project(project_id.to_string())
}

pub async fn project_event_series_page(
    Path(project_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(event_series): Extension<Arc<dyn EventSeriesRepo>>,
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
    Extension(event_series): Extension<Arc<dyn EventSeriesRepo>>,
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
        CreateEventSeriesParams {
            project_id: project_id.clone(),
            name: form.name.trim().to_string(),
            description: non_empty(&form.description),
            event_type: non_empty(&form.event_type),
            recurrence: form.recurrence.trim().to_string(),
            anchor_date,
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
