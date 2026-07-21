use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

use crate::model::{CaptureResult, LineMeta, StreamKind, line_flag};
use crate::util;

const MAX_JSON_SYNOPSIS_INPUT_BYTES: u64 = 512 * 1024;
const MAX_JSON_PROBE_BYTES: u64 = 256;
const MAX_JSON_INLINE_BYTES: usize = 768;
const MAX_JSON_INLINE_ITEMS: usize = 8;
const MAX_JSON_DEPTH: usize = 6;
const MAX_JSON_PATH_BYTES: usize = 256;

pub fn json_synopsis(
    capture: &CaptureResult,
    maximum_lines: usize,
) -> Result<Option<Vec<String>>, String> {
    if maximum_lines == 0
        || capture.stdout.length == 0
        || capture.stdout.length > MAX_JSON_SYNOPSIS_INPUT_BYTES
        || capture.stderr.length != 0
        || capture.stdout.binary
        || capture.stdout.non_utf8
        || capture.retention_truncated
    {
        return Ok(None);
    }
    let mut readers = capture.readers()?;
    let prefix = readers.read_section_prefix(StreamKind::Stdout, MAX_JSON_PROBE_BYTES)?;
    let prefix = prefix
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .unwrap_or(&prefix)
        .trim_ascii_start();
    if !matches!(prefix.first(), Some(b'{') | Some(b'[')) {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(capture.stdout.length as usize);
    readers.copy_section(StreamKind::Stdout, &mut bytes)?;
    Ok(json_synopsis_bytes(&bytes, maximum_lines))
}

fn json_synopsis_bytes(bytes: &[u8], maximum_lines: usize) -> Option<Vec<String>> {
    let bytes = bytes
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .unwrap_or(bytes)
        .trim_ascii();
    if !matches!(bytes.first(), Some(b'{') | Some(b'[')) {
        return None;
    }
    let root: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if !matches!(
        root,
        serde_json::Value::Object(_) | serde_json::Value::Array(_)
    ) {
        return None;
    }

    let mut pending = VecDeque::new();
    match &root {
        serde_json::Value::Object(values) => {
            pending.extend(
                values
                    .iter()
                    .map(|(key, value)| (json_path("$", key), value, 1_usize)),
            );
        }
        serde_json::Value::Array(_) => pending.push_back(("$".into(), &root, 0)),
        _ => unreachable!(),
    }

    let data_limit = maximum_lines.saturating_sub(1).max(1);
    let mut lines = Vec::with_capacity(maximum_lines.min(16));
    while lines.len() < data_limit {
        let Some((path, value, depth)) = pending.pop_front() else {
            break;
        };
        if let Some(compact) = compact_json(value, 0) {
            lines.push(format!("{path} = {compact}"));
            continue;
        }
        match value {
            serde_json::Value::Object(values) if depth < MAX_JSON_DEPTH => {
                pending.extend(
                    values
                        .iter()
                        .map(|(key, value)| (json_path(&path, key), value, depth + 1)),
                );
            }
            serde_json::Value::Object(values) => {
                lines.push(format!("{path} = object[{}]", values.len()));
            }
            serde_json::Value::Array(values) => {
                lines.push(format!("{path} = array[{}]", values.len()));
            }
            serde_json::Value::String(value) => {
                lines.push(format!("{path} = string[{} B]", value.len()));
            }
            _ => lines.push(format!("{path} = {value}")),
        }
    }
    if !pending.is_empty() && lines.len() < maximum_lines {
        lines.push("... additional JSON fields omitted".into());
    }
    (!lines.is_empty()).then_some(lines)
}

fn compact_json(value: &serde_json::Value, depth: usize) -> Option<String> {
    if depth > 2 {
        return None;
    }
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
        serde_json::Value::String(value) if value.len() <= 160 => {}
        serde_json::Value::Array(values) if values.len() <= MAX_JSON_INLINE_ITEMS => {
            for value in values {
                compact_json(value, depth + 1)?;
            }
        }
        serde_json::Value::Object(values) if values.len() <= MAX_JSON_INLINE_ITEMS => {
            for value in values.values() {
                compact_json(value, depth + 1)?;
            }
        }
        _ => return None,
    }
    let rendered = serde_json::to_string(value).ok()?;
    (rendered.len() <= MAX_JSON_INLINE_BYTES).then_some(rendered)
}

fn json_path(base: &str, key: &str) -> String {
    let next = if is_json_identifier(key) {
        format!("{base}.{key}")
    } else {
        let key = raw_clip(key, 128);
        format!(
            "{base}[{}]",
            serde_json::to_string(key).unwrap_or_else(|_| "\"?\"".into())
        )
    };
    raw_clip(&next, MAX_JSON_PATH_BYTES).into()
}

fn is_json_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}

