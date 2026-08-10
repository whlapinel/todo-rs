use crate::auth::AuthUser;
use crate::domain::item::{Item, ItemKind};
use crate::handlers::web_ui::nav::{self, ActiveContext, SidebarSection};
use crate::handlers::web_ui::tasks;
use crate::handlers::web_ui::{TzOffset, to_local};
use crate::service::items::{self as item_service, ItemError};
use crate::service::templates::{self as template_service, CreateTemplateParams};
use crate::storage::sqlite::{ItemRepo, RepoError, TeamRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path, Query};
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

/// Guards every route below to the item actually being an Event — this screen renders
/// event-specific field layouts (scheduled window primary, no offset/Kind UI), so a Task or
/// Simple item's id reaching one of these handlers must 404 rather than render nonsense.
fn require_event(item: Item) -> Result<Item, ItemError> {
    if item.kind() == ItemKind::Event {
        Ok(item)
    } else {
        Err(ItemError::NotFound)
    }
}

// ---- form parsing helpers -------------------------------------------------
//
// Duplicated from `items.rs` rather than shared, matching the precedent `team_items.rs`
// already set for this exact helper set (see CLAUDE.md/issues.md's noted items/team_items
// duplication) — same three-way read convention: field absent => untouched, present-empty =>
// explicit clear, present-with-content => set.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventForm {
    name: Option<String>,
    description: Option<String>,
    scheduled_date: Option<String>,
    scheduled_time: Option<String>,
    scheduled_end_date: Option<String>,
    scheduled_end_time: Option<String>,
    due_date: Option<String>,
    due_time: Option<String>,
    event_type: Option<String>,
    complete: Option<String>,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    show_complete: Option<String>,
    /// Set on the standalone `/events/new` page's create form and on every edit form's
    /// "Save and close" submission — see `tasks.rs`'s identical field for the full rationale.
    redirect: Option<String>,
}

fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn overlay_str(form_value: &Option<String>, current: Option<String>) -> Option<String> {
    match form_value {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
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

fn overlay_has_due_time(form_time: &Option<String>, current: bool) -> bool {
    match form_time {
        None => current,
        Some(s) => !s.trim().is_empty(),
    }
}

fn combine_local_to_utc(
    date: &str,
    time: Option<&str>,
    tz_offset_minutes: i32,
    default_time: chrono::NaiveTime,
) -> Option<DateTime<Utc>> {
    let naive_date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let naive_time = time
        .filter(|t| !t.trim().is_empty())
        .and_then(|t| chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M").ok())
        .unwrap_or(default_time);
    let naive = naive_date.and_time(naive_time);
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Some(as_utc + chrono::Duration::minutes(tz_offset_minutes as i64))
}

fn end_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap()
}

fn start_of_day() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}

fn overlay_due_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            end_of_day(),
        ),
    }
}

fn overlay_scheduled_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            start_of_day(),
        ),
    }
}

fn overlay_scheduled_end_date(
    form_date: &Option<String>,
    form_time: &Option<String>,
    tz_offset_minutes: i32,
    current: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match form_date {
        None => current,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => combine_local_to_utc(
            s.trim(),
            form_time.as_deref(),
            tz_offset_minutes,
            end_of_day(),
        ),
    }
}

fn create_params_from_form(
    user_id: &str,
    form: &EventForm,
    tz: i32,
) -> item_service::CreateItemParams {
    item_service::CreateItemParams {
        user_id: user_id.to_string(),
        name: form.name.clone().unwrap_or_default(),
        description: non_empty(&form.description),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, None),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            None,
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            None,
        ),
        complete: form.complete.as_deref().map(|s| s == "true"),
        recurrence: non_empty(&form.recurrence),
        recurrence_basis: non_empty(&form.recurrence_basis),
        has_due_time: form.due_time.as_deref().map(|t| !t.trim().is_empty()),
        has_scheduled_time: form.scheduled_time.as_deref().map(|t| !t.trim().is_empty()),
        has_end_time: form
            .scheduled_end_time
            .as_deref()
            .map(|t| !t.trim().is_empty()),
        parent_item_id: None,
        item_type: Some(ItemKind::Event),
        event_type: non_empty(&form.event_type),
        due_offset_days: None,
        timezone_offset_minutes: Some(tz),
    }
}

