use chrono::{DateTime, Datelike, Duration, Months, Utc, Weekday};
use rrule::{Frequency, NWeekday, RRule, RRuleSet, Tz};

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
    if let Some(time_override) = rule.time_override {
        let (utc_h, utc_m) = override_to_utc_hm(time_override, tz_offset_minutes);
        next.with_hour(utc_h).and_then(|d| d.with_minute(utc_m)).unwrap_or(next)
    } else {
        next
    }
}

/// Convert a local (hour, minute) `time_override` to UTC (hour, minute), given the
/// submitter's `tz_offset_minutes` (JavaScript `getTimezoneOffset()` convention:
/// UTC = local + tz_offset_minutes).
fn override_to_utc_hm(time_override: (u8, u8), tz_offset_minutes: i32) -> (u32, u32) {
    let (h, m) = time_override;
    let local_minutes = h as i32 * 60 + m as i32;
    let utc_minutes = (local_minutes + tz_offset_minutes).rem_euclid(24 * 60);
    ((utc_minutes / 60) as u32, (utc_minutes % 60) as u32)
}

/// Build an `rrule` crate `RRuleSet` equivalent to `rule`, anchored at `anchor`.
///
/// `MonthlyDay(31)` maps to `by_month_day(-1)` ("last day of month"), an exact match for
/// today's clamping behavior since 31 always clamps to the month's last day. `MonthlyDay(29)`
/// and `MonthlyDay(30)` map to their literal day number instead, which means RRULE's native
/// semantics apply: a February that's too short for that day is skipped entirely, rather than
/// clamped to Feb 28/29 the way `next_month_day` does. That's an intentional, narrow behavior
/// difference from `next_date`/`advance` above (see docs/recurring-events-virtual-occurrences-rough-plan.md).
fn to_rrule(rule: &RecurrenceRule, anchor: DateTime<Utc>, tz_offset_minutes: i32) -> Result<RRuleSet, String> {
    use chrono::Timelike;
    let dt_start_utc = if let Some(time_override) = rule.time_override {
        let (utc_h, utc_m) = override_to_utc_hm(time_override, tz_offset_minutes);
        anchor.with_hour(utc_h).and_then(|d| d.with_minute(utc_m)).unwrap_or(anchor)
    } else {
        anchor
    };
    let dt_start = dt_start_utc.with_timezone(&Tz::UTC);

    let interval = u16::try_from(rule.count).unwrap_or(u16::MAX);
    let rrule = match &rule.unit {
        RecurrenceUnit::Days => RRule::new(Frequency::Daily).interval(interval),
        RecurrenceUnit::Weeks => RRule::new(Frequency::Weekly).interval(interval),
        RecurrenceUnit::Months => RRule::new(Frequency::Monthly).interval(interval),
        RecurrenceUnit::Years => RRule::new(Frequency::Yearly).interval(interval),
        RecurrenceUnit::MonthlyDay(day) => {
            let month_day = if *day == 31 { -1 } else { *day as i8 };
            RRule::new(Frequency::Monthly).by_month_day(vec![month_day])
        }
        RecurrenceUnit::WeeklyDay(weekday) => {
            RRule::new(Frequency::Weekly).by_weekday(vec![NWeekday::Every(*weekday)])
        }
    };

    rrule.build(dt_start).map_err(|e| e.to_string())
}