fn raw_clip(value: &str, maximum_bytes: usize) -> &str {
    let mut end = value.len().min(maximum_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub fn score_timeline(capture: &mut CaptureResult, keywords: &[String]) -> Result<(), String> {
    let total = capture.total_lines;
    let mut readers = capture.readers()?;
    for line in &mut capture.timeline {
        let clean = readers.read_display_line(line)?;
        let (score, flags) = score_line(
            &clean,
            line.stream,
            line.line,
            total,
            capture.exit_code,
            keywords,
        );
        line.score = score;
        line.flags = flags;
    }
    for index in 0..capture.timeline.len() {
        let base_score = capture.timeline[index].score;
        if base_score < 100 {
            continue;
        }
        for nearby in [index.checked_sub(1), index.checked_add(1)] {
            if let Some(nearby) = nearby.filter(|&value| value < capture.timeline.len())
                && capture.timeline[nearby]
                    .line
                    .abs_diff(capture.timeline[index].line)
                    == 1
                && capture.timeline[nearby].score < base_score
            {
                capture.timeline[nearby].score += 10;
            }
        }
    }
    Ok(())
}

pub fn select_important(lines: &[LineMeta], maximum: usize) -> Vec<usize> {
    if maximum == 0 {
        return Vec::new();
    }
    let mut selected = Vec::new();
    let mut selected_set = HashSet::new();
    let has_failure = lines.iter().any(|line| line.has(line_flag::FAILURE));
    let failure_budget = maximum.div_ceil(2);
    for index in (0..lines.len())
        .rev()
        .filter(|&index| lines[index].has(line_flag::FAILURE))
    {
        for nearby in [index.checked_sub(1), Some(index), index.checked_add(1)] {
            if selected.len() >= failure_budget {
                break;
            }
            if let Some(nearby) = nearby.filter(|&value| value < lines.len())
                && selected_set.insert(nearby)
            {
                selected.push(nearby);
            }
        }
        if selected.len() >= failure_budget {
            break;
        }
    }
    if !has_failure {
        let latest_outcome = lines.iter().rposition(|line| line.has(line_flag::SUCCESS));
        let latest_check = lines
            .iter()
            .rposition(|line| line.has(line_flag::SUCCESSFUL_CHECK));
        if let Some(index) = latest_outcome.or(latest_check) {
            selected_set.insert(index);
            selected.push(index);
        }
    }
    let boundary_budget = maximum
        .div_ceil(3)
        .min(maximum.saturating_sub(selected.len()));
    if boundary_budget > 0 {
        let mut boundaries_added = 0;
        for index in lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| line.has(line_flag::COMMAND_BOUNDARY).then_some(index))
        {
            if selected_set.insert(index) {
                selected.push(index);
                boundaries_added += 1;
                if boundaries_added == boundary_budget {
                    break;
                }
            }
        }
    }
    let mut templates: HashMap<(StreamKind, u32), usize> = HashMap::new();
    let mut successful_checks = selected
        .iter()
        .filter(|&&index| lines[index].has(line_flag::SUCCESSFUL_CHECK))
        .count();
    while selected.len() < maximum {
        let mut best = None;
        for (index, line) in lines.iter().enumerate() {
            if selected_set.contains(&index) || (has_failure && line.has(line_flag::SUCCESS)) {
                continue;
            }
            if line.has(line_flag::SUCCESSFUL_CHECK) && successful_checks >= 2 {
                continue;
            }
            let template = reason_template(line);
            let limit = if line.has(line_flag::ERROR) { 3 } else { 2 };
            if templates.get(&template).copied().unwrap_or(0) >= limit {
                continue;
            }
            if best.is_none_or(|current: usize| {
                (line.score, Reverse(line.line))
                    > (lines[current].score, Reverse(lines[current].line))
            }) {
                best = Some(index);
            }
        }
        let Some(index) = best else { break };
        selected_set.insert(index);
        if lines[index].has(line_flag::SUCCESSFUL_CHECK) {
            successful_checks += 1;
        }
        *templates.entry(reason_template(&lines[index])).or_default() += 1;
        selected.push(index);
    }
    selected.sort_by_key(|&index| lines[index].line);
    selected
}

pub fn has_high_confidence_signal(lines: &[LineMeta]) -> bool {
    lines
        .iter()
        .any(|line| line.has(line_flag::HIGH_CONFIDENCE))
}

pub fn representative_groups(
    capture: &CaptureResult,
    maximum: usize,
) -> Result<Vec<(usize, String)>, String> {
    const MAX_GROUP_SAMPLES: usize = 50_000;
    static NUMBER: OnceLock<regex::Regex> = OnceLock::new();
    static TIMESTAMP: OnceLock<regex::Regex> = OnceLock::new();
    static IDENTIFIER: OnceLock<regex::Regex> = OnceLock::new();
    let number = NUMBER.get_or_init(|| regex::Regex::new(r"\b\d+(?:\.\d+)?\b").unwrap());
    let timestamp = TIMESTAMP.get_or_init(|| {
        regex::Regex::new(
            r"\b\d{2,4}[-/:T]\d{1,2}[-/:T]\d{1,2}(?:[T ]\d{1,2}:\d{2}:\d{2}(?:\.\d+)?)?\b",
        )
        .unwrap()
    });
    let identifier = IDENTIFIER.get_or_init(|| regex::Regex::new(r"\b[0-9a-fA-F]{8,}\b").unwrap());
    let mut reader = capture.readers()?;
    let mut groups: HashMap<String, (usize, String)> = HashMap::new();
    let stride = capture.timeline.len().div_ceil(MAX_GROUP_SAMPLES).max(1);
    for line in capture.timeline.iter().step_by(stride) {
        let text = reader.read_display_line(line)?;
        let trimmed = text.trim();
        if trimmed.is_empty() || is_summary_boilerplate(trimmed) {
            continue;
        }
        let normalized = identifier
            .replace_all(&timestamp.replace_all(trimmed, "<TIME>"), "<ID>")
            .into_owned();
        let normalized = number.replace_all(&normalized, "<N>").into_owned();
        let key = normalized.chars().take(64).collect::<String>();
        let entry = groups
            .entry(key)
            .or_insert_with(|| (0, trimmed.chars().take(240).collect()));
        entry.0 += 1;
    }
    let mut values: Vec<_> = groups.into_values().collect();
    values.retain(|(count, _)| *count > 1);
    values.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    values.truncate(maximum);
    Ok(values)
}

fn is_summary_boilerplate(trimmed: &str) -> bool {
    if trimmed.starts_with("--- begin ")
        || trimmed.starts_with("--- end ")
        || trimmed == "PROGRAM data:"
        || trimmed == "Common PROGRAM line forms:"
        || trimmed.starts_with("# pira_")
    {
        return true;
    }
    if trimmed.eq_ignore_ascii_case("ok")
        || trimmed
            .chars()
            .all(|character| character.is_ascii_punctuation() || character.is_whitespace())
        || is_pretty_json_scaffolding(trimmed)
    {
        return true;
    }
    let without_marker = trimmed
        .strip_prefix('>')
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let Some((left, right)) = without_marker.split_once('|') else {
        return false;
    };
    right.trim().is_empty()
        && !left.trim().is_empty()
        && left.trim().bytes().all(|byte| byte.is_ascii_digit())
}

fn is_pretty_json_scaffolding(trimmed: &str) -> bool {
    let Some((key, value)) = trimmed.split_once(':') else {
        return false;
    };
    if !(key.starts_with('"') && key.ends_with('"')) {
        return false;
    }
    let value = value.trim().trim_end_matches(',').trim();
    matches!(value, "{" | "[" | "}" | "]")
        || value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("null")
        || value.parse::<f64>().is_ok()
}

fn reason_template(line: &LineMeta) -> (StreamKind, u32) {
    (line.stream, line.flags & line_flag::STABLE_TEMPLATE)
}

fn top_indices<'a>(
    lines: impl Iterator<Item = (usize, &'a LineMeta)>,
    maximum: usize,
) -> Vec<usize> {
    let mut heap = BinaryHeap::with_capacity(maximum.saturating_add(1));
    for (index, line) in lines {
        heap.push((Reverse(line.score), line.line, index));
        if heap.len() > maximum {
            heap.pop();
        }
    }
    let mut selected = heap.into_vec();
    selected.sort_by(|a, b| b.0.0.cmp(&a.0.0).then_with(|| a.1.cmp(&b.1)));
    selected.into_iter().map(|(_, _, index)| index).collect()
}

pub fn detected_paths(capture: &CaptureResult) -> Result<Vec<String>, String> {
    let mut readers = capture.readers()?;
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for index in top_indices(capture.timeline.iter().enumerate(), 100) {
        let line = &capture.timeline[index];
        let clean = readers.read_display_line(line)?;
        for token in clean.split_whitespace() {
            let candidate = token.trim_matches(|character: char| {
                matches!(character, ',' | ';' | ')' | '(' | '[' | ']' | '"' | '\'')
            });
            if is_path_like(candidate) && seen.insert(candidate.to_string()) {
                output.push(candidate.to_string());
                if output.len() == 20 {
                    return Ok(output);
                }
            }
        }
    }
    Ok(output)
}

pub fn suggested_keywords(
    capture: &CaptureResult,
    command: &[String],
    _user_keywords: &[String],
) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    let diff_context = is_diff_context(command);
    let eligible = capture.timeline.iter().enumerate().filter(|(_, line)| {
        (diff_context && line.has(line_flag::DIFF))
            || (capture.exit_code != 0
                && line.has(
                    line_flag::FAILURE
                        | line_flag::ERROR
                        | line_flag::FAILED_TEST
                        | line_flag::WARNING
                        | line_flag::NUMERIC_ANOMALY,
                ))
    });
    let mut readers = capture.readers()?;
    for index in top_indices(eligible, 24) {
        let line = &capture.timeline[index];
        let clean = readers.read_display_line(line)?;
        if is_serialized_payload_line(&clean) {
            continue;
        }
        let is_diff = line.has(line_flag::DIFF);
        let candidates = if is_diff && diff_context {
            diff_search_term(&clean).into_iter().collect()
        } else if capture.exit_code != 0 && is_diagnostic_evidence(&clean) {
            structured_search_terms(&clean)
        } else {
            Vec::new()
        };
        for candidate in candidates {
            let lower = candidate.to_lowercase();
            if seen.insert(lower) {
                output.push(candidate);
                if output.len() == 5 {
                    return Ok(output);
                }
            }
        }
    }
    Ok(output)
}