fn update_params_from_form(
    user_id: &str,
    item_id: &str,
    current: &Item,
    form: &EventForm,
    tz: i32,
) -> item_service::UpdateItemParams {
    item_service::UpdateItemParams {
        user_id: user_id.to_string(),
        item_id: item_id.to_string(),
        name: overlay_required_str(&form.name, &current.name),
        description: overlay_str(&form.description, current.description.clone()),
        due_date: overlay_due_date(&form.due_date, &form.due_time, tz, current.due_date()),
        scheduled_date: overlay_scheduled_date(
            &form.scheduled_date,
            &form.scheduled_time,
            tz,
            current.scheduled_date(),
        ),
        scheduled_end_date: overlay_scheduled_end_date(
            &form.scheduled_end_date,
            &form.scheduled_end_time,
            tz,
            current.scheduled_end_date(),
        ),
        complete: overlay_bool(&form.complete, current.complete),
        recurrence: overlay_str(&form.recurrence, current.recurrence_pattern()),
        recurrence_basis: overlay_str(&form.recurrence_basis, current.recurrence_basis()),
        has_due_time: Some(overlay_has_due_time(&form.due_time, current.has_due_time())),
        has_scheduled_time: Some(overlay_has_due_time(
            &form.scheduled_time,
            current.has_scheduled_time(),
        )),
        has_end_time: Some(overlay_has_due_time(
            &form.scheduled_end_time,
            current.has_end_time(),
        )),
        parent_item_id: None,
        item_type: Some(ItemKind::Event),
        event_type: overlay_str(&form.event_type, current.event_type()),
        due_offset_days: None,
        timezone_offset_minutes: Some(tz),
    }
}

// ---- templates --------------------------------------------------------------

fn recurrence_basis_label(recurrence_basis: &Option<String>) -> String {
    match recurrence_basis.as_deref() {
        Some("COMPLETION_DATE") => "completion date".to_string(),
        Some("SCHEDULED_DATE") => "scheduled date".to_string(),
        Some(other) if other != "DUE_DATE" => other.to_string(),
        _ => "due date".to_string(),
    }
}

#[derive(Template)]
#[template(path = "events/row.html")]
struct EventRow {
    id: String,
    name: String,
    complete: bool,
    scheduled_date: Option<String>,
    scheduled_end_date: Option<String>,
    due_date: Option<String>,
    overdue: bool,
    event_type: Option<String>,
    has_children: bool,
    recurrence: Option<String>,
    toggle_complete_json: String,
}

impl EventRow {
    fn from_item(item: &Item, tz: i32) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            complete: item.complete,
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
            due_date: item
                .due_date()
                .map(|d| to_local(d, tz).format("%Y-%m-%d %H:%M").to_string()),
            overdue: item.is_overdue(Utc::now()),
            event_type: item.event_type(),
            has_children: item.has_children,
            recurrence: item.recurrence_pattern(),
            toggle_complete_json: (!item.complete).to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "events/detail_fields.html")]
struct EventDetailFields {
    id: String,
    name: String,
    description: String,
    complete: bool,
    scheduled_date_input: String,
    scheduled_time_input: String,
    scheduled_end_date_input: String,
    scheduled_end_time_input: String,
    due_date_input: String,
    due_time_input: String,
    event_type_input: String,
    recurrence: Option<String>,
    recurrence_basis: Option<String>,
    /// See `items.rs`'s `DetailFields.just_saved` — set only on the fragment returned by a
    /// successful save.
    just_saved: bool,
}

impl EventDetailFields {
    fn from_item(item: &Item, tz: i32, just_saved: bool) -> Self {
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
            name: item.name.clone(),
            description: item.description.clone().unwrap_or_default(),
            complete: item.complete,
            scheduled_date_input,
            scheduled_time_input,
            scheduled_end_date_input,
            scheduled_end_time_input,
            due_date_input,
            due_time_input,
            event_type_input: item.event_type().unwrap_or_default(),
            recurrence: item.recurrence_pattern(),
            recurrence_basis: item.recurrence_basis(),
            just_saved,
        }
    }
}

