use chrono::{DateTime, Utc};

/// A project admin's subscription to one external (Google Calendar) iCal feed — see
/// docs/google-calendar-import-plan.md. `last_synced_at`/`last_sync_error` are written
/// by the Stage 3 sync engine after every attempt (`None`/`None` until the first sync
/// ever runs); a non-`None` `last_sync_error` doesn't clear previously-imported items,
/// it only reports that the most recent attempt failed (see the plan's Stage 3 diff
/// logic: a transient fetch failure leaves existing items untouched).
#[derive(Debug, Clone)]
pub struct CalendarSubscription {
    pub id: String,
    pub project_id: String,
    pub ical_url: String,
    pub created_by_user_id: String,
    pub created_at: DateTime<Utc>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_sync_error: Option<String>,
}
