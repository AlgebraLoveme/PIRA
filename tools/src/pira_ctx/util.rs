use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const BROKEN_PIPE: &str = "__PIRA_CTX_BROKEN_PIPE__";
pub const MAX_DISPLAY_READ_BYTES: u64 = 64 * 1024;
pub const MAX_SEARCH_LINE_BYTES: u64 = 128 * 1024;
const DISPLAY_CLIP_BYTES: usize = 1200;

pub struct BoundedStdout {
    remaining: usize,
    truncated: bool,
}

impl BoundedStdout {
    pub fn new(maximum_bytes: usize) -> Self {
        Self {
            remaining: maximum_bytes,
            truncated: false,
        }
    }

    pub fn line(&mut self, text: &str) -> Result<(), String> {
        if self.truncated {
            return Ok(());
        }
        let clean = sanitize_terminal(text);
        let needed = clean.len().saturating_add(1);
        if needed <= self.remaining {
            stdout_line(&clean)?;
            self.remaining -= needed;
            return Ok(());
        }
        const MARKER: &str = "[pira_ctx output truncated by byte limit]";
        let marker_needed = MARKER.len() + 1;
        let available = self.remaining.saturating_sub(marker_needed + 1);
        if available > 0 {
            stdout_line(safe_prefix(&clean, available))?;
            self.remaining = self.remaining.saturating_sub(available + 1);
        }
        if marker_needed <= self.remaining {
            stdout_line(MARKER)?;
        }
        self.remaining = 0;
        self.truncated = true;
        Ok(())
    }
}

pub fn io_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::BrokenPipe {
        BROKEN_PIPE.to_string()
    } else {
        error.to_string()
    }
}

pub fn read_file_limited(path: &Path, maximum: u64, label: &str) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("open {label} {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {label} {}: {error}", path.display()))?;
    if bytes.len() as u64 > maximum {
        return Err(format!(
            "{label} {} exceeds the {maximum}-byte limit",
            path.display()
        ));
    }
    Ok(bytes)
}

pub fn stdout_line(text: &str) -> Result<(), String> {
    let mut output = io::stdout().lock();
    writeln!(output, "{}", sanitize_terminal(text)).map_err(io_error)
}

pub fn millis(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
}

pub fn status_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    125
}

pub fn argv_display(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shellish_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Return a display-safe copy of command arguments for persisted metadata and
/// context-facing output. The command still receives the original arguments.
pub fn redacted_argv(argv: &[String]) -> Vec<String> {
    let mut output = Vec::with_capacity(argv.len());
    let mut redact_next = false;
    for argument in argv {
        if redact_next {
            output.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }

        let trimmed = argument.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some((key, _)) = trimmed.split_once('=')
            && sensitive_name(key.trim_start_matches('-'))
        {
            output.push(format!("{key}=[REDACTED]"));
            continue;
        }
        if let Some((key, _)) = trimmed.split_once(':')
            && sensitive_name(key.trim_start_matches('-'))
        {
            output.push(format!("{key}: [REDACTED]"));
            continue;
        }
        if contains_url_credentials(&lower) || looks_like_bearer(&lower) || looks_like_token(&lower)
        {
            output.push("[REDACTED]".to_string());
            continue;
        }
        if sensitive_name(lower.trim_start_matches('-')) {
            output.push(argument.clone());
            redact_next = true;
        } else {
            output.push(argument.clone());
        }
    }
    output
}

pub fn redacted_argv_display(argv: &[String]) -> String {
    argv_display(&redacted_argv(argv))
}

fn sensitive_name(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase().replace(['-', '_'], "");
    matches!(
        normalized.as_str(),
        "authorization"
            | "authtoken"
            | "accesstoken"
            | "refreshtoken"
            | "bearer"
            | "token"
            | "secret"
            | "password"
            | "passwd"
            | "pwd"
            | "apikey"
            | "cookie"
            | "setcookie"
            | "signature"
            | "privatekey"
            | "clientsecret"
    ) || normalized.ends_with("authtoken")
        || normalized.ends_with("accesstoken")
        || normalized.ends_with("refreshtoken")
        || normalized.ends_with("apikey")
        || normalized.ends_with("password")
        || normalized.ends_with("clientsecret")
}

fn contains_url_credentials(value: &str) -> bool {
    value
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split('/').next())
        .is_some_and(|authority| authority.contains('@') && authority.contains(':'))
}

fn looks_like_bearer(value: &str) -> bool {
    value
        .strip_prefix("bearer ")
        .is_some_and(|token| !token.trim().is_empty())
}