/// Read-only counterpart to `EventDetailFields` — see `items.rs`'s `DetailView` for the
/// row-editing convention this mirrors (complete-toggle lives here too, so marking an event
/// done doesn't require entering edit mode).
#[derive(Template)]
#[template(path = "events/detail_view.html")]
struct EventDetailView {
    id: String,
    description: Option<String>,
    complete: bool,
    toggle_complete_json: String,
    scheduled_date: Option<String>,
    scheduled_end_date: Option<String>,
    due_date: Option<String>,
    overdue: bool,
    event_type: Option<String>,
    recurrence: Option<String>,
    recurrence_basis_label: String,
}

impl EventDetailView {
    fn from_item(item: &Item, tz: i32) -> Self {
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
            complete: item.complete,
            toggle_complete_json: (!item.complete).to_string(),
            scheduled_date,
            scheduled_end_date,
            due_date,
            overdue: item.is_overdue(Utc::now()),
            event_type: item.event_type(),
            recurrence: item.recurrence_pattern(),
            recurrence_basis_label: recurrence_basis_label(&item.recurrence_basis()),
        }
    }
}

#[derive(Template)]
#[template(path = "events/rows_fragment.html")]
struct EventRowsFragmentTemplate {
    rows: Vec<String>,
    empty_message: String,
}

#[derive(Template)]
#[template(path = "events/list_page.html")]
struct EventsListPageTemplate {
    rows: Vec<String>,
    show_complete: bool,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "events/new_page.html")]
struct NewEventPageTemplate {
    show_complete: bool,
    blank_recurrence: Option<String>,
    blank_recurrence_basis: Option<String>,
    blank_event_type_input: String,
    blank_scheduled_date_input: String,
    blank_scheduled_time_input: String,
    blank_scheduled_end_date_input: String,
    blank_scheduled_end_time_input: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "events/detail_page.html")]
struct EventDetailPageTemplate {
    id: String,
    name: String,
    complete: bool,
    view: String,
    nav_html: String,
}

#[derive(Template)]
#[template(path = "events/edit_page.html")]
struct EventEditPageTemplate {
    id: String,
    name: String,
    fields: String,
    nav_html: String,
}

struct CalendarEventEntry {
    id: String,
    name: String,
    time_label: Option<String>,
}

struct CalendarDay {
    date: String,
    day_number: u32,
    is_current_month: bool,
    is_today: bool,
    events: Vec<CalendarEventEntry>,
}

#[derive(Template)]
#[template(path = "events/calendar_page.html")]
struct EventsCalendarPageTemplate {
    month_label: String,
    month_iso: String,
    prev_year: i32,
    prev_month: u32,
    next_year: i32,
    next_month: u32,
    days: Vec<CalendarDay>,
    nav_html: String,
}

// ---- shared rendering helpers ------------------------------------------------

fn render_rows(items: &[Item], show_complete: bool, tz: i32) -> Result<Vec<String>, ItemError> {
    items
        .iter()
        .filter(|i| show_complete || !i.complete)
        .map(|i| EventRow::from_item(i, tz).render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(ItemError::from)
}

/// Sort key for the events list: primary date is `scheduled_date` (falling back to
/// `due_date`), undated events last — see the plan's rationale for why Events makes the
/// scheduled window primary instead of `due_date` (the one axis CLAUDE.md flags as not yet
/// surfaced anywhere in the UI).
fn sort_key(item: &Item) -> i64 {
    item.scheduled_date()
        .or(item.due_date())
        .map(|d| d.timestamp())
        .unwrap_or(i64::MAX)
}

/// `repo.list` already scopes to top-level, non-Template items (see
/// `storage::sqlite::items::list`) — this narrows further to `Event` and re-sorts by the
/// scheduled-primary key above. No storage-layer filter/sort exists for this; filtering here
/// in the presentation layer is deliberate for a single screen (see the plan).
async fn list_events(repo: &Arc<dyn ItemRepo>, user_id: &str) -> Result<Vec<Item>, ItemError> {
    let mut items = repo.list(user_id).await.map_err(ItemError::from)?;
    items.retain(|i| i.kind() == ItemKind::Event);
    items.sort_by_key(sort_key);
    Ok(items)
}

fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 { (year - 1, 12) } else { (year, month - 1) }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 { (year + 1, 1) } else { (year, month + 1) }
}

