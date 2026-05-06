use chrono::{DateTime, Datelike, Duration, Months, Utc, Weekday};

#[derive(Debug, Clone)]
pub enum RecurrenceUnit {
    Days,
    Weeks,
    Months,
    Years,
    MonthlyDay(u32),
    WeeklyDay(Weekday),
}

#[derive(Debug, Clone)]
pub struct RecurrenceRule {
    pub unit: RecurrenceUnit,
    pub count: u32,
    pub raw: String,
    pub time_override: Option<(u8, u8)>, // (hour 0-23, minute 0-59) in local time
}

fn strip_time_suffix(s: &str) -> (&str, Option<(u8, u8)>) {
    if let Some(at_pos) = s.rfind(" at ") {
        let time_part = s[at_pos + 4..].trim();
        if let Some(hm) = parse_time_str(time_part) {
            return (&s[..at_pos], Some(hm));
        }
    }
    (s, None)
}

fn parse_time_str(s: &str) -> Option<(u8, u8)> {
    if s == "noon" { return Some((12, 0)); }
    if s == "midnight" { return Some((0, 0)); }
    let (s, pm) = if let Some(t) = s.strip_suffix("pm") { (t, true) }
                  else if let Some(t) = s.strip_suffix("am") { (t, false) }
                  else { return None; };
    let (h_str, m_str) = if let Some(colon) = s.find(':') {
        (&s[..colon], &s[colon + 1..])
    } else {
        (s, "0")
    };
    let h: u8 = h_str.parse().ok()?;
    let m: u8 = m_str.parse().ok()?;
    if h > 12 || m > 59 { return None; }
    let h24 = if pm { if h == 12 { 12 } else { h + 12 } } else { if h == 12 { 0 } else { h } };
    Some((h24, m))
}

pub fn parse(s: &str) -> Result<RecurrenceRule, String> {
    let s = s.trim().to_lowercase();
    let (base, time_override) = strip_time_suffix(&s);
    let base = base.trim();

    if let Some(rest) = base.strip_prefix("every month on the ") {
        let day: u32 = rest
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .map_err(|_| format!("invalid day number in \"{s}\""))?;
        if day < 1 || day > 31 {
            return Err(format!("day {day} out of range in \"{s}\""));
        }
        return Ok(RecurrenceRule { unit: RecurrenceUnit::MonthlyDay(day), count: 1, raw: s, time_override });
    }

    if let Some(weekday) = base.strip_prefix("every ").and_then(|w| parse_weekday(w.trim())) {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::WeeklyDay(weekday), count: 1, raw: s, time_override });
    }

    if base == "every day" || base == "daily" {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Days, count: 1, raw: s, time_override });
    }
    if let Some(n) = extract_n(base, "days") {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Days, count: n, raw: s, time_override });
    }

    if base == "every week" || base == "weekly" {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Weeks, count: 1, raw: s, time_override });
    }
    if let Some(n) = extract_n(base, "weeks") {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Weeks, count: n, raw: s, time_override });
    }

    if base == "every month" || base == "monthly" {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Months, count: 1, raw: s, time_override });
    }
    if let Some(n) = extract_n(base, "months") {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Months, count: n, raw: s, time_override });
    }

    if base == "every year" || base == "yearly" || base == "annually" {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Years, count: 1, raw: s, time_override });
    }
    if let Some(n) = extract_n(base, "years") {
        return Ok(RecurrenceRule { unit: RecurrenceUnit::Years, count: n, raw: s, time_override });
    }

    Err(format!(
        "unrecognized recurrence \"{s}\". Supported: \
        \"every N days/weeks/months/years\", \
        \"every month on the Nth\", \
        \"every [weekday]\", optionally followed by \"at H:MMam/pm\""
    ))
}