fn structured_search_terms(clean: &str) -> Vec<String> {
    let trimmed = clean.trim();
    let lower = trimmed.to_lowercase();
    let mut output = Vec::new();
    if lower.starts_with("not ok ")
        && let Some((_, description)) = trimmed.split_once(" - ")
    {
        let phrase = description.split(':').next().unwrap_or(description).trim();
        if !phrase.is_empty() {
            output.push(util::single_line_clip(phrase, 80));
        }
    }
    if lower.starts_with("failed ")
        && let Some(identifier) = trimmed.split_whitespace().nth(1)
    {
        if trimmed.contains(" - ") {
            output.push(identifier.trim_end_matches(':').to_string());
        } else {
            let phrase = trimmed
                .split(':')
                .next()
                .unwrap_or(trimmed)
                .split(['\'', '"'])
                .next()
                .unwrap_or(trimmed)
                .trim();
            output.push(util::single_line_clip(phrase, 80));
        }
    }
    if lower.starts_with("diff in ") {
        let location = trimmed[8..].trim_end_matches(':');
        let path = location.split(':').next().unwrap_or(location);
        if let Some(name) = std::path::Path::new(path)
            .file_name()
            .and_then(|v| v.to_str())
        {
            output.push(name.to_string());
        }
    }
    if lower.starts_with("error: could not compile") {
        output.push("could not compile".into());
    }
    if lower.contains("command not found") {
        output.push("command not found".into());
    }
    if lower.contains("permission denied") {
        output.push("permission denied".into());
    }
    if lower.starts_with("panic:") {
        output.push(util::single_line_clip(trimmed, 80));
    }
    for found in diagnostic_code_re().find_iter(trimmed) {
        output.push(found.as_str().to_string());
    }
    for found in lint_re().find_iter(trimmed) {
        output.push(found.as_str().to_string());
    }
    for found in exception_re().find_iter(trimmed) {
        if has_exception_punctuation(trimmed, found.start(), found.end()) {
            output.push(found.as_str().to_string());
        }
    }
    for found in path_re().find_iter(trimmed) {
        if !is_dependency_path(found.as_str()) {
            output.push(stable_path_term(found.as_str()));
        }
    }
    for found in paren_location_re().find_iter(trimmed) {
        if !is_dependency_path(found.as_str()) {
            output.push(stable_path_term(found.as_str()));
        }
    }
    output
}

