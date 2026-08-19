use crate::domain::item::ItemKind;
use crate::service::error::ItemError;
use crate::service::project_items::{self, CreateProjectItemParams};
use crate::service::projects::require_project_member;
use crate::storage::sqlite::{ItemRepo, ProjectRepo, TeamRepo};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ImportItemResult {
    pub row_number: i32,
    pub success: bool,
    pub item_id: Option<String>,
    pub error: Option<String>,
}

fn ok_result(row_number: i32, item_id: String) -> ImportItemResult {
    ImportItemResult {
        row_number,
        success: true,
        item_id: Some(item_id),
        error: None,
    }
}

fn err_result(row_number: i32, error: String) -> ImportItemResult {
    ImportItemResult {
        row_number,
        success: false,
        item_id: None,
        error: Some(error),
    }
}

/// `YYYY-MM-DD` or a Unix timestamp — the same convention `--due`/`--scheduled` already use
/// (`todo-cli/src/helpers.rs::parse_date`). A Unix timestamp is already a precise instant, no
/// timezone involved. A bare `YYYY-MM-DD` has no time component, so — mirroring the web UI's
/// own `combine_local_to_utc` (e.g. `src/web_ui/project_tasks/mod.rs`) — it's interpreted as
/// `default_time` in the *caller's* local time (per `tz_offset_minutes`, the same
/// `X-Tz-Offset-Minutes`/JS-`getTimezoneOffset()` convention used everywhere else: minutes to
/// *add* to local time to get UTC), not literal UTC midnight — otherwise a date near a
/// timezone's midnight boundary lands on the wrong calendar day once displayed locally again.
fn parse_csv_date(s: &str, tz_offset_minutes: i32, default_time: NaiveTime) -> Result<DateTime<Utc>, String> {
    if let Ok(secs) = s.parse::<i64>() {
        return DateTime::from_timestamp(secs, 0).ok_or_else(|| format!("invalid timestamp '{s}'"));
    }
    let naive_date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("invalid date '{s}' — use YYYY-MM-DD or a Unix timestamp"))?;
    let naive = naive_date.and_time(default_time);
    let as_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    Ok(as_utc + Duration::minutes(tz_offset_minutes as i64))
}

/// No existing server-side bool-from-string convention to copy — this set
/// (`true`/`false`/`1`/`0`/`yes`/`no`, case-insensitive) is new to this file.
fn parse_csv_bool(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        other => Err(format!(
            "invalid boolean '{other}' — use true/false, 1/0, or yes/no"
        )),
    }
}

fn cell<'a>(record: &'a csv::StringRecord, headers: &HashMap<String, usize>, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|&idx| record.get(idx))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 23:59:59 — matches `src/web_ui/project_tasks/mod.rs::end_of_day` etc., the default a bare
/// date implies for a *deadline*-shaped field (`dueDate`/`scheduledEndDate`).
fn end_of_day() -> NaiveTime {
    NaiveTime::from_hms_opt(23, 59, 59).unwrap()
}

/// 00:00:00 — the default a bare date implies for a *window-start*-shaped field
/// (`scheduledDate`); a start defaulting to end-of-day would be backwards.
fn start_of_day() -> NaiveTime {
    NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}

