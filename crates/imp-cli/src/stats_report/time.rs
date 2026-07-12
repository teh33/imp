#[derive(Debug, Clone, Copy)]
pub(super) enum PeriodKind {
    Day,
    Week,
}

pub(super) fn period_key(timestamp: u64, kind: PeriodKind) -> String {
    let days = (timestamp / 86_400) as i64;
    match kind {
        PeriodKind::Day => format_utc_day(days),
        PeriodKind::Week => format_utc_week(days),
    }
}

fn format_utc_day(days_since_epoch: i64) -> String {
    let (y, m, d) = civil_from_days(days_since_epoch);
    format!("{y:04}-{m:02}-{d:02}")
}

fn format_utc_week(days_since_epoch: i64) -> String {
    let thursday = days_since_epoch + 3;
    let (year, _, _) = civil_from_days(thursday);
    let jan_4 = days_from_civil(year, 1, 4);
    let week1_monday = jan_4 - ((jan_4 + 3).rem_euclid(7));
    let monday = days_since_epoch - ((days_since_epoch + 3).rem_euclid(7));
    let week = ((monday - week1_monday) / 7) + 1;
    format!("{year:04}-W{week:02}")
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
