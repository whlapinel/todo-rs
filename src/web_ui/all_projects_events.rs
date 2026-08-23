use crate::auth::AuthUser;
use crate::domain::item::ItemKind;
use crate::service::error::ItemError;
use crate::service::item_series::{self as item_series_service, OccurrenceState, ProjectOccurrence};
use crate::service::project_items::{self as project_item_service};
use crate::service::projects as project_service;
use crate::storage::sqlite::{ItemRepo, ItemSeriesRepo, ProjectRepo, UserRepo};
use crate::web_ui::TzOffset;
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::web_ui::project_events;
use crate::web_ui::project_events::templates::ProjectEventRow;
use crate::web_ui::to_local;
use askama::Template;
use axum::extract::Extension;
use axum::response::Html;
use chrono::Utc;
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Cross-project counterpart to `project_events::templates::ProjectEventVirtualRow` — an
/// Event series' virtual/skipped occurrence, tagged with the project it belongs to (mirrors
/// `all_projects_tasks::AllProjectsTaskVirtualRow`). No `is_current`/assignee fields here,
/// same rationale as `ProjectEventVirtualRow`'s own doc comment: an Event-typed series has no
/// cursor/"current" concept, so every occurrence in the window is shown.
#[derive(Template)]
#[template(path = "all_projects_events/virtual_row.html")]
struct AllProjectsEventVirtualRow {
    series_id: String,
    occurrence_ts: i64,
    project_name: String,
    name: String,
    date_label: String,
    materialize_url: String,
    skip_url: String,
    is_skipped: bool,
    unskip_url: String,
}

impl AllProjectsEventVirtualRow {
    fn from_occurrence(
        occ: &ProjectOccurrence,
        project_id: &str,
        project_name: &str,
        tz: i32,
    ) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            project_name: project_name.to_string(),
            name: occ.series_name.clone(),
            date_label: local.format("%Y-%m-%d %H:%M").to_string(),
            materialize_url: occ.materialize_url(project_id),
            skip_url: occ.skip_url(project_id),
            is_skipped: occ.is_skipped(),
            unskip_url: occ.unskip_url(project_id),
        }
    }
}

/// Builds a real Event item's row for this screen — `ProjectEventRow::from_item` plus the
/// `project_name` tag, mirroring `all_projects_tasks::all_projects_task_row`. No
/// `complete_url` rewrite (Events are never completable — `ProjectEventRow::from_item` already
/// hardcodes it `None`) and no toggle route of this screen's own, unlike Tasks'.
/// `reschedule_url` carries a `?view=all-events` suffix for `project_tasks::normalize_row_view`
/// to recognize once `docs/all-projects-landing-plan.md` Stage 4 wires it — until then it's
/// simply ignored, same transient gap `all_projects_task_row`'s identical suffix documents.
fn all_projects_event_row(
    item: &crate::domain::item::Item,
    project_id: &str,
    project_name: &str,
    tz: i32,
    skip_url: Option<String>,
) -> Result<String, ItemError> {
    let mut row = ProjectEventRow::from_item(item, project_id, tz, skip_url);
    row.project_name = Some(project_name.to_string());
    row.reschedule_url = row.reschedule_url.map(|url| format!("{url}?view=all-events"));
    Ok(row.render()?)
}

#[derive(Template)]
#[template(path = "all_projects_events/list_page.html")]
struct AllProjectsEventsListPageTemplate {
    rows: Vec<String>,
    nav_html: String,
}

/// Cross-project row assembly — one project at a time, mirroring
/// `all_projects_tasks::list_all_projects_task_rows`'s own gather shape (top-level Event items
/// + every still-virtual/skipped Event-series occurrence within
/// `project_events::virtual_occurrence_window`'s forward window — reused directly rather than
/// duplicated, since it's a pure constant-window function with no per-screen behavior to
/// diverge on) across every project the requester belongs to, each row tagged with its
/// project's name. No inclusion filter beyond kind==Event: `main_calendar::is_included`'s own
/// `Event => true` arm means Events are never assignment-restricted, unlike Tasks.
async fn list_all_projects_event_rows(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    users: &Arc<dyn UserRepo>,
    series: &Arc<dyn ItemSeriesRepo>,
    requester_user_id: &str,
    tz: i32,
) -> Result<Vec<String>, ItemError> {
    let user_projects = project_service::list_projects(projects, requester_user_id).await?;
    let (range_start, range_end) = project_events::virtual_occurrence_window(Utc::now());

    let mut entries: Vec<(i64, String)> = Vec::new();
    for project in &user_projects {
        let mut items =
            project_item_service::list_project_items_unchecked(repo, &project.id, None).await?;
        items.retain(|i| i.kind() == ItemKind::Event);

        for item in &items {
            let ts = item
                .scheduled_date()
                .or(item.due_date())
                .map(|d| d.timestamp())
                .unwrap_or(i64::MAX);
            let skip_url = item_series_service::skip_url_for_item(series, item, &project.id).await?;
            let html = all_projects_event_row(item, &project.id, &project.name, tz, skip_url)?;
            entries.push((ts, html));
        }

        let occurrences = item_series_service::list_occurrence_states_for_project(
            series,
            users,
            &project.id,
            range_start,
            range_end,
            tz,
        )
        .await?
        .into_iter()
        .filter(|occ| occ.item_type == ItemKind::Event)
        .filter(|occ| !matches!(occ.state, OccurrenceState::Materialized { .. }));
        for occ in occurrences {
            entries.push((
                occ.occurrence_date.timestamp(),
                AllProjectsEventVirtualRow::from_occurrence(&occ, &project.id, &project.name, tz)
                    .render()?,
            ));
        }
    }

    entries.sort_by_key(|(ts, _)| *ts);
    Ok(entries.into_iter().map(|(_, html)| html).collect())
}

pub async fn all_projects_events_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(series): Extension<Arc<dyn ItemSeriesRepo>>,
    TzOffset(tz): TzOffset,
) -> Result<Html<String>, ItemError> {
    let rows = list_all_projects_event_rows(
        &repo,
        &projects,
        &users,
        &series,
        &auth_user.user_id,
        tz,
    )
    .await?;
    let nav_html = nav::build_nav_html(
        &projects,
        &auth_user.user_id,
        ActiveContext::AllProjects,
        SidebarSection::Events,
    )
    .await?;
    render(AllProjectsEventsListPageTemplate { rows, nav_html })
}