fn build_row_params(
    record: &csv::StringRecord,
    headers: &HashMap<String, usize>,
    project_id: &str,
    tz_offset_minutes: i32,
) -> Result<CreateProjectItemParams, String> {
    let name = cell(record, headers, "name")
        .ok_or_else(|| "missing required column 'name' or empty value".to_string())?
        .to_string();

    if cell(record, headers, "recurrence").is_some() {
        return Err(
            "recurrence is no longer supported by CSV import — create an item series instead \
             (`prl series create` or the Item Series screen)"
                .to_string(),
        );
    }

    let due_date = cell(record, headers, "dueDate")
        .map(|s| parse_csv_date(s, tz_offset_minutes, end_of_day()))
        .transpose()?;
    let scheduled_date = cell(record, headers, "scheduledDate")
        .map(|s| parse_csv_date(s, tz_offset_minutes, start_of_day()))
        .transpose()?;
    let scheduled_end_date = cell(record, headers, "scheduledEndDate")
        .map(|s| parse_csv_date(s, tz_offset_minutes, end_of_day()))
        .transpose()?;

    let complete = cell(record, headers, "complete").map(parse_csv_bool).transpose()?;
    let has_due_time = cell(record, headers, "hasDueTime")
        .map(parse_csv_bool)
        .transpose()?;
    let has_scheduled_time = cell(record, headers, "hasScheduledTime")
        .map(parse_csv_bool)
        .transpose()?;
    let has_end_time = cell(record, headers, "hasEndTime")
        .map(parse_csv_bool)
        .transpose()?;

    let item_type = cell(record, headers, "itemType")
        .map(|s| s.parse::<ItemKind>())
        .transpose()?;

    let due_offset_days = cell(record, headers, "dueOffsetDays")
        .map(|s| {
            s.parse::<i32>()
                .map_err(|_| format!("invalid integer '{s}' for dueOffsetDays"))
        })
        .transpose()?;
    let points = cell(record, headers, "points")
        .map(|s| {
            s.parse::<i32>()
                .map_err(|_| format!("invalid integer '{s}' for points"))
        })
        .transpose()?;

    Ok(CreateProjectItemParams {
        project_id: project_id.to_string(),
        name,
        description: cell(record, headers, "description").map(str::to_string),
        due_date,
        scheduled_date,
        scheduled_end_date,
        complete,
        has_due_time,
        has_scheduled_time,
        has_end_time,
        parent_item_id: cell(record, headers, "parentItemId").map(str::to_string),
        item_type,
        event_type: cell(record, headers, "eventType").map(str::to_string),
        due_offset_days,
        assigned_to_user_id: cell(record, headers, "assignedToUserId").map(str::to_string),
        source_event_id: cell(record, headers, "sourceEventId").map(str::to_string),
        timezone_offset_minutes: None,
        points,
        series_id: None,
    })
}

/// Server-side CSV import: the caller (CLI/MCP) sends raw CSV text, this function does all
/// parsing + validation + creation — see CLAUDE.md's "CSV import" section for the full
/// rationale. Best-effort: every row is attempted independently, valid rows are created,
/// invalid ones collect into a failure result, nothing rolls back. Row numbers are 1-based
/// counting the header as row 1, so the first data row is 2.
#[allow(clippy::too_many_arguments)]
pub async fn import_project_items(
    repo: &Arc<dyn ItemRepo>,
    projects: &Arc<dyn ProjectRepo>,
    teams: &Arc<dyn TeamRepo>,
    requester_user_id: &str,
    project_id: &str,
    csv: &str,
    format: Option<&str>,
    timezone_offset_minutes: Option<i32>,
) -> Result<Vec<ImportItemResult>, ItemError> {
    require_project_member(projects, teams, project_id, requester_user_id).await?;

    match format.unwrap_or("PRL") {
        "PRL" => {}
        other => {
            return Err(ItemError::Invalid(format!(
                "unknown import format '{other}' — only 'PRL' is currently supported"
            )));
        }
    }

    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(csv.as_bytes());
    let headers: HashMap<String, usize> = reader
        .headers()
        .map_err(|e| ItemError::Invalid(format!("failed to read CSV header row: {e}")))?
        .iter()
        .enumerate()
        .map(|(idx, name)| (name.to_string(), idx))
        .collect();

    let mut results = Vec::new();
    for (idx, record) in reader.records().enumerate() {
        let row_number = idx as i32 + 2;
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                results.push(err_result(row_number, e.to_string()));
                continue;
            }
        };

        let mut params = match build_row_params(
            &record,
            &headers,
            project_id,
            timezone_offset_minutes.unwrap_or(0),
        ) {
            Ok(p) => p,
            Err(msg) => {
                results.push(err_result(row_number, msg));
                continue;
            }
        };
        params.timezone_offset_minutes = timezone_offset_minutes;

        match project_items::create_project_item(repo, projects, teams, requester_user_id, params).await {
            Ok(item_id) => results.push(ok_result(row_number, item_id)),
            Err(e) => results.push(err_result(row_number, e.to_string())),
        }
    }

    Ok(results)
}

