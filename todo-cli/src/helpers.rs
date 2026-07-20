use chrono::{NaiveDate, TimeZone, Utc};
use todo_client::primitives::DateTime;

pub fn parse_date(s: &str) -> Result<DateTime, String> {
    if let Ok(secs) = s.parse::<i64>() {
        return Ok(DateTime::from_secs(secs));
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|d| {
            let ts = Utc
                .from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .timestamp();
            DateTime::from_secs(ts)
        })
        .map_err(|_| format!("invalid date '{s}' — use YYYY-MM-DD or a Unix timestamp"))
}

pub fn fmt_date(dt: &DateTime) -> String {
    if dt.secs() == 0 {
        return "-".to_string();
    }
    chrono::DateTime::from_timestamp(dt.secs(), 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub fn fmt_date_opt(dt: Option<&DateTime>) -> String {
    dt.map(fmt_date).unwrap_or_else(|| "-".into())
}

pub fn unwrap_or_exit<T, E: std::error::Error>(result: Result<T, E>, action: &str) -> T {
    result.unwrap_or_else(|e| {
        eprintln!(
            "error: {action} failed: {}",
            todo_client::error::DisplayErrorContext(e)
        );
        std::process::exit(1);
    })
}

pub fn require_user(user: Option<String>) -> String {
    user.unwrap_or_else(|| {
        eprintln!("error: no user ID — set one with `prl config set-user <id>` or pass --user");
        std::process::exit(1);
    })
}