fn looks_like_token(value: &str) -> bool {
    ["ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "sk-"]
        .iter()
        .any(|prefix| value.starts_with(prefix) && value.len() > prefix.len() + 8)
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    #[test]
    fn redacts_secret_flags_assignments_headers_urls_and_tokens() {
        let argv = vec![
            "curl".into(),
            "--token".into(),
            "flag-secret".into(),
            "API_KEY=env-secret".into(),
            "Authorization: Bearer header-secret".into(),
            "https://user:url-secret@example.test/path".into(),
            "ghp_1234567890abcdefghij".into(),
            "safe".into(),
        ];
        let rendered = redacted_argv_display(&argv);
        for secret in [
            "flag-secret",
            "env-secret",
            "header-secret",
            "url-secret",
            "ghp_1234567890abcdefghij",
        ] {
            assert!(!rendered.contains(secret));
        }
        assert!(rendered.contains("safe"));
        assert!(rendered.contains("[REDACTED]"));
    }
}

fn shellish_quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "_+-=./:".contains(character))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
pub fn unicode_contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

pub fn sanitize_terminal(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
                    chars.next();
                    let mut previous_escape = false;
                    for next in chars.by_ref() {
                        if next == '\u{7}' || (previous_escape && next == '\\') {
                            break;
                        }
                        previous_escape = next == '\u{1b}';
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if matches!(character, '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}') {
            // Carriage returns can rewrite terminal output; render them as line boundaries.
            output.push(' ');
        } else if matches!(
            character,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
        ) {
            // Remove invisible and bidirectional formatting that can visually
            // reorder or disguise untrusted program output.
        } else if character.is_control() && !matches!(character, '\n' | '\t') {
            // Suppress terminal controls while retaining readable whitespace.
        } else {
            output.push(character);
        }
    }
    output.trim_end_matches(['\n', '\r']).to_string()
}

pub fn clip_display(value: &str) -> String {
    if value.len() <= DISPLAY_CLIP_BYTES {
        return value.to_string();
    }
    let start = safe_prefix(value, 600);
    let end = safe_suffix(value, 300);
    let clipped = value.len().saturating_sub(start.len() + end.len());
    format!(
        "{start} … clipped {clipped} bytes (~{} words) … {end}",
        clipped / 6
    )
}

pub fn clip_match_display(value: &str, match_start: usize, match_end: usize) -> String {
    if value.len() <= DISPLAY_CLIP_BYTES {
        return value.to_string();
    }
    let match_start = match_start.min(value.len());
    let match_end = match_end.max(match_start).min(value.len());
    let mut start = match_start.saturating_sub(400);
    while !value.is_char_boundary(start) {
        start = start.saturating_sub(1);
    }
    let mut end = match_end.saturating_add(500).min(value.len());
    while end < value.len() && !value.is_char_boundary(end) {
        end += 1;
    }
    if end.saturating_sub(start) > 1_000 {
        end = start.saturating_add(1_000).min(value.len());
        while !value.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
    }
    let prefix = if start > 0 {
        format!("… {start} bytes omitted … ")
    } else {
        String::new()
    };
    let suffix = if end < value.len() {
        format!(" … {} bytes omitted …", value.len() - end)
    } else {
        String::new()
    };
    format!("{prefix}{}{suffix}", &value[start..end])
}

pub fn single_line_clip(value: &str, maximum_bytes: usize) -> String {
    let clean = sanitize_terminal(value)
        .chars()
        .map(|character| {
            if matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.len() <= maximum_bytes {
        clean
    } else {
        format!("{}…", safe_prefix(&clean, maximum_bytes.saturating_sub(3)))
    }
}

pub fn xml_field(value: &str, maximum_bytes: usize) -> String {
    single_line_clip(value, maximum_bytes)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn safe_prefix(value: &str, bytes: usize) -> &str {
    let mut end = bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn safe_suffix(value: &str, bytes: usize) -> &str {
    let mut start = value.len().saturating_sub(bytes);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_osc_and_carriage_return() {
        let input = "\x1b[31mred\x1b[0m \x1b]8;;https://example.test\x07label\x1b]8;;\x07\rnext";
        assert_eq!(sanitize_terminal(input), "red label next");
    }

    #[test]
    fn unicode_search_is_case_insensitive() {
        assert!(unicode_contains_ci("CAFÉ diagnostic", "café"));
    }

    #[test]
    fn long_line_excerpt_keeps_the_match_local() {
        let value = format!("{}NEEDLE{}", "a".repeat(5_000), "z".repeat(5_000));
        let excerpt = clip_match_display(&value, 5_000, 5_006);
        assert!(excerpt.contains("NEEDLE"));
        assert!(excerpt.len() < 1_100);
        assert!(excerpt.contains("bytes omitted"));
    }
}
