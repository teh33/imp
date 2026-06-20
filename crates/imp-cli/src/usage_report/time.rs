use std::io;

use crate::reporting::BoundKind;

pub(crate) fn parse_usage_time_bound(
    raw: &str,
    kind: BoundKind,
) -> Result<u64, Box<dyn std::error::Error>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(io::Error::other("time bound cannot be empty").into());
    }

    if let Ok(timestamp) = trimmed.parse::<u64>() {
        return Ok(timestamp);
    }

    let (year, month, day) = parse_yyyy_mm_dd(trimmed).ok_or_else(|| {
        io::Error::other(format!(
            "invalid time bound '{trimmed}': expected unix timestamp or YYYY-MM-DD"
        ))
    })?;
    let day_start = day_start_timestamp(year, month, day)
        .ok_or_else(|| io::Error::other(format!("invalid calendar date '{trimmed}'")))?;

    Ok(match kind {
        BoundKind::Since => day_start,
        BoundKind::Until => day_start.saturating_add(86_400),
    })
}

fn parse_yyyy_mm_dd(value: &str) -> Option<(i32, u32, u32)> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((year, month, day))
}

fn day_start_timestamp(year: i32, month: u32, day: u32) -> Option<u64> {
    if !(1..=12).contains(&month) {
        return None;
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return None;
    }

    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    Some((days as u64) * 86_400)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

pub(super) fn format_utc_day(timestamp: u64) -> String {
    let (year, month, day) = civil_from_days((timestamp / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}