/// Compute every occurrence of `rule` (anchored at `anchor`) within `[range_start, range_end]`,
/// inclusive of both ends — including `anchor` itself if it falls in range and matches the
/// pattern. This differs deliberately from `next_date`'s strictly-after-`reference` semantics:
/// `next_date` answers "what's next" for a single completed item, while this answers "what
/// occurrences exist in this window" for browsing/display, where the first occurrence landing
/// in the window should show up. On an internal `rrule` build failure (e.g. a degenerate rule
/// like "every 0 days", which `parse()` doesn't currently reject), returns an empty `Vec`
/// rather than panicking — no caller exists yet, so there's nothing to propagate an error to.
pub fn occurrences_between(
    rule: &RecurrenceRule,
    anchor: DateTime<Utc>,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    tz_offset_minutes: i32,
) -> Vec<DateTime<Utc>> {
    let Ok(rrule_set) = to_rrule(rule, anchor, tz_offset_minutes) else {
        return Vec::new();
    };
    rrule_set
        .after(range_start.with_timezone(&Tz::UTC))
        .before(range_end.with_timezone(&Tz::UTC))
        .all(u16::MAX)
        .dates
        .into_iter()
        .map(|d| d.with_timezone(&Utc))
        .collect()
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
    use chrono::TimeZone;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

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

    #[test]
    fn occurrences_every_3_days() {
        let rule = parse("every 3 days").unwrap();
        let anchor = dt(2026, 1, 1, 9, 0);
        let result = occurrences_between(&rule, anchor, dt(2026, 1, 1, 0, 0), dt(2026, 1, 10, 23, 59), 0);
        assert_eq!(
            result,
            vec![dt(2026, 1, 1, 9, 0), dt(2026, 1, 4, 9, 0), dt(2026, 1, 7, 9, 0), dt(2026, 1, 10, 9, 0)]
        );
    }

    #[test]
    fn occurrences_every_2_weeks_interval_spacing() {
        let rule = parse("every 2 weeks").unwrap();
        let anchor = dt(2026, 1, 5, 9, 0); // Monday
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 3, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![
                dt(2026, 1, 5, 9, 0),
                dt(2026, 1, 19, 9, 0),
                dt(2026, 2, 2, 9, 0),
                dt(2026, 2, 16, 9, 0),
            ]
        );
    }

    #[test]
    fn occurrences_every_month_on_the_1st() {
        let rule = parse("every month on the 1st").unwrap();
        let anchor = dt(2026, 1, 1, 9, 0);
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 4, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![dt(2026, 1, 1, 9, 0), dt(2026, 2, 1, 9, 0), dt(2026, 3, 1, 9, 0)]
        );
    }

    #[test]
    fn occurrences_every_month_on_the_15th() {
        let rule = parse("every month on the 15th").unwrap();
        let anchor = dt(2026, 1, 15, 9, 0);
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 4, 15, 0, 0), 0);
        assert_eq!(
            result,
            vec![
                dt(2026, 1, 15, 9, 0),
                dt(2026, 2, 15, 9, 0),
                dt(2026, 3, 15, 9, 0),
            ]
        );
    }

    #[test]
    fn occurrences_every_month_on_the_28th() {
        let rule = parse("every month on the 28th").unwrap();
        let anchor = dt(2026, 1, 28, 9, 0);
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 4, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![
                dt(2026, 1, 28, 9, 0),
                dt(2026, 2, 28, 9, 0),
                dt(2026, 3, 28, 9, 0),
            ]
        );
    }

    /// Day 29 is never out of range for any month except non-leap February, where RRULE's
    /// native semantics skip the month entirely rather than clamping to the 28th like
    /// `next_month_day`/`advance` do — a deliberate, narrow behavior change (see `to_rrule`).
    #[test]
    fn occurrences_every_month_on_the_29th_skips_non_leap_february() {
        let rule = parse("every month on the 29th").unwrap();
        let anchor = dt(2026, 1, 29, 9, 0); // 2026 is not a leap year
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 4, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![dt(2026, 1, 29, 9, 0), dt(2026, 3, 29, 9, 0)] // no Feb 2026 occurrence
        );
    }

    #[test]
    fn occurrences_every_month_on_the_29th_includes_leap_february() {
        let rule = parse("every month on the 29th").unwrap();
        let anchor = dt(2028, 1, 29, 9, 0); // 2028 is a leap year
        let result = occurrences_between(&rule, anchor, anchor, dt(2028, 4, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![dt(2028, 1, 29, 9, 0), dt(2028, 2, 29, 9, 0), dt(2028, 3, 29, 9, 0)]
        );
    }

    /// Day 30 clamps to Feb's last day today (`next_month_day`); via RRULE it skips February
    /// entirely instead, in every year (Feb never has 30 days) — same deliberate change as day 29.
    #[test]
    fn occurrences_every_month_on_the_30th_skips_february() {
        let rule = parse("every month on the 30th").unwrap();
        let anchor = dt(2026, 1, 30, 9, 0);
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 4, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![dt(2026, 1, 30, 9, 0), dt(2026, 3, 30, 9, 0)] // no Feb occurrence
        );
    }

    /// Day 31 always clamps to the month's last day today, in every month — so mapping it to
    /// RRULE's `by_month_day(-1)` ("last day of month") is an exact match, not an approximation.
    #[test]
    fn occurrences_every_month_on_the_31st_always_last_day() {
        let rule = parse("every month on the 31st").unwrap();
        let anchor = dt(2026, 1, 31, 9, 0);
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 5, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![
                dt(2026, 1, 31, 9, 0), // 31-day month: the 31st
                dt(2026, 2, 28, 9, 0), // Feb (non-leap): last day = 28th
                dt(2026, 3, 31, 9, 0), // 31-day month: the 31st
                dt(2026, 4, 30, 9, 0), // 30-day month: last day = 30th
            ]
        );
    }

    #[test]
    fn occurrences_every_weekday() {
        let rule = parse("every Monday").unwrap();
        // Anchor on a Wednesday: RRULE finds the next matching Monday forward from DTSTART,
        // it doesn't require DTSTART itself to already be a Monday.
        let anchor = dt(2026, 1, 7, 9, 0); // Wednesday
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 2, 1, 0, 0), 0);
        assert_eq!(
            result,
            vec![
                dt(2026, 1, 12, 9, 0),
                dt(2026, 1, 19, 9, 0),
                dt(2026, 1, 26, 9, 0),
            ]
        );
    }

    #[test]
    fn occurrences_inclusive_of_anchor_when_anchor_is_range_start() {
        let rule = parse("every week").unwrap();
        let anchor = dt(2026, 1, 5, 9, 0);
        let result = occurrences_between(&rule, anchor, anchor, anchor, 0);
        assert_eq!(result, vec![anchor]);
    }

    #[test]
    fn occurrences_empty_when_range_is_before_anchor() {
        let rule = parse("every day").unwrap();
        let anchor = dt(2026, 6, 1, 9, 0);
        let result = occurrences_between(&rule, anchor, dt(2026, 1, 1, 0, 0), dt(2026, 1, 31, 0, 0), 0);
        assert_eq!(result, Vec::<DateTime<Utc>>::new());
    }

    #[test]
    fn occurrences_time_override_applies_to_every_occurrence() {
        let rule = parse("every day at 8pm").unwrap();
        let anchor = dt(2026, 1, 1, 0, 0);
        // tz_offset_minutes = 300 (EST): local 20:00 + 300min offset = UTC 01:00.
        let result = occurrences_between(&rule, anchor, anchor, dt(2026, 1, 3, 23, 59), 300);
        assert_eq!(
            result,
            vec![dt(2026, 1, 1, 1, 0), dt(2026, 1, 2, 1, 0), dt(2026, 1, 3, 1, 0)]
        );
    }
}