/// The date each event displays under: `scheduled_date` if set, else `due_date` — same
/// scheduled-primary fallback `sort_key`/`list_events` above use for the flat list, kept
/// consistent so an event lands on the same day in both views. `calendar_has_time` mirrors it:
/// whichever field supplied the date is also the field whose `has_*_time` flag decides
/// whether a time label is shown.
fn calendar_date(item: &Item) -> Option<DateTime<Utc>> {
    item.scheduled_date().or(item.due_date())
}

fn calendar_has_time(item: &Item) -> bool {
    if item.scheduled_date().is_some() {
        item.has_scheduled_time()
    } else {
        item.has_due_time()
    }
}

/// Builds the 42-cell (6-week, Monday-start) grid for `year`/`month`, bucketing `items` by
/// local calendar day via `calendar_date`. Always 6 rows regardless of how many the month
/// actually spans, matching the fixed `grid-rows-6` layout of the vendored Tailwind Plus
/// component this view is adapted from.
fn build_calendar_days(
    year: i32,
    month: u32,
    items: &[Item],
    tz: i32,
    today: NaiveDate,
) -> Vec<CalendarDay> {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let leading = first_of_month.weekday().num_days_from_monday();
    let grid_start = first_of_month - chrono::Duration::days(leading as i64);

    let mut by_date: std::collections::HashMap<NaiveDate, Vec<CalendarEventEntry>> =
        std::collections::HashMap::new();
    for item in items {
        if let Some(dt) = calendar_date(item) {
            let local = to_local(dt, tz);
            let time_label = calendar_has_time(item).then(|| local.format("%H:%M").to_string());
            by_date
                .entry(local.date_naive())
                .or_default()
                .push(CalendarEventEntry {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    time_label,
                });
        }
    }

    let mut days = Vec::with_capacity(42);
    for i in 0..42i64 {
        let date = grid_start + chrono::Duration::days(i);
        let mut events = by_date.remove(&date).unwrap_or_default();
        events.sort_by(|a, b| a.time_label.cmp(&b.time_label));
        days.push(CalendarDay {
            date: date.format("%Y-%m-%d").to_string(),
            day_number: date.day(),
            is_current_month: date.month() == month && date.year() == year,
            is_today: date == today,
            events,
        });
    }
    days
}

// ---- handlers -----------------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowCompleteQuery {
    show_complete: Option<String>,
}

pub async fn events_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let show_complete = q.show_complete.is_some();
    let items = list_events(&repo, &auth_user.user_id).await?;
    let rows = render_rows(&items, show_complete, tz)?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Events,
    )
    .await?;
    render(EventsListPageTemplate {
        rows,
        show_complete,
        nav_html,
    })
}

#[derive(serde::Deserialize)]
pub struct CalendarQuery {
    year: Option<i32>,
    month: Option<u32>,
}

pub async fn events_calendar_page(
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

    let items = list_events(&repo, &auth_user.user_id).await?;
    let days = build_calendar_days(year, month, &items, tz, today);
    let (prev_year, prev_month) = prev_month(year, month);
    let (next_year, next_month) = next_month(year, month);
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Events,
    )
    .await?;

    render(EventsCalendarPageTemplate {
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

pub async fn new_event_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    Query(q): Query<ShowCompleteQuery>,
) -> Result<Html<String>, ItemError> {
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Events,
    )
    .await?;
    render(NewEventPageTemplate {
        show_complete: q.show_complete.is_some(),
        blank_recurrence: None,
        blank_recurrence_basis: Some("SCHEDULED_DATE".to_string()),
        blank_event_type_input: String::new(),
        blank_scheduled_date_input: String::new(),
        blank_scheduled_time_input: String::new(),
        blank_scheduled_end_date_input: String::new(),
        blank_scheduled_end_time_input: String::new(),
        nav_html,
    })
}

pub async fn event_detail_page(
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
    let item = require_event(item)?;
    let view = EventDetailView::from_item(&item, tz).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Events,
    )
    .await?;
    render(EventDetailPageTemplate {
        id: item.id,
        name: item.name,
        complete: item.complete,
        view,
        nav_html,
    })
}

