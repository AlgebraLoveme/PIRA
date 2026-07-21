use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const BROKEN_PIPE: &str = "__PIRA_DECISION_BROKEN_PIPE__";
pub const MAX_TIMESTAMP_MS: u64 = 253_402_300_799_999;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct UtcParts {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
}

pub fn now_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?
        .as_millis();
    let millis = u64::try_from(millis).map_err(|_| "system clock is out of range".to_string())?;
    validate_timestamp(millis)?;
    Ok(millis)
}

pub fn parse_time_bound(value: &str, now_ms: u64) -> Result<u64, String> {
    if value == "now" {
        return Ok(now_ms);
    }
    if let Some((amount, multiplier)) = parse_age(value)? {
        let age_ms = amount
            .checked_mul(multiplier)
            .ok_or_else(|| format!("search time {value:?} is too large"))?;
        return Ok(now_ms.saturating_sub(age_ms));
    }
    let timestamp = value.parse::<jiff::Timestamp>().map_err(|_| {
        format!(
            "search time {value:?} must be RFC 3339, `now`, or a relative age such as 30m, 24h, or 7d"
        )
    })?;
    let millis = u64::try_from(timestamp.as_millisecond())
        .map_err(|_| "search timestamps before 1970 are unsupported".to_string())?;
    validate_timestamp(millis)?;
    Ok(millis)
}

fn parse_age(value: &str) -> Result<Option<(u64, u64)>, String> {
    let Some((amount, unit)) = value.split_at_checked(value.len().saturating_sub(1)) else {
        return Ok(None);
    };
    let multiplier = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "w" => 604_800_000,
        _ => return Ok(None),
    };
    if amount.is_empty() || !amount.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let amount = amount
        .parse()
        .map_err(|_| format!("search time {value:?} is too large"))?;
    Ok(Some((amount, multiplier)))
}

pub fn validate_timestamp(timestamp_ms: u64) -> Result<(), String> {
    if timestamp_ms > MAX_TIMESTAMP_MS {
        return Err("timestamp is outside the supported UTC range".into());
    }
    Ok(())
}

pub fn decision_id(timestamp_ms: u64) -> Result<String, String> {
    Ok(format!(
        "D-{}-{}",
        format_id_timestamp(timestamp_ms)?,
        nonce_hex()
    ))
}

pub fn nonce_hex() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha256::new();
    hasher.update(b"pira-decision-nonce-v1\0");
    hasher.update(now.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(counter.to_le_bytes());
    let digest = hasher.finalize();
    hex(&digest[..8])
}

pub fn format_id_timestamp(timestamp_ms: u64) -> Result<String, String> {
    let parts = utc_parts(timestamp_ms)?;
    Ok(format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second
    ))
}

pub fn format_rfc3339(timestamp_ms: u64) -> Result<String, String> {
    let parts = utc_parts(timestamp_ms)?;
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second, parts.millis
    ))
}

fn utc_parts(timestamp_ms: u64) -> Result<UtcParts, String> {
    validate_timestamp(timestamp_ms)?;
    let seconds = timestamp_ms / 1_000;
    let days = i64::try_from(seconds / 86_400).map_err(|_| "timestamp is out of range")?;
    let seconds_of_day = (seconds % 86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    Ok(UtcParts {
        year,
        month,
        day,
        hour: seconds_of_day / 3_600,
        minute: (seconds_of_day % 3_600) / 60,
        second: seconds_of_day % 60,
        millis: (timestamp_ms % 1_000) as u32,
    })
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (
        (year + i64::from(month <= 2)) as i32,
        month as u32,
        day as u32,
    )
}

pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn single_line_clip(value: &str, maximum: usize) -> String {
    let mut output = String::new();
    let mut pending_space = false;
    let mut count = 0;
    for character in value.chars() {
        if character.is_whitespace() || character.is_control() {
            pending_space = !output.is_empty();
            continue;
        }
        if pending_space {
            output.push(' ');
            pending_space = false;
        }
        output.push(character);
        count += 1;
        if count >= maximum {
            output.push('…');
            break;
        }
    }
    output
}

pub fn stdout_line(value: &str) -> Result<(), String> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    writeln!(lock, "{value}").map_err(output_error)
}

pub fn stdout_text(value: &str) -> Result<(), String> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    lock.write_all(value.as_bytes()).map_err(output_error)?;
    if !value.ends_with('\n') {
        lock.write_all(b"\n").map_err(output_error)?;
    }
    Ok(())
}

fn output_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::BrokenPipe {
        BROKEN_PIPE.into()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_fixed_utc_timestamps() {
        assert_eq!(
            format_id_timestamp(1_784_269_812_000).unwrap(),
            "20260717-063012"
        );
        assert_eq!(
            format_rfc3339(1_784_269_812_345).unwrap(),
            "2026-07-17T06:30:12.345Z"
        );
    }

    #[test]
    fn clips_multiline_output() {
        assert_eq!(
            single_line_clip("alpha\n beta\t gamma", 100),
            "alpha beta gamma"
        );
    }

    #[test]
    fn parses_absolute_and_relative_search_times() {
        let now = 10 * 3_600_000;
        assert_eq!(parse_time_bound("2h", now).unwrap(), 8 * 3_600_000);
        assert_eq!(parse_time_bound("now", now).unwrap(), now);

        let timestamp = parse_time_bound("2026-07-21T10:00:00+08:00", now).unwrap();
        assert_eq!(
            format_rfc3339(timestamp).unwrap(),
            "2026-07-21T02:00:00.000Z"
        );
    }

    #[test]
    fn rejects_invalid_search_time() {
        let error = parse_time_bound("yesterday", 1_000).unwrap_err();
        assert!(error.contains("must be RFC 3339"));
    }
}