fn diagnostic_code_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\b(?:E\d{4}|TS\d+|CS\d+|ERR_[A-Z0-9_]+|[A-Z][A-Z0-9_]{1,15}\d{3,6})\b")
            .unwrap()
    })
}

fn lint_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\b[a-z][a-z0-9_-]*::[a-z][a-z0-9_-]+\b").unwrap())
}

fn exception_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?:(?:[A-Za-z_][A-Za-z0-9_]*[.:])+(?:[A-Za-z_][A-Za-z0-9_]*(?:Error|Exception)|Error|Exception)|[A-Za-z_][A-Za-z0-9_]*(?:Error|Exception))\b",
        )
        .unwrap()
    })
}

fn path_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?:[A-Za-z]:[\\/])?(?:[A-Za-z0-9_.@+-]+[\\/])*[A-Za-z0-9_.@+-]+\.[A-Za-z0-9_+-]+(?::\d+(?::\d+)?)?\b",
        )
        .unwrap()
    })
}

fn paren_location_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\b[A-Za-z0-9_.@+-]+\.[A-Za-z0-9_+-]+\(\d+(?:,\d+)?\)").unwrap()
    })
}

fn is_diagnostic_evidence(clean: &str) -> bool {
    let trimmed = clean.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("not ok ")
        || lower.starts_with("failed ")
        || lower.starts_with("error:")
        || lower.starts_with("error[")
        || lower.starts_with("error ")
        || lower.starts_with("fatal:")
        || lower.starts_with("panic:")
        || lower.starts_with("traceback ")
        || lower.contains("test result: failed")
        || lower.contains("command not found")
        || lower.contains("permission denied")
        || lower.contains(": fatal error:")
        || lower.contains(": error ")
    {
        return true;
    }
    diagnostic_code_re().is_match(trimmed) && (lower.contains("error") || lower.contains("failed"))
        || has_runtime_exception(trimmed)
}

fn has_runtime_exception(value: &str) -> bool {
    exception_re()
        .find_iter(value)
        .any(|found| has_exception_punctuation(value, found.start(), found.end()))
}

fn has_exception_punctuation(value: &str, start: usize, end: usize) -> bool {
    let trimmed = value.trim();
    if start == value.find(trimmed).unwrap_or(0) && end == start + trimmed.len() {
        return true;
    }
    let before = value[..start].trim_end();
    let after = value[end..].trim_start();
    after.starts_with(':') || (before.ends_with('(') && after.starts_with(')'))
}