fn extract_n(s: &str, unit: &str) -> Option<u32> {
    let unit_singular = unit.strip_suffix('s').unwrap_or(unit);
    for prefix in &["every "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
            if parts.len() == 2 {
                if let Ok(n) = parts[0].parse::<u32>() {
                    if parts[1] == unit || parts[1] == unit_singular {
                        return Some(n);
                    }
                }
            }
        }
    }
    None
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    match s {
        "monday" | "mon" => Some(Weekday::Mon),
        "tuesday" | "tue" => Some(Weekday::Tue),
        "wednesday" | "wed" => Some(Weekday::Wed),
        "thursday" | "thu" => Some(Weekday::Thu),
        "friday" | "fri" => Some(Weekday::Fri),
        "saturday" | "sat" => Some(Weekday::Sat),
        "sunday" | "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

/// Compute the next due date after `reference`, advancing until the result is in the future.
/// `tz_offset_minutes`: value of `new Date().getTimezoneOffset()` from the client
/// (minutes to add to local time to get UTC; positive = west of UTC, e.g. 240 for EDT).
pub fn next_date(rule: &RecurrenceRule, reference: DateTime<Utc>, tz_offset_minutes: i32) -> DateTime<Utc> {
    let mut next = advance(rule, reference, tz_offset_minutes);
    let now = Utc::now();
    while next <= now {
        next = advance(rule, next, tz_offset_minutes);
    }
    next
}

fn advance(rule: &RecurrenceRule, from: DateTime<Utc>, tz_offset_minutes: i32) -> DateTime<Utc> {
    use chrono::Timelike;
    let next = match &rule.unit {
        RecurrenceUnit::Days => from + Duration::days(rule.count as i64),
        RecurrenceUnit::Weeks => from + Duration::weeks(rule.count as i64),
        RecurrenceUnit::Months => from + Months::new(rule.count),
        RecurrenceUnit::Years => from + Months::new(rule.count * 12),
        RecurrenceUnit::MonthlyDay(day) => next_month_day(from, *day),
        RecurrenceUnit::WeeklyDay(weekday) => next_weekday(from, *weekday),
    };
    if let Some((h, m)) = rule.time_override {
        // time_override is in the client's local time; convert to UTC.
        // UTC = local + tz_offset_minutes (JavaScript getTimezoneOffset() convention).
        let local_minutes = h as i32 * 60 + m as i32;
        let utc_minutes = (local_minutes + tz_offset_minutes).rem_euclid(24 * 60);
        let utc_h = (utc_minutes / 60) as u32;
        let utc_m = (utc_minutes % 60) as u32;
        next.with_hour(utc_h).and_then(|d| d.with_minute(utc_m)).unwrap_or(next)
    } else {
        next
    }
}

fn next_month_day(from: DateTime<Utc>, day: u32) -> DateTime<Utc> {
    let next_month = from + Months::new(1);
    let days_in_month = days_in_month(next_month.year(), next_month.month());
    let clamped = day.min(days_in_month);
    next_month.with_day(clamped).unwrap_or(next_month)
}

fn next_weekday(from: DateTime<Utc>, weekday: Weekday) -> DateTime<Utc> {
    let mut d = from + Duration::days(1);
    while d.weekday() != weekday {
        d = d + Duration::days(1);
    }
    d
}

/// Set the time component of `dt` to 23:59:59 in the client's local timezone.
pub fn apply_end_of_day(dt: DateTime<Utc>, tz_offset_minutes: i32) -> DateTime<Utc> {
    use chrono::Timelike;
    // Shift into local time, set 23:59:59, then shift back to UTC.
    // This correctly handles the date changing when the offset crosses midnight.
    let offset = Duration::minutes(tz_offset_minutes as i64);
    let local = dt - offset;
    let local_eod = local
        .with_hour(23).and_then(|d| d.with_minute(59)).and_then(|d| d.with_second(59))
        .unwrap_or(local);
    local_eod + offset
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    chrono::NaiveDate::from_ymd_opt(y, m, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_every_3_days() {
        let r = parse("every 3 days").unwrap();
        assert!(matches!(r.unit, RecurrenceUnit::Days));
        assert_eq!(r.count, 3);
    }

    #[test]
    fn parse_monthly_day() {
        let r = parse("every month on the 15th").unwrap();
        assert!(matches!(r.unit, RecurrenceUnit::MonthlyDay(15)));
    }

    #[test]
    fn parse_weekday() {
        let r = parse("every Monday").unwrap();
        assert!(matches!(r.unit, RecurrenceUnit::WeeklyDay(Weekday::Mon)));
    }

    #[test]
    fn parse_with_time_suffix() {
        let r = parse("every 3 days at 8pm").unwrap();
        assert!(matches!(r.unit, RecurrenceUnit::Days));
        assert_eq!(r.count, 3);
        assert_eq!(r.time_override, Some((20, 0)));
    }

    #[test]
    fn parse_invalid() {
        assert!(parse("every purple").is_err());
    }
}
