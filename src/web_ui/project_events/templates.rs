use crate::domain::item::Item;
use crate::web_ui::components::row::Row;
use crate::web_ui::to_local;
use askama::Template;
use chrono::Utc;

// ---- templates --------------------------------------------------------------

/// Builds a generic `components::row::Row` rather than an Event-specific template of its
/// own — the same reuse `ProjectTaskRow` (stage B5a) established. An Event is always
/// top-level (see `project_events::require_event`'s doc comment on why it can never have
/// structural children), so unlike `ProjectTaskRow` this never sets `offset_label`,
/// `assignee_name`, or `siblings` — an Event never carries any of those concepts. `complete`
/// is hardcoded `false`/`complete_url: None` (mirroring `ProjectSimpleItemRow`'s identical
/// precedent) — `Item::validate` rejects `complete: true` for `ItemType::Event` outright, so
/// there's no toggle to render.
pub struct ProjectEventRow;

impl ProjectEventRow {
    pub fn from_item(item: &Item, project_id: &str, tz: i32, skip_url: Option<String>) -> Row {
        Row {
            id: item.id.clone(),
            item_url: format!("/web/projects/{project_id}/events/{}", item.id),
            name: item.name.clone(),
            complete: false,
            due_date: item
                .due_date()
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            overdue: item.is_overdue(Utc::now()),
            scheduled_date: item.scheduled_date().map(|d| {
                let local = to_local(d, tz);
                if item.has_scheduled_time() {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            scheduled_end_date: item.scheduled_end_date().map(|d| {
                let local = to_local(d, tz);
                if item.has_end_time() {
                    local.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    local.format("%Y-%m-%d").to_string()
                }
            }),
            event_type: item.event_type(),
            expanded_row: true,
            has_children: false,
            offset_label: None,
            assignee_name: None,
            complete_url: None,
            edit_url: if item.google_event_id.is_some() {
                None
            } else {
                Some(format!(
                    "/web/projects/{project_id}/events/{}/edit",
                    item.id
                ))
            },
            duplicate_url: if item.google_event_id.is_some() {
                None
            } else {
                Some(format!(
                    "/web/projects/{project_id}/events/{}/duplicate",
                    item.id
                ))
            },
            reschedule_url: if item.google_event_id.is_some() {
                None
            } else {
                Some(format!(
                    "/web/projects/{project_id}/events/{}/reschedule",
                    item.id
                ))
            },
            assign_url: None,
            skip_url,
            toggle_complete_json: String::new(),
            siblings: Vec::new(),
            is_source_event_linked: false,
            show_complete: false,
            confirmation: None,
            dismiss_after_ms: None,
            is_imported: item.google_event_id.is_some(),
            // Calendar-only fields — see `Row`'s doc comments. The calendar screens build a
            // `Row` via this same `from_item` and then override these themselves.
            type_badge: None,
            parent_name: None,
            project_name: None,
            // Stage 2 of docs/dialog-item-forms-plan.md — project_events' detail/edit/new
            // pages are now dialog fragments.
            detail_via_dialog: true,
        }
    }
}

/// Stage D of `docs/unify-virtual-materialized-occurrences-plan.md` — an Event-series
/// virtual/skipped occurrence rendered as its own row in the flat `/events` list, mirroring
/// `project_tasks::templates::ProjectTaskVirtualRow`. No `is_current` field here (unlike
/// Tasks') — an Event-typed series has no cursor/"current" concept at all (see
/// `ItemSeries::cursor_date`'s doc comment), so every occurrence in the window is equally
/// "just upcoming."
#[derive(Template)]
#[template(path = "project_events/virtual_row.html")]
pub struct ProjectEventVirtualRow {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub name: String,
    pub date_label: String,
    pub materialize_url: String,
    pub skip_url: String,
    pub is_skipped: bool,
    pub unskip_url: String,
}

impl ProjectEventVirtualRow {
    pub fn from_occurrence(
        occ: &crate::service::item_series::ProjectOccurrence,
        project_id: &str,
        tz: i32,
    ) -> Self {
        let local = to_local(occ.occurrence_date, tz);
        Self {
            series_id: occ.series_id.clone(),
            occurrence_ts: occ.occurrence_date.timestamp(),
            name: occ.series_name.clone(),
            date_label: local.format("%Y-%m-%d %H:%M").to_string(),
            materialize_url: occ.materialize_url(project_id),
            skip_url: occ.skip_url(project_id),
            is_skipped: occ.is_skipped(),
            unskip_url: occ.unskip_url(project_id),
        }
    }
}

#[derive(Template)]
#[template(path = "project_events/detail_fields.html")]
pub struct ProjectEventDetailFields {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub scheduled_date_input: String,
    pub scheduled_time_input: String,
    pub scheduled_end_date_input: String,
    pub scheduled_end_time_input: String,
    pub due_date_input: String,
    pub due_time_input: String,
    pub event_type_input: String,
    /// Set only on the fragment returned by a successful save — see `items.rs`'s
    /// `DetailFields.just_saved` for the full rationale.
    pub just_saved: bool,
}

impl ProjectEventDetailFields {
    pub fn from_item(item: &Item, project_id: &str, tz: i32, just_saved: bool) -> Self {
        let local_scheduled_date = item.scheduled_date().map(|d| to_local(d, tz));
        let scheduled_date_input = local_scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_time_input = if item.has_scheduled_time() {
            local_scheduled_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_scheduled_end_date = item.scheduled_end_date().map(|d| to_local(d, tz));
        let scheduled_end_date_input = local_scheduled_end_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let scheduled_end_time_input = if item.has_end_time() {
            local_scheduled_end_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let local_due_date = item.due_date().map(|d| to_local(d, tz));
        let due_date_input = local_due_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let due_time_input = if item.has_due_time() {
            local_due_date
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            id: item.id.clone(),
            project_id: project_id.to_string(),
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            due_date_input,
            due_time_input,
            event_type_input: item.event_type().unwrap_or_default(),
            just_saved,
        }
    }
}

/// Read-only counterpart to `ProjectEventDetailFields` — see `items.rs`'s `DetailView` for
/// the row-editing convention this mirrors. No complete-toggle here (unlike Task's): an
/// Event can never be marked complete, see `Item::validate`.
#[derive(Template)]
#[template(path = "project_events/detail_view.html")]
pub struct ProjectEventDetailView {
    pub id: String,
    pub description: Option<String>,
    pub scheduled_date: Option<String>,
    pub scheduled_end_date: Option<String>,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub event_type: Option<String>,
    /// See `project_tasks::templates::ProjectTaskDetailView::series_link`'s identical
    /// rationale.
    pub series_link: Option<(String, String)>,
}

impl ProjectEventDetailView {
    pub fn from_item(item: &Item, tz: i32, series_link: Option<(String, String)>) -> Self {
        let scheduled_date = item.scheduled_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_scheduled_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let scheduled_end_date = item.scheduled_end_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_end_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        let due_date = item.due_date().map(|d| {
            let local = to_local(d, tz);
            if item.has_due_time() {
                local.format("%Y-%m-%d %H:%M").to_string()
            } else {
                local.format("%Y-%m-%d").to_string()
            }
        });
        Self {
            id: item.id.clone(),
            description: item.description.clone(),
            scheduled_date,
            scheduled_end_date,
            due_date,
            overdue: item.is_overdue(Utc::now()),
            event_type: item.event_type(),
            series_link,
        }
    }
}

/// The Event-flavored twin of `project_tasks::templates::RescheduleDialog` — see that struct's
/// doc comment for the shared-save-endpoint rationale (reuses `PUT
/// /web/projects/:project_id/events/:item_id`, i.e. `handlers::update_project_event_form`).
#[derive(Template)]
#[template(path = "components/reschedule_dialog.html")]
pub struct RescheduleDialog {
    pub item_id: String,
    pub post_reschedule_url: String,
    pub scheduled_start_date: String,
    pub scheduled_start_time: String,
    pub scheduled_end_date: String,
    pub scheduled_end_time: String,
    pub due_date: String,
    pub due_time: String,
}

impl RescheduleDialog {
    /// `view`: see `project_tasks::templates::RescheduleDialog::from_task`'s identical
    /// rationale.
    pub fn from_event(event: &Item, project_id: &str, tz: i32, view: Option<&str>) -> Self {
        let local_due_date = event.due_date().map(|d| to_local(d, tz));
        let local_scheduled_date = event.scheduled_date().map(|d| to_local(d, tz));
        let local_scheduled_end_date = event.scheduled_end_date().map(|d| to_local(d, tz));
        let post_reschedule_url = format!("/web/projects/{project_id}/events/{}", event.id);
        let post_reschedule_url = match view {
            Some(v) => format!("{post_reschedule_url}?view={v}"),
            None => post_reschedule_url,
        };
        RescheduleDialog {
            item_id: event.id.clone(),
            post_reschedule_url,
            due_date: local_due_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            due_time: if event.has_due_time() {
                local_due_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            scheduled_start_date: local_scheduled_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_start_time: if event.has_scheduled_time() {
                local_scheduled_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            scheduled_end_date: local_scheduled_end_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            scheduled_end_time: if event.has_end_time() {
                local_scheduled_end_date
                    .map(|d| d.format("%H:%M").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
        }
    }
}

/// Stage C of `docs/unify-virtual-materialized-occurrences-plan.md` — the Events counterpart
/// of `project_tasks::templates::ProjectTaskSeriesOccurrenceView`. No complete-toggle/
/// current-badge here, mirroring `ProjectEventDetailView`'s own precedent — an Event-typed
/// series has no cursor/completion concept at all (see `ItemSeries::cursor_date`'s doc
/// comment).
#[derive(Template)]
#[template(path = "project_events/series_occurrence_view.html")]
pub struct ProjectEventSeriesOccurrenceView {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub description: Option<String>,
    pub is_skipped: bool,
    pub scheduled_date: Option<String>,
    pub due_date: Option<String>,
    pub overdue: bool,
    pub event_type: Option<String>,
    pub skip_url: String,
    pub unskip_url: String,
    /// See `project_tasks::templates::ProjectTaskSeriesOccurrenceView::series_link`'s doc
    /// comment — identical rationale, Events counterpart.
    pub series_link: (String, String),
}

impl ProjectEventSeriesOccurrenceView {
    pub fn from_series(
        series: &crate::domain::item_series::ItemSeries,
        occurrence_date: chrono::DateTime<Utc>,
        project_id: &str,
        is_skipped: bool,
        tz: i32,
    ) -> Self {
        let occurrence_ts = occurrence_date.timestamp();
        let local = to_local(occurrence_date, tz);
        let is_due_date_basis = crate::service::item_series::is_due_date_basis(series);
        let due_date = is_due_date_basis.then(|| local.format("%Y-%m-%d %H:%M").to_string());
        let scheduled_date =
            (!is_due_date_basis).then(|| local.format("%Y-%m-%d %H:%M").to_string());
        Self {
            series_id: series.id.clone(),
            occurrence_ts,
            description: series.description.clone(),
            is_skipped,
            scheduled_date,
            overdue: is_due_date_basis && occurrence_date < Utc::now(),
            due_date,
            event_type: series.event_type.clone(),
            skip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/skip",
                series.id,
            ),
            unskip_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/unskip",
                series.id,
            ),
            series_link: (
                series.name.clone(),
                format!("/web/projects/{project_id}/series/{}/edit", series.id),
            ),
        }
    }
}

/// Stage C counterpart to `ProjectEventSeriesOccurrenceView` — the edit form for a still-
/// virtual Event series occurrence. Mirrors
/// `project_tasks::templates::ProjectTaskSeriesOccurrenceFields`, minus the
/// assignee/points/status fields Events never carry (see `ProjectEventDetailFields`).
#[derive(Template)]
#[template(path = "project_events/series_occurrence_fields.html")]
pub struct ProjectEventSeriesOccurrenceFields {
    pub series_id: String,
    pub occurrence_ts: i64,
    pub name: String,
    pub description: String,
    pub scheduled_date_input: String,
    pub scheduled_time_input: String,
    pub scheduled_end_date_input: String,
    pub scheduled_end_time_input: String,
    pub due_date_input: String,
    pub due_time_input: String,
    pub event_type_input: String,
    pub update_url: String,
}

impl ProjectEventSeriesOccurrenceFields {
    pub fn from_series(
        series: &crate::domain::item_series::ItemSeries,
        occurrence_date: chrono::DateTime<Utc>,
        project_id: &str,
        tz: i32,
    ) -> Self {
        let occurrence_ts = occurrence_date.timestamp();
        let local = to_local(occurrence_date, tz);
        let date_input = local.format("%Y-%m-%d").to_string();
        let time_input = local.format("%H:%M").to_string();
        let is_due_date_basis = crate::service::item_series::is_due_date_basis(series);
        Self {
            series_id: series.id.clone(),
            occurrence_ts,
            name: series.name.clone(),
            description: series.description.clone().unwrap_or_default(),
            scheduled_date_input: if is_due_date_basis {
                String::new()
            } else {
                date_input.clone()
            },
            scheduled_time_input: if is_due_date_basis {
                String::new()
            } else {
                time_input.clone()
            },
            scheduled_end_date_input: String::new(),
            scheduled_end_time_input: String::new(),
            due_date_input: if is_due_date_basis {
                date_input
            } else {
                String::new()
            },
            due_time_input: if is_due_date_basis {
                time_input
            } else {
                String::new()
            },
            event_type_input: series.event_type.clone().unwrap_or_default(),
            update_url: format!(
                "/web/projects/{project_id}/series/{}/occurrences/{occurrence_ts}/event",
                series.id,
            ),
        }
    }
}

/// Stage 2 of docs/dialog-item-forms-plan.md — the read-only dialog for a still-virtual/
/// skipped Event series occurrence, mirroring `ProjectEventDetailDialog` below.
#[derive(Template)]
#[template(path = "project_events/series_occurrence_detail_dialog.html")]
pub struct ProjectEventSeriesOccurrenceDetailDialog {
    pub name: String,
    pub is_skipped: bool,
    pub view: String,
    pub edit_url: String,
}

impl ProjectEventSeriesOccurrenceDetailDialog {
    pub fn new(
        project_id: &str,
        series_id: &str,
        occurrence_ts: i64,
        name: &str,
        is_skipped: bool,
        view: String,
    ) -> Self {
        Self {
            name: name.to_string(),
            is_skipped,
            view,
            edit_url: format!(
                "/web/projects/{project_id}/series/{series_id}/occurrences/{occurrence_ts}/edit"
            ),
        }
    }
}

#[derive(Template)]
#[template(path = "project_events/series_occurrence_detail_page.html")]
pub struct ProjectEventSeriesOccurrenceDetailPageTemplate {
    pub project_id: String,
    pub name: String,
    pub is_skipped: bool,
    pub view: String,
    /// See `ProjectEventDetailDialog` — rendered separately from `view` and embedded
    /// alongside it (Decision 3: a direct-URL/bookmark load auto-opens the same dialog an
    /// interactive trigger would have opened).
    pub dialog: String,
    pub child_create_url: String,
    pub edit_url: String,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_events/series_occurrence_edit_page.html")]
pub struct ProjectEventSeriesOccurrenceEditPageTemplate {
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}

/// Resolves the (series_name, edit-page URL) of the `ItemSeries` this item was materialized
/// from — the Events counterpart of
/// `project_tasks::templates::resolve_series_link`, identical rationale.
pub async fn resolve_series_link(
    series: &std::sync::Arc<dyn crate::storage::sqlite::ItemSeriesRepo>,
    project_id: &str,
    item: &Item,
) -> Result<Option<(String, String)>, crate::service::error::ItemError> {
    let Some(series_id) = &item.series_id else {
        return Ok(None);
    };
    // `delete_series` orphans rather than cascades (see its doc comment) — a materialized
    // item can outlive its parent series, so a missing series here means "no link to show",
    // not "this item doesn't exist."
    let series = match series.get_series(series_id).await {
        Ok(series) => series,
        Err(crate::storage::sqlite::RepoError::NotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some((
        series.name,
        format!("/web/projects/{project_id}/series/{series_id}/edit"),
    )))
}

#[derive(Template)]
#[template(path = "project_events/rows_fragment.html")]
pub struct ProjectEventRowsFragmentTemplate {
    pub rows: Vec<String>,
    pub empty_message: String,
}

#[derive(Template)]
#[template(path = "project_events/list_page.html")]
pub struct ProjectEventsListPageTemplate {
    pub project_id: String,
    pub rows: Vec<String>,
    pub is_admin: bool,
    pub nav_html: String,
}

#[derive(Template)]
#[template(path = "project_events/new_page.html")]
pub struct NewProjectEventPageTemplate {
    pub project_id: String,
    pub blank_event_type_input: String,
    pub blank_scheduled_date_input: String,
    pub blank_scheduled_time_input: String,
    pub blank_scheduled_end_date_input: String,
    pub blank_scheduled_end_time_input: String,
    pub nav_html: String,
}

/// Stage 2 of docs/dialog-item-forms-plan.md — the read-only detail dialog wrapping
/// `ProjectEventDetailView`'s already-rendered `view` HTML, mirroring
/// `project_tasks::templates::ProjectTaskDetailDialog`.
#[derive(Template)]
#[template(path = "project_events/detail_dialog.html")]
pub struct ProjectEventDetailDialog {
    pub name: String,
    pub is_imported: bool,
    pub view: String,
    pub edit_url: String,
    pub full_page_url: String,
}

impl ProjectEventDetailDialog {
    pub fn new(id: &str, project_id: &str, name: &str, is_imported: bool, view: String) -> Self {
        Self {
            name: name.to_string(),
            is_imported,
            view,
            edit_url: format!("/web/projects/{project_id}/events/{id}/edit"),
            full_page_url: format!("/web/projects/{project_id}/events/{id}"),
        }
    }
}

#[derive(Template)]
#[template(path = "project_events/detail_page.html")]
pub struct ProjectEventDetailPageTemplate {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub view: String,
    /// See `ProjectEventDetailDialog` — rendered separately from `view` and embedded
    /// alongside it (Decision 3: a direct-URL/bookmark load auto-opens the same dialog an
    /// interactive row-click would have opened).
    pub dialog: String,
    pub nav_html: String,
    /// Hides the "Edit" link — see `templates/project_events/detail_page.html` and
    /// CLAUDE.md's read-only-enforcement note. `service::project_items::update_project_item`
    /// rejects any edit of an imported item regardless, so this is purely to avoid showing a
    /// link that would only ever lead to an error.
    pub is_imported: bool,
}

#[derive(Template)]
#[template(path = "project_events/edit_page.html")]
pub struct ProjectEventEditPageTemplate {
    pub name: String,
    pub fields: String,
    pub nav_html: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{ItemSeriesRepo, MockItemSeriesRepo, RepoError};
    use std::sync::Arc;

    #[tokio::test]
    async fn resolve_series_link_returns_none_when_parent_series_was_deleted() {
        // Regression for the "delete a series, then its materialized occurrence 404s"
        // bug: `delete_series` orphans (see its doc comment) rather than cascading, so
        // `item.series_id` can point at a series row that no longer exists.
        let item = Item {
            series_id: Some("deleted-series".to_string()),
            ..Item::default()
        };
        let mut mock = MockItemSeriesRepo::new();
        mock.expect_get_series()
            .returning(|_| Err(RepoError::NotFound));
        let series: Arc<dyn ItemSeriesRepo> = Arc::new(mock);

        let result = resolve_series_link(&series, "p1", &item).await;

        assert_eq!(result.unwrap(), None);
    }

    fn series_fixture() -> crate::domain::item_series::ItemSeries {
        crate::domain::item_series::ItemSeries {
            id: "s1".to_string(),
            project_id: "p1".to_string(),
            name: "Standup".to_string(),
            description: None,
            event_type: None,
            recurrence: "every 7 days".to_string(),
            anchor_date: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            item_type: crate::domain::item::ItemKind::Event,
            cursor_date: None,
            basis: None,
            template_item_id: None,
            assigned_to_user_id: None,
            points: None,
        }
    }

    #[test]
    fn from_series_links_to_the_series_edit_page() {
        let series = series_fixture();

        let view = ProjectEventSeriesOccurrenceView::from_series(
            &series,
            series.anchor_date,
            "p1",
            false,
            0,
        );

        assert_eq!(
            view.series_link,
            (
                "Standup".to_string(),
                "/web/projects/p1/series/s1/edit".to_string()
            )
        );
    }
}
