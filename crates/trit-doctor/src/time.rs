use std::time::{SystemTime, UNIX_EPOCH};

/// Python doctor 的 `at` 口径：UTC、秒级、`YYYY-MM-DDTHH:MM:SSZ`。
#[must_use]
pub fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64);
    let days = secs / 86_400;
    let secs_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant 的 days -> civil 算法（1970-01-01 起的天数）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = i64::from((5 * doy + 2) / 153);
    let day = i64::from(doy) - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970() {
        let (year, month, day) = civil_from_days(0);
        assert_eq!((year, month, day), (1970, 1, 1));
    }

    #[test]
    fn known_day_matches_civil_calendar() {
        // 2026-11-22 = 1970-01-01 起第 20,779 天。
        let (year, month, day) = civil_from_days(20_779);
        assert_eq!((year, month, day), (2026, 11, 22));
    }
}
