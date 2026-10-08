use sha2::{Digest, Sha256};
use std::fs;
#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const BROKEN_PIPE: &str = "__PIRA_DEC_BROKEN_PIPE__";
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

pub fn parse_time_bound(value: &str, now_ms: u64) -> Result<jiff::Timestamp, String> {
    if value == "now" {
        return jiff::Timestamp::from_millisecond(now_ms as i64).map_err(|e| e.to_string());
    }
    if let Some((amount, multiplier)) = parse_age(value)? {
        let age_ms = amount
            .checked_mul(multiplier)
            .ok_or_else(|| format!("search time {value:?} is too large"))?;
        return jiff::Timestamp::from_millisecond(now_ms.saturating_sub(age_ms) as i64)
            .map_err(|e| e.to_string());
    }
    let timestamp = value.parse::<jiff::Timestamp>().map_err(|_| {
        format!(
            "search time {value:?} must be RFC 3339, `now`, or a relative age such as 30m, 24h, or 7d"
        )
    })?;
    if timestamp.as_nanosecond() < 0 {
        return Err("search timestamps before 1970 are unsupported".into());
    }
    let millis = timestamp.as_millisecond() as u64;
    validate_timestamp(millis)?;
    Ok(timestamp)
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

pub fn is_bidi_control(character: char) -> bool {
    matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

pub fn escape_diagnostic(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_control() || is_bidi_control(character) {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    output
}

pub fn single_line_clip(value: &str, maximum: usize) -> String {
    let mut normalized = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace() || character.is_control() {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        if is_bidi_control(character) {
            normalized.extend(character.escape_default());
        } else {
            normalized.push(character);
        }
    }
    let mut characters = normalized.chars();
    let mut output: String = characters.by_ref().take(maximum).collect();
    if characters.next().is_some() {
        output.push('…');
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

/// Fail before mutation when the platform lacks the implemented storage contract.
pub fn require_write_support() -> Result<(), String> {
    if cfg!(any(target_os = "macos", target_os = "linux", windows)) {
        Ok(())
    } else {
        Err("private writes are supported only on macOS, Linux and Windows; this platform lacks implemented privacy guarantees".into())
    }
}

/// macOS ACL grants are independent of mode bits; preserve non-granting ACLs.
#[cfg(target_os = "macos")]
pub fn check_private_file(file: &std::fs::File) -> Result<(), String> {
    use std::ffi::{c_int, c_void};
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn acl_get_fd(fd: c_int) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
        fn acl_get_permset_mask_np(entry: *mut c_void, mask: *mut u64) -> c_int;
        fn acl_free(acl: *mut c_void) -> c_int;
    }
    // Darwin sys/acl.h: ACL_FIRST_ENTRY = 0. Darwin returns 0 for an entry,
    // and -1/EINVAL when the ACL contains no further entries (unlike Linux).
    let acl = unsafe { acl_get_fd(file.as_raw_fd()) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        // Darwin reports ENOENT for no extended ACL. This is an open descriptor,
        // not a pathname lookup; all other retrieval failures remain errors.
        return if error.raw_os_error() == Some(2) {
            Ok(())
        } else {
            Err(format!("inspect private-file ACL: {error}"))
        };
    }
    let result = (|| {
        let mut selector = 0; // ACL_FIRST_ENTRY; subsequent entries use ACL_NEXT_ENTRY (-1).
        loop {
            let mut entry = std::ptr::null_mut();
            if unsafe { acl_get_entry(acl, selector, &mut entry) } != 0 {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(22) {
                    Ok(())
                } else {
                    Err(format!("inspect private-file ACL entry: {error}"))
                };
            }
            selector = -1;
            let mut tag = 0;
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 {
                return Err(format!(
                    "inspect private-file ACL tag: {}",
                    io::Error::last_os_error()
                ));
            }
            match tag {
                2 => continue, // ACL_EXTENDED_DENY can only restrict access.
                1 => {
                    // ACL_EXTENDED_ALLOW with no permissions grants nothing.
                    let mut permissions = 0;
                    if unsafe { acl_get_permset_mask_np(entry, &mut permissions) } != 0 {
                        return Err(format!(
                            "inspect private-file ACL permissions: {}",
                            io::Error::last_os_error()
                        ));
                    }
                    if permissions == 0 {
                        continue;
                    }
                    // PIRA: do not approximate principal/group membership or
                    // ordered deny/allow evaluation. Preserve the ACL and fail.
                    return Err("refusing permission-granting extended ACL on private decision storage/output; use a location without ACL grants (deny-only ACLs are supported; ACLs are never removed automatically)".into());
                }
                _ => {
                    return Err(
                        "unsupported extended ACL entry type on private decision storage/output"
                            .into(),
                    );
                }
            }
        }
    })();
    unsafe { acl_free(acl) };
    result
}

#[cfg(windows)]
pub use crate::windows::check_private_file;

#[cfg(not(any(target_os = "macos", windows)))]
pub fn check_private_file(_file: &std::fs::File) -> Result<(), String> {
    // Linux POSIX ACL grants are bounded by the 0700/0600 group-class mask.
    require_write_support()
}

pub fn write_private_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    require_write_support()?;
    #[cfg(windows)]
    let opened = crate::windows::create_new_file(path);
    #[cfg(not(windows))]
    let opened = {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)
    };
    let mut file = opened.map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            format!("output already exists: {}", path.display())
        } else {
            format!("create output {}: {error}", path.display())
        }
    })?;
    let result = check_private_file(&file).and_then(|()| {
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())
    });
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        return Err(format!("write output {}: {error}", path.display()));
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
    fn clipping_counts_rendered_scalars_and_only_marks_omissions() {
        for (input, maximum, expected) in [
            ("abc", 3, "abc"),
            ("abc \n", 3, "abc"),
            ("a b c", 3, "a b…"),
            ("前後", 2, "前後"),
            ("前後次", 2, "前後…"),
            ("", 0, ""),
            ("a", 0, "…"),
            (" \t", 0, ""),
            ("\u{202e}", 8, "\\u{202e}"),
            ("\u{202e}x", 8, "\\u{202e}…"),
        ] {
            assert_eq!(single_line_clip(input, maximum), expected);
        }
        assert_eq!(
            single_line_clip(&"\u{202e}".repeat(200), 200)
                .chars()
                .count(),
            201
        );
    }

    #[test]
    fn concise_rows_escape_all_bidi_controls() {
        for character in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
            '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            assert_eq!(
                single_line_clip(&character.to_string(), 100),
                character.escape_default().to_string()
            );
        }
        assert_eq!(single_line_clip("مرحبا 前", 100), "مرحبا 前");
    }

    #[test]
    fn parses_absolute_and_relative_search_times() {
        let now = 10 * 3_600_000;
        assert_eq!(
            parse_time_bound("2h", now).unwrap().as_millisecond(),
            8 * 3_600_000
        );
        assert_eq!(
            parse_time_bound("now", now).unwrap().as_millisecond(),
            now as i64
        );

        let timestamp = parse_time_bound("2026-07-21T10:00:00+08:00", now).unwrap();
        assert_eq!(
            format_rfc3339(timestamp.as_millisecond() as u64).unwrap(),
            "2026-07-21T02:00:00.000Z"
        );
    }

    #[test]
    fn rejects_invalid_search_time() {
        let error = parse_time_bound("yesterday", 1_000).unwrap_err();
        assert!(error.contains("must be RFC 3339"));
    }

    #[test]
    fn rejects_submillisecond_pre_epoch_bound() {
        assert!(parse_time_bound("1969-12-31T23:59:59.999999999Z", 0).is_err());
        assert_eq!(
            parse_time_bound("1970-01-01T00:00:00Z", 0)
                .unwrap()
                .as_nanosecond(),
            0
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    #[test]
    fn private_output_is_new_and_never_overwritten() {
        let path = std::env::temp_dir().join(format!("pira-dec-output-{}", nonce_hex()));
        write_private_new(&path, b"first").unwrap();
        let error = write_private_new(&path, b"second").unwrap_err();
        assert!(error.contains("already exists"));
        assert_eq!(fs::read(&path).unwrap(), b"first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_file(path).unwrap();
    }
}