const ITEM_IMPORT_TEMPLATE_CSV: &str = "\
name,description,dueDate,scheduledDate,scheduledEndDate,complete,hasDueTime,hasScheduledTime,hasEndTime,itemType,eventType,dueOffsetDays,parentItemId,assignedToUserId,points,sourceEventId
Submit report,Quarterly report for finance,2026-09-01,,,false,,,,TASK,,,,,,
Team offsite,,,2026-09-10,2026-09-12,false,,,,EVENT,offsite,,,,,
Buy milk,,,,,false,,,,SIMPLE,,,,,,
";

pub fn item_import_template() -> String {
    ITEM_IMPORT_TEMPLATE_CSV.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::Item;
    use crate::domain::project::Project;
    use crate::storage::sqlite::{MockItemRepo, MockProjectRepo, MockTeamRepo};

    fn test_project(id: &str, owner: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "Test Project".to_string(),
            owner_user_id: owner.to_string(),
            team_id: None,
        }
    }

    #[tokio::test]
    async fn import_project_items_creates_valid_rows_and_reports_invalid_rows_independently() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let mut repo = MockItemRepo::new();
        repo.expect_create()
            .times(2)
            .returning(|item: &Item| Ok(item.id.clone()));

        let teams = MockTeamRepo::new();

        let csv_text = "name,dueDate\nGood row 1,2026-09-01\n,2026-09-02\nGood row 2,2026-09-03\n";
        let results = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            csv_text,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 3);
        assert!(results[0].success);
        assert!(!results[1].success);
        assert_eq!(results[1].row_number, 3);
        assert!(results[1]
            .error
            .as_deref()
            .unwrap()
            .contains("missing required column 'name'"));
        assert!(results[2].success);
    }

    #[tokio::test]
    async fn import_project_items_is_header_order_independent() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let mut repo = MockItemRepo::new();
        repo.expect_create()
            .withf(|item: &Item| item.name == "Reordered" && item.due_date().is_some())
            .times(1)
            .returning(|item: &Item| Ok(item.id.clone()));

        let teams = MockTeamRepo::new();

        let csv_text = "dueDate,name\n2026-09-01,Reordered\n";
        let results = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            csv_text,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn import_project_items_handles_missing_and_blank_optional_columns() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let mut repo = MockItemRepo::new();
        repo.expect_create()
            .withf(|item: &Item| item.name == "Bare" && item.description.is_none())
            .times(1)
            .returning(|item: &Item| Ok(item.id.clone()));

        let teams = MockTeamRepo::new();

        let csv_text = "name,description\nBare,\n";
        let results = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            csv_text,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn import_project_items_parses_dates_as_ymd_and_unix_timestamp() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let mut repo = MockItemRepo::new();
        repo.expect_create()
            .times(2)
            .returning(|item: &Item| Ok(item.id.clone()));

        let teams = MockTeamRepo::new();

        let csv_text = "name,dueDate\nYMD,2026-09-01\nEpoch,1893456000\n";
        let results = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            csv_text,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);
    }

    /// Regression test for a real bug report: importing `dueDate=2026-09-30` with no timezone
    /// awareness landed on `2026-09-30T00:00:00Z`, which a US-EDT (UTC-4) viewer sees as
    /// `2026-09-29T20:00:00-04:00` — the wrong calendar day. `tz_offset_minutes` (JS
    /// `getTimezoneOffset()` convention: minutes to *add* to local time to reach UTC, so EDT is
    /// `+240`) must shift a bare date's default end-of-day time into the correct UTC instant, the
    /// same way `src/web_ui/project_tasks/mod.rs::combine_local_to_utc` already does for the
    /// browser-submitted case.
    #[tokio::test]
    async fn import_project_items_interprets_bare_dates_in_the_callers_timezone() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let mut repo = MockItemRepo::new();
        repo.expect_create()
            .withf(|item: &Item| {
                item.due_date()
                    == Some(
                        chrono::DateTime::parse_from_rfc3339("2026-10-01T03:59:59Z")
                            .unwrap()
                            .with_timezone(&Utc),
                    )
            })
            .times(1)
            .returning(|item: &Item| Ok(item.id.clone()));

        let teams = MockTeamRepo::new();

        let csv_text = "name,dueDate\nEat donuts,2026-09-30\n";
        let results = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            csv_text,
            None,
            Some(240), // US-EDT
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn import_project_items_surfaces_create_item_validation_errors_per_row() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let repo = MockItemRepo::new(); // no `create` calls expected — scheduledEndDate < scheduledDate is rejected before reaching the repo

        let teams = MockTeamRepo::new();

        let csv_text =
            "name,scheduledDate,scheduledEndDate\nBad window,2026-09-10,2026-09-01\n";
        let results = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            csv_text,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0].error.is_some());
    }

    /// Gap 4 of the item_series redesign (`docs/recurring-events-virtual-occurrences-rough-plan.md`,
    /// Stage 10): CSV import no longer creates legacy recurring items — a row with `recurrence`
    /// set is a hard error, not silently dropped/ignored. Anyone wanting a recurring series
    /// creates it once via the UI/CLI/MCP series endpoints instead.
    #[tokio::test]
    async fn import_project_items_rejects_rows_with_a_recurrence_column() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let repo = MockItemRepo::new(); // no `create` calls expected

        let teams = MockTeamRepo::new();

        let csv_text = "name,recurrence\nBad row,every week\n";
        let results = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            csv_text,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0]
            .error
            .as_deref()
            .unwrap()
            .contains("no longer supported"));
    }

    #[tokio::test]
    async fn import_project_items_rejects_unknown_format() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "u1")));
        projects
            .expect_find_personal_project()
            .returning(|_| Ok(None));

        let repo = MockItemRepo::new();
        let teams = MockTeamRepo::new();

        let result = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            "name\nAnything\n",
            Some("TODOIST"),
            None,
        )
        .await;

        assert!(matches!(result, Err(ItemError::Invalid(_))));
    }

    #[tokio::test]
    async fn import_project_items_rejects_non_member() {
        let mut projects = MockProjectRepo::new();
        projects
            .expect_get()
            .returning(|id| Ok(test_project(id, "someone-else")));

        let repo = MockItemRepo::new();
        let teams = MockTeamRepo::new();

        let result = import_project_items(
            &(Arc::new(repo) as Arc<dyn ItemRepo>),
            &(Arc::new(projects) as Arc<dyn ProjectRepo>),
            &(Arc::new(teams) as Arc<dyn TeamRepo>),
            "u1",
            "p1",
            "name\nAnything\n",
            None,
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[test]
    fn item_import_template_has_three_rows_matching_prl_header_columns() {
        let template = item_import_template();
        let mut reader = csv::Reader::from_reader(template.as_bytes());
        let headers: Vec<String> = reader.headers().unwrap().iter().map(str::to_string).collect();
        assert_eq!(
            headers,
            vec![
                "name",
                "description",
                "dueDate",
                "scheduledDate",
                "scheduledEndDate",
                "complete",
                "hasDueTime",
                "hasScheduledTime",
                "hasEndTime",
                "itemType",
                "eventType",
                "dueOffsetDays",
                "parentItemId",
                "assignedToUserId",
                "points",
                "sourceEventId",
            ]
        );

        let records: Vec<csv::StringRecord> = reader.records().collect::<Result<_, _>>().unwrap();
        assert_eq!(records.len(), 3);
        let item_type_idx = headers.iter().position(|h| h == "itemType").unwrap();
        let types: Vec<&str> = records.iter().map(|r| r.get(item_type_idx).unwrap()).collect();
        assert_eq!(types, vec!["TASK", "EVENT", "SIMPLE"]);
    }
}