pub async fn event_edit_page(
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
    let item = require_event(item)?;
    let fields = EventDetailFields::from_item(&item, tz, false).render()?;
    let nav_html = nav::build_nav_html(
        &team_repo,
        &auth_user.user_id,
        ActiveContext::Personal,
        SidebarSection::Events,
    )
    .await?;
    render(EventEditPageTemplate {
        id: item.id,
        name: item.name,
        fields,
        nav_html,
    })
}

/// Redirect back to the events list (via the `hx-redirect` header, same mechanism
/// `items.rs`'s `redirect_to_items` uses) after a create from the standalone `/events/new`
/// page.
fn redirect_to_events(show_complete: bool) -> Response {
    let location = if show_complete {
        "/web/events?showComplete=1".to_string()
    } else {
        "/web/events".to_string()
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

pub async fn create_event_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<EventForm>,
) -> Result<Response, ItemError> {
    let show_complete = form.show_complete.is_some();
    let params = create_params_from_form(&auth_user.user_id, &form, tz);
    item_service::create_item(&repo, params).await?;
    if form.redirect.is_some() {
        return Ok(redirect_to_events(show_complete));
    }
    let items = list_events(&repo, &auth_user.user_id).await?;
    let rows = render_rows(&items, show_complete, tz)?;
    Ok(render(EventRowsFragmentTemplate {
        rows,
        empty_message: "No events yet.".to_string(),
    })?
    .into_response())
}

pub async fn update_event_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    Extension(team_repo): Extension<Arc<dyn TeamRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<EventForm>,
) -> Result<Response, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    let current = require_event(current)?;
    let close = form.redirect.is_some();
    let params = update_params_from_form(&auth_user.user_id, &item_id, &current, &form, tz);
    item_service::update_item(&repo, params).await?;

    match repo.get(&auth_user.user_id, &item_id).await {
        Ok(updated) if close => {
            let view = EventDetailView::from_item(&updated, tz).render()?;
            let nav_html = nav::build_nav_html(
                &team_repo,
                &auth_user.user_id,
                ActiveContext::Personal,
                SidebarSection::Events,
            )
            .await?;
            Ok(render(EventDetailPageTemplate {
                id: updated.id.clone(),
                name: updated.name.clone(),
                complete: updated.complete,
                view,
                nav_html,
            })?
            .into_response())
        }
        Ok(updated) => {
            let row = EventRow::from_item(&updated, tz).render()?;
            let fields = EventDetailFields::from_item(&updated, tz, true).render()?;
            let view = EventDetailView::from_item(&updated, tz).render()?;
            Ok(Html(format!("{row}{fields}{view}")).into_response())
        }
        // The event was recurring, just got marked complete, and the service layer replaced
        // it with a fresh successor under a new id (see `service::items::update_item`) — same
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

pub async fn delete_event_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
) -> Result<Html<String>, ItemError> {
    let current = repo
        .get(&auth_user.user_id, &item_id)
        .await
        .map_err(ItemError::from)?;
    require_event(current)?;
    item_service::delete_item(&repo, &auth_user.user_id, &item_id).await?;
    Ok(Html(String::new()))
}

/// An event's own sub-items are always plain `Task`-typed (same as a Task's own children),
/// so this reuses `tasks::render_children_fragment` rather than duplicating `TaskRow`/its
/// template here — see that function's doc comment.
pub async fn event_children_fragment(
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
    tasks::render_children_fragment(&repo, &item_id, tz).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventChildForm {
    name: String,
    due_offset_days: Option<String>,
}

pub async fn create_event_child_form(
    Path(item_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(repo): Extension<Arc<dyn ItemRepo>>,
    TzOffset(tz): TzOffset,
    Form(form): Form<EventChildForm>,
) -> Result<Html<String>, ItemError> {
    let params = item_service::CreateItemParams {
        user_id: auth_user.user_id.clone(),
        name: form.name,
        parent_item_id: Some(item_id.clone()),
        item_type: Some(ItemKind::Task),
        due_offset_days: form
            .due_offset_days
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        timezone_offset_minutes: Some(tz),
        ..Default::default()
    };
    item_service::create_item(&repo, params).await?;
    tasks::render_children_fragment(&repo, &item_id, tz).await
}

pub async fn save_event_as_template(
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
