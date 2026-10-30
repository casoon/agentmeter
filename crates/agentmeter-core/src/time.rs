//! Minimal UTC time handling; the logs only need ISO 8601 and calendar days.

pub const DAY_MS: i64 = 86_400_000;
pub const HOUR_MS: i64 = 3_600_000;
pub const MINUTE_MS: i64 = 60_000;

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn digits(text: &str, start: usize, len: usize) -> Option<i64> {
    let part = text.get(start..start + len)?;
    if part.bytes().all(|b| b.is_ascii_digit()) {
        part.parse().ok()
    } else {
        None
    }
}

/// Parses `YYYY-MM-DD` into milliseconds at 00:00 UTC.
pub fn parse_date_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let (year, month, day) = (
        digits(text, 0, 4)?,
        digits(text, 5, 2)?,
        digits(text, 8, 2)?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * DAY_MS)
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)` into milliseconds since the epoch, UTC.
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let date = parse_date_ms(text.get(..10)?)?;
    let (hour, minute, second) = (
        digits(text, 11, 2)?,
        digits(text, 14, 2)?,
        digits(text, 17, 2)?,
    );
    let mut millis = 0;
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let length = fraction.bytes().take_while(u8::is_ascii_digit).count();
        let mut padded = fraction[..length.min(3)].to_owned();
        while padded.len() < 3 {
            padded.push('0');
        }
        millis = padded.parse::<i64>().ok()?;
        rest = &fraction[length..];
    }
    let offset = match rest {
        "" | "Z" => 0,
        _ if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            sign * (digits(rest, 1, 2)? * HOUR_MS + digits(rest, 4, 2)? * MINUTE_MS)
        }
        _ => return None,
    };
    Some(date + hour * HOUR_MS + minute * MINUTE_MS + second * 1000 + millis - offset)
}

/// `YYYY-MM-DD` of the UTC day containing `ms`.
pub fn format_day(ms: i64) -> String {
    let (year, month, day) = civil_from_days(ms.div_euclid(DAY_MS));
    format!("{year:04}-{month:02}-{day:02}")
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_iso(ms: i64) -> String {
    let seconds = ms.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        format_day(ms),
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_round_trip() {
        let ms = parse_iso_ms("2026-10-29T12:34:56.789Z").unwrap();
        assert_eq!(format_iso(ms), "2026-10-29T12:34:56Z");
        assert_eq!(ms % 1000, 789);
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn offsets_are_converted_to_utc() {
        assert_eq!(
            parse_iso_ms("2026-10-29T02:00:00+02:00"),
            parse_iso_ms("2026-10-29T00:00:00Z")
        );
    }

    #[test]
    fn leap_day_and_garbage() {
        assert_eq!(
            format_day(parse_date_ms("2024-02-29").unwrap()),
            "2024-02-29"
        );
        assert_eq!(parse_iso_ms("yesterday"), None);
        assert_eq!(parse_date_ms("2026-13-01"), None);
    }
}