fn is_serialized_payload_line(value: &str) -> bool {
    let trimmed = value.trim();
    (matches!(trimmed.as_bytes().first(), Some(b'{') | Some(b'['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok())
        || (trimmed.contains("\\n")
            && trimmed.matches('"').count() >= 4
            && (trimmed.contains("\"output\"") || trimmed.contains("\"content\"")))
}

fn is_diff_context(command: &[String]) -> bool {
    let executable = command
        .first()
        .and_then(|value| std::path::Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let direct = matches!(executable.as_str(), "diff" | "colordiff")
        || (matches!(executable.as_str(), "git" | "hg" | "svn")
            && command.iter().skip(1).any(|argument| {
                matches!(
                    argument.as_str(),
                    "diff" | "show" | "format-patch" | "whatchanged"
                ) || argument == "--patch"
                    || argument == "-p"
            }));
    let shell = matches!(
        executable.as_str(),
        "sh" | "bash" | "zsh" | "fish" | "cmd" | "cmd.exe" | "powershell" | "pwsh"
    );
    let nested = shell
        && command.iter().skip(1).any(|argument| {
            let lower = argument.to_ascii_lowercase();
            ["git diff", "git show", "hg diff", "svn diff"]
                .iter()
                .any(|needle| lower.contains(needle))
        });
    direct || nested
}

fn diff_search_term(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let remainder = trimmed.strip_prefix("diff --git ")?;
    let words = split_quoted_words(remainder);
    let path = if let Some(index) = remainder.rfind(" b/") {
        &remainder[index + 1..]
    } else {
        words.get(1).or_else(|| words.first())?.as_str()
    };
    let normalized = path.replace('\\', "/");
    let normalized = normalized.strip_prefix("b/").unwrap_or(&normalized);
    if normalized == "/dev/null" {
        return None;
    }
    normalized
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn split_quoted_words(value: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' && quoted {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                output.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

fn stable_path_term(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let suffix_start = normalized
        .char_indices()
        .rev()
        .filter(|(_, character)| *character == '/')
        .nth(2)
        .map_or(0, |(index, _)| index + 1);
    normalized[suffix_start..].to_string()
}

fn score_line(
    clean: &str,
    stream: StreamKind,
    line_number: usize,
    total: usize,
    exit_code: i32,
    keywords: &[String],
) -> (i64, u32) {
    if is_rendering_scaffolding(clean.trim()) {
        return (-100, 0);
    }
    let lower = clean.to_lowercase();
    let trimmed = lower.trim_start();
    let tokens = lexical_tokens(&lower);
    let mut score = 0_i64;
    let mut flags = 0_u32;
    let successful_check = (trimmed.starts_with("ok ") && trimmed.contains(" - "))
        || (trimmed.starts_with("test ") && trimmed.ends_with(" ... ok"));
    let successful_outcome = is_success_outcome(trimmed);
    let successful = successful_check || successful_outcome;
    let diff_content = matches!(trimmed.chars().next(), Some('+' | '-'))
        || trimmed.starts_with("b+")
        || trimmed.starts_with("b-");
    if is_failure_outcome(trimmed) {
        score += 220;
        flags |= line_flag::FAILURE;
    } else if successful_outcome {
        score += 180;
        flags |= line_flag::SUCCESS;
    } else if successful_check {
        score += 20;
        flags |= line_flag::SUCCESSFUL_CHECK;
    }
    if trimmed.starts_with("diff --git ") {
        score += 140;
        flags |= line_flag::DIFF;
    }
    if !successful
        && !diff_content
        && (tokens.has(token::SEVERE)
            || has_runtime_exception(trimmed)
            || [
                "permission denied",
                "no such file",
                "command not found",
                "timed out",
            ]
            .iter()
            .any(|phrase| lower.contains(phrase)))
    {
        score += 100;
        flags |= line_flag::ERROR;
    }
    if !successful && tokens.has(token::TEST) && tokens.has(token::FAIL | token::FAILED) {
        score += 70;
        flags |= line_flag::FAILED_TEST;
    }
    if !successful && !diff_content && (tokens.has(token::WARNING) || lower.contains("warn:")) {
        score += 40;
        flags |= line_flag::WARNING;
    }
    if score < 80
        && ["note", "remark", "info"]
            .iter()
            .any(|word| tokens.contains_known(word))
    {
        score += 5;
        flags |= line_flag::NOTE;
    }
    for keyword in keywords {
        if !successful && !keyword.is_empty() && lower.contains(keyword) {
            score += 80;
            flags |= line_flag::KEYWORD;
        }
    }
    if is_path_like(clean) {
        score += 30;
        flags |= line_flag::PATH;
    }
    if is_metric_line(&lower) {
        score += 25;
        flags |= line_flag::METRIC;
    }
    if tokens.has(token::TODO | token::PIRA) {
        score += 20;
        flags |= line_flag::MARKER;
    }
    if is_command_boundary(clean) {
        score += 80;
        flags |= line_flag::COMMAND_BOUNDARY;
    }
    if lower.contains(" at ")
        || lower.trim_start().starts_with("at ")
        || lower.contains("stack backtrace")
    {
        score += 15;
        flags |= line_flag::STACK;
    }
    if stream == StreamKind::Stderr && !is_progress_noise(&lower) {
        score += 10;
        flags |= line_flag::STDERR;
    }
    let position = position_boost(line_number, total);
    if position > 0 {
        score += position;
        flags |= line_flag::POSITION;
    }
    if exit_code != 0
        && line_number + 5 > total
        && (stream == StreamKind::Stderr
            || tokens.has(token::EXIT | token::FAIL | token::FAILED | token::ERROR))
    {
        score += 20;
        flags |= line_flag::NONZERO_TAIL;
    }
    if !successful && has_structured_numeric_anomaly(&tokens, &lower) {
        score += 35;
        flags |= line_flag::NUMERIC_ANOMALY;
    }
    let token_bonus = tokens.informative as i64;
    score += token_bonus;
    if token_bonus > 0 {
        flags |= line_flag::INFORMATIVE;
    }
    (score, flags)
}

fn is_rendering_scaffolding(trimmed: &str) -> bool {
    if trimmed.starts_with("--- begin ")
        || trimmed.starts_with("--- end ")
        || trimmed == "PROGRAM data:"
        || trimmed == "Common PROGRAM line forms:"
        || trimmed
            .chars()
            .all(|character| character.is_ascii_punctuation() || character.is_whitespace())
    {
        return true;
    }
    let without_marker = trimmed
        .strip_prefix('>')
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let Some((left, right)) = without_marker.split_once('|') else {
        return false;
    };
    right.trim().is_empty()
        && !left.trim().is_empty()
        && left.trim().bytes().all(|byte| byte.is_ascii_digit())
}

fn is_command_boundary(clean: &str) -> bool {
    clean.trim_start().starts_with("# pira_")
}

struct TokenFacts {
    bits: u32,
    informative: usize,
}

impl TokenFacts {
    fn has(&self, flags: u32) -> bool {
        self.bits & flags != 0
    }

    fn contains_known(&self, value: &str) -> bool {
        self.has(token::flag(value))
    }
}

mod token {
    pub const FATAL: u32 = 1 << 0;
    pub const ERROR: u32 = 1 << 1;
    pub const FAILURE: u32 = 1 << 2;
    pub const FAILED: u32 = 1 << 3;
    pub const PANIC: u32 = 1 << 4;
    pub const EXCEPTION: u32 = 1 << 5;
    pub const TRACEBACK: u32 = 1 << 6;
    pub const TIMEOUT: u32 = 1 << 7;
    pub const TEST: u32 = 1 << 8;
    pub const FAIL: u32 = 1 << 9;
    pub const WARNING: u32 = 1 << 10;
    pub const NOTE: u32 = 1 << 11;
    pub const REMARK: u32 = 1 << 12;
    pub const INFO: u32 = 1 << 13;
    pub const TODO: u32 = 1 << 14;
    pub const PIRA: u32 = 1 << 15;
    pub const EXIT: u32 = 1 << 16;
    pub const NAN: u32 = 1 << 17;
    pub const INF: u32 = 1 << 18;
    pub const INFINITY: u32 = 1 << 19;
    pub const OVERFLOW: u32 = 1 << 20;
    pub const UNDERFLOW: u32 = 1 << 21;
    pub const SEVERE: u32 =
        FATAL | ERROR | FAILURE | FAILED | PANIC | EXCEPTION | TRACEBACK | TIMEOUT;
    pub const ANOMALY: u32 = NAN | INF | INFINITY | OVERFLOW | UNDERFLOW;

    pub fn flag(value: &str) -> u32 {
        match value {
            "fatal" => FATAL,
            "error" => ERROR,
            "failure" => FAILURE,
            "failed" => FAILED,
            "panic" => PANIC,
            "exception" => EXCEPTION,
            "traceback" => TRACEBACK,
            "timeout" => TIMEOUT,
            "test" => TEST,
            "fail" => FAIL,
            "warning" => WARNING,
            "note" => NOTE,
            "remark" => REMARK,
            "info" => INFO,
            "todo" => TODO,
            "pira" => PIRA,
            "exit" => EXIT,
            "nan" => NAN,
            "inf" => INF,
            "infinity" => INFINITY,
            "overflow" => OVERFLOW,
            "underflow" => UNDERFLOW,
            _ => 0,
        }
    }
}

fn lexical_tokens(value: &str) -> TokenFacts {
    let mut bits = 0_u32;
    let mut informative = [""; 10];
    let mut informative_count = 0_usize;
    for value in value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|value| !value.is_empty())
    {
        bits |= token::flag(value);
        if informative_count < informative.len()
            && !is_stopword(value)
            && !informative[..informative_count].contains(&value)
        {
            informative[informative_count] = value;
            informative_count += 1;
        }
    }
    TokenFacts {
        bits,
        informative: informative_count,
    }
}

fn has_structured_numeric_anomaly(tokens: &TokenFacts, lower: &str) -> bool {
    tokens.has(token::ANOMALY)
        && (is_metric_line(lower)
            || lower.contains("=nan")
            || lower.contains(": nan")
            || lower.contains("overflowerror")
            || lower.contains("arithmetic overflow"))
}

fn is_failure_outcome(trimmed: &str) -> bool {
    trimmed.starts_with("not ok ")
        || trimmed.starts_with("failed ")
        || trimmed.starts_with("error: could not")
        || trimmed.starts_with("error: aborting")
        || trimmed.starts_with("fatal:")
        || trimmed.starts_with("panic:")
        || trimmed.starts_with("traceback ")
        || trimmed.starts_with("diff in ")
        || trimmed.contains("test result: failed")
        || trimmed.contains("process completed with exit code")
        || trimmed.contains("tests failed")
        || trimmed.contains("command not found")
        || trimmed.contains("permission denied")
        || starts_with_exception_type(trimmed)
}

fn starts_with_exception_type(trimmed: &str) -> bool {
    exception_re().find(trimmed).is_some_and(|found| {
        found.start() == 0 && has_exception_punctuation(trimmed, 0, found.end())
    })
}

fn is_dependency_path(value: &str) -> bool {
    let lower = value.replace('\\', "/").to_ascii_lowercase();
    [
        ".cargo/registry/",
        "cargo/registry/",
        "node_modules/",
        "site-packages/",
        "vendor/",
        "go/pkg/mod/",
        ".gradle/caches/",
        ".m2/repository/",
        ".nuget/packages/",
        "/gems/",
    ]
    .iter()
    .any(|part| lower.contains(part))
}

fn is_success_outcome(trimmed: &str) -> bool {
    trimmed.starts_with("test result: ok")
        || trimmed.starts_with("result: ok")
        || trimmed.starts_with("ok:")
        || trimmed == "verification passed."
        || trimmed == "verification passed"
        || trimmed.ends_with("all tests passed")
}

fn position_boost(line: usize, total: usize) -> i64 {
    let boosts = [15, 12, 9, 6, 3];
    if (1..=5).contains(&line) {
        return boosts[line - 1];
    }
    if total >= line && total - line < 5 {
        return boosts[total - line];
    }
    0
}

fn is_progress_noise(lower: &str) -> bool {
    lower.contains('%') && (lower.contains("download") || lower.contains("progress"))
}

fn is_metric_line(lower: &str) -> bool {
    let has_key = lower.split(|c: char| !c.is_alphanumeric()).any(|token| {
        matches!(
            token,
            "accuracy"
                | "loss"
                | "metric"
                | "score"
                | "auc"
                | "f1"
                | "precision"
                | "recall"
                | "passed"
                | "failed"
                | "result"
        )
    });
    has_key && lower.chars().any(|character| character.is_ascii_digit())
}

fn is_path_like(value: &str) -> bool {
    value.split_whitespace().any(|raw| {
        let token = raw.trim_matches(|character: char| {
            matches!(character, ',' | ';' | ')' | '(' | '[' | ']' | '"' | '\'')
        });
        let path_part = token.split(':').next().unwrap_or(token);
        let extension = std::path::Path::new(path_part)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        let recognized = matches!(
            extension.as_deref(),
            Some(
                "rs" | "py"
                    | "md"
                    | "txt"
                    | "sh"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "js"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "go"
                    | "java"
                    | "kt"
                    | "swift"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "json"
            )
        );
        recognized && (token.contains('/') || token.contains('\\') || token.contains(':'))
    })
}

fn is_stopword(word: &str) -> bool {
    matches!(
        word,
        "the"
            | "and"
            | "for"
            | "with"
            | "this"
            | "that"
            | "from"
            | "into"
            | "have"
            | "has"
            | "are"
            | "was"
            | "were"
            | "you"
            | "your"
            | "but"
            | "not"
            | "all"
            | "out"
            | "line"
            | "lines"
            | "noise"
            | "more"
            | "done"
            | "begin"
            | "end"
            | "build"
            | "word"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_test_name_is_not_a_diagnostic() {
        let (_, flags) = score_line(
            "ok 39 - error beats noise",
            StreamKind::Stdout,
            39,
            100,
            1,
            &[],
        );
        assert_ne!(flags & line_flag::SUCCESSFUL_CHECK, 0);
        assert_eq!(flags & line_flag::ERROR, 0);
        assert_eq!(flags & line_flag::FAILED_TEST, 0);
    }

    #[test]
    fn actual_test_failure_is_an_outcome() {
        let (_, flags) = score_line(
            "not ok 72 - direct transform count mismatch",
            StreamKind::Stdout,
            72,
            100,
            1,
            &[],
        );
        assert_ne!(flags & line_flag::FAILURE, 0);
    }

    #[test]
    fn successful_result_does_not_trigger_failed_test() {
        let (_, flags) = score_line(
            "test result: ok. 11 passed; 0 failed",
            StreamKind::Stdout,
            10,
            10,
            0,
            &[],
        );
        assert_ne!(flags & line_flag::SUCCESS, 0);
        assert_eq!(flags & line_flag::FAILED_TEST, 0);
    }

    #[test]
    fn rust_test_harness_success_is_not_a_diagnostic() {
        let (_, flags) = score_line(
            "test parser::failed_input_is_rejected ... ok",
            StreamKind::Stdout,
            8,
            10,
            0,
            &[],
        );
        assert_ne!(flags & line_flag::SUCCESSFUL_CHECK, 0);
        assert_eq!(flags & line_flag::ERROR, 0);
    }

    #[test]
    fn search_terms_use_diagnostic_identifiers_and_outcome_phrases() {
        assert_eq!(
            structured_search_terms(
                "not ok 72 - direct transform count mismatch: CompletedProcess(--store-dir /tmp/x)"
            ),
            vec!["direct transform count mismatch"]
        );
        let terms = structured_search_terms(
            "error[E0425]: cannot find value; clippy::collapsible_if at src/lib.rs:42:7",
        );
        assert!(terms.contains(&"E0425".to_string()));
        assert!(terms.contains(&"clippy::collapsible_if".to_string()));
        assert!(terms.contains(&"src/lib.rs:42:7".to_string()));
        assert!(
            structured_search_terms(
                "error: in cargo/registry/src/pkg/src/lib.rs and src/main.rs:7:2"
            )
            .iter()
            .all(|term| !term.contains("cargo/registry"))
        );
        assert!(is_failure_outcome("assertionerror: expected value"));
    }

    #[test]
    fn search_terms_cover_broad_diagnostic_grammars() {
        assert!(
            structured_search_terms("panic: runtime error: index out of range")
                .iter()
                .any(|term| term.starts_with("panic:"))
        );
        assert!(
            structured_search_terms(
                "Program.cs(17,9): error CS0103: a referenced name does not exist"
            )
            .iter()
            .any(|term| term == "CS0103")
        );
        assert!(
            structured_search_terms(
                "app/service.rb:23:in `call': undefined method for nil (NoMethodError)"
            )
            .iter()
            .any(|term| term == "NoMethodError")
        );
        assert!(
            structured_search_terms("shutil.Error: file operation failed")
                .iter()
                .any(|term| term == "shutil.Error")
        );
        assert!(
            structured_search_terms("deploy.sh: command not found")
                .iter()
                .any(|term| term == "command not found")
        );
    }

    #[test]
    fn suggestions_reject_prose_serialization_and_dependency_paths() {
        assert!(!is_diagnostic_evidence(
            "ValueError is commonly used to report conversion failures."
        ));
        assert!(is_serialized_payload_line(
            r#"{"status":"failed","example":"TypeError: demonstration"}"#
        ));
        assert!(is_dependency_path(
            r"C:\project\node_modules\package\index.js:8:3"
        ));
        assert!(is_dependency_path(
            "/home/user/go/pkg/mod/example.org/lib/core.go:44"
        ));
    }

    #[test]
    fn diff_suggestions_require_context_and_parse_quoted_paths() {
        assert_eq!(
            diff_search_term(r#"diff --git "a/docs/old name.md" "b/docs/new name.md""#),
            Some("new name.md".into())
        );
        assert_eq!(
            diff_search_term("diff --git a/docs/old name.md b/docs/new name.md"),
            Some("new name.md".into())
        );
        assert!(is_diff_context(&[
            "git".into(),
            "diff".into(),
            "--stat".into()
        ]));
        assert!(!is_diff_context(&["cat".into(), "guide.md".into()]));
    }

    #[test]
    fn successful_summary_reserves_latest_check() {
        let lines = (1..=5)
            .map(|line| LineMeta {
                line,
                stream: StreamKind::Stdout,
                offset: 0,
                length: 1,
                score: if line == 1 { 100 } else { 20 },
                flags: line_flag::SUCCESSFUL_CHECK,
            })
            .collect::<Vec<_>>();
        assert!(select_important(&lines, 2).contains(&4));
    }

    #[test]
    fn summary_boilerplate_and_command_boundaries_are_separated() {
        assert!(is_summary_boilerplate(
            "--- begin untrusted repository source ---"
        ));
        assert!(is_summary_boilerplate("# pira_nav outline file=\"x.py\""));
        assert!(is_summary_boilerplate("21 |"));
        assert!(is_summary_boilerplate("----------------"));
        assert!(is_summary_boilerplate("OK"));
        assert!(is_summary_boilerplate("\"runs\": 20,"));
        assert!(is_summary_boilerplate("\"command\": ["));
        assert!(!is_summary_boilerplate("error: compilation failed"));
        assert!(!is_summary_boilerplate(
            "\"message\": \"compilation failed\""
        ));
        assert!(is_command_boundary("# pira_nav search path=\".\""));
        assert!(!is_command_boundary("ordinary program output"));
        assert!(is_rendering_scaffolding(
            "--- begin untrusted repository source ---"
        ));
        assert!(!is_rendering_scaffolding("error: compilation failed"));
    }

    #[test]
    fn command_boundaries_do_not_displace_failure_evidence() {
        let mut lines = (0..12)
            .map(|index| LineMeta {
                line: index + 1,
                stream: StreamKind::Stdout,
                offset: 0,
                length: 1,
                score: 10,
                flags: if index % 3 == 0 {
                    line_flag::COMMAND_BOUNDARY
                } else {
                    0
                },
            })
            .collect::<Vec<_>>();
        lines[10].score = 300;
        lines[10].flags = line_flag::FAILURE;
        let selected = select_important(&lines, 6);
        assert!(selected.contains(&10));
        assert!(
            selected
                .iter()
                .any(|&index| lines[index].has(line_flag::COMMAND_BOUNDARY))
        );
    }

    #[test]
    fn json_synopsis_preserves_scalars_and_compacts_large_arrays() {
        let value = serde_json::json!({
            "statuses": [
                {"name": "tree", "exit": 0},
                {"name": "tests", "exit": 1}
            ],
            "tree": {
                "path_count": 4559,
                "prefix": {"nn": 138, "_dynamo": 119},
                "files": (0..20).collect::<Vec<_>>()
            },
            "diagnostics": (0..300).map(|index| format!("warning {index}")).collect::<Vec<_>>(),
            "counts": {"warnings": 197, "citations": 108}
        });
        let bytes = serde_json::to_vec_pretty(&value).unwrap();
        let lines = json_synopsis_bytes(&bytes, 16).unwrap();
        assert!(lines.contains(
            &r#"$.statuses = [{"exit":0,"name":"tree"},{"exit":1,"name":"tests"}]"#.to_string()
        ));
        assert!(lines.contains(&"$.tree.path_count = 4559".to_string()));
        assert!(lines.contains(&"$.diagnostics = array[300]".to_string()));
        assert!(lines.contains(&r#"$.counts = {"citations":108,"warnings":197}"#.to_string()));
        assert!(lines.iter().all(|line| !line.contains("warning 299")));
    }

    #[test]
    fn json_synopsis_is_bounded_and_requires_one_complete_document() {
        let hostile_key = format!("key-{}", "x".repeat(1_000));
        let value = serde_json::json!({
            hostile_key: {"nested": {"value": 1}},
            "long": "x".repeat(1_000),
            "many": (0..100).collect::<Vec<_>>(),
        });
        let lines = json_synopsis_bytes(&serde_json::to_vec(&value).unwrap(), 3).unwrap();
        assert!(lines.len() <= 3);
        assert!(lines.iter().all(|line| line.len() < 1_100));
        assert!(json_synopsis_bytes(b"{\"ok\": true}\n{\"ok\": false}", 8).is_none());
        assert!(json_synopsis_bytes(b"ordinary text", 8).is_none());
    }
}
