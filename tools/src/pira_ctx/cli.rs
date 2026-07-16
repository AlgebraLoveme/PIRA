use std::path::PathBuf;

pub const USAGE: &str = crate::help::GLOBAL;

pub const MAX_INTENT_BYTES: usize = 256;
pub const MAX_KEYWORDS: usize = 16;
pub const MAX_KEYWORD_BYTES: usize = 256;
pub const MAX_SEARCH_CONTEXT: usize = 20;
pub const MAX_QUERY_BYTES: usize = 4096;
pub const MAX_TRANSFORM_PATTERNS: usize = 16;
pub const MAX_HISTORY_RESULTS: usize = 100;
pub const MAX_HISTORY_WINDOW: usize = 8_000;
pub const MAX_RECAP_EVENTS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Exact,
    Check,
    Capture,
    Search,
    Range,
    Raw,
    Exec,
    Transform,
    Recap,
    History,
    Batch,
    List,
    Stats,
    Verify,
    Prune,
    Forget,
    Help,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryScope {
    Current,
    Workspace,
}

#[derive(Debug, Clone, Default)]
pub struct TransformOptions {
    pub plan: Option<PathBuf>,
    pub matches: Vec<String>,
    pub excludes: Vec<String>,
    pub unique: bool,
    pub count: bool,
    pub head: Option<usize>,
    pub tail: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub store_dir: Option<PathBuf>,
    pub intent: Option<String>,
    pub keywords: Vec<String>,
    pub cmd: Vec<String>,
    pub target: Option<String>,
    pub query: Option<String>,
    pub regex: bool,
    pub history_scope: HistoryScope,
    pub history_details: bool,
    pub context: usize,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub workspace_current: bool,
    pub raw_stream: Option<RawStream>,
    pub exec_code: Option<String>,
    pub exec_file: Option<PathBuf>,
    pub python: Option<String>,
    pub max_age_days: Option<u64>,
    pub max_store_bytes: Option<u64>,
    pub prune_legacy_events: bool,
    pub transform: TransformOptions,
    pub limit: usize,
    pub history_lookback: Option<usize>,
    pub history_offset: usize,
    pub history_since: Option<String>,
    pub history_until: Option<String>,
    pub batch_file: Option<PathBuf>,
    pub help_topic: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Auto,
            store_dir: None,
            intent: None,
            keywords: vec![],
            cmd: vec![],
            target: None,
            query: None,
            regex: false,
            history_scope: HistoryScope::Current,
            history_details: false,
            context: 0,
            start_line: None,
            end_line: None,
            workspace_current: false,
            raw_stream: None,
            exec_code: None,
            exec_file: None,
            python: None,
            max_age_days: None,
            max_store_bytes: None,
            prune_legacy_events: false,
            transform: TransformOptions::default(),
            limit: 20,
            history_lookback: None,
            history_offset: 0,
            history_since: None,
            history_until: None,
            batch_file: None,
            help_topic: None,
        }
    }
}

pub fn parse_args(args: &[String]) -> Result<Config, String> {
    if args.is_empty() {
        return Err(
            "missing command or options\nRun `pira_ctx --help` for command selection.".into(),
        );
    }
    if let Some(topic) = parse_help_request(args)? {
        return Ok(Config {
            mode: Mode::Help,
            help_topic: topic,
            ..Default::default()
        });
    }
    if matches!(args[0].as_str(), "--version" | "-V" | "version") {
        return Ok(Config {
            mode: Mode::Version,
            ..Default::default()
        });
    }
    let topic = invocation_topic(args);
    parse_non_help(args).map_err(|error| usage_error(&topic, error))
}

fn parse_non_help(args: &[String]) -> Result<Config, String> {
    let mut c = Config::default();
    match args[0].as_str() {
        "auto" => {
            let p = parse_exec_options(&mut c, args, 1, true)?;
            parse_command(&mut c, args, p)?;
            require_intent(&mut c)?;
        }
        "exact" => {
            c.mode = Mode::Exact;
            let p = parse_exec_options(&mut c, args, 1, false)?;
            parse_command(&mut c, args, p)?;
            require_intent(&mut c)?;
        }
        "check" => {
            c.mode = Mode::Check;
            let p = parse_exec_options(&mut c, args, 1, false)?;
            parse_command(&mut c, args, p)?;
            require_intent(&mut c)?;
        }
        "capture" | "summary" => {
            c.mode = Mode::Capture;
            let p = parse_exec_options(&mut c, args, 1, true)?;
            parse_command(&mut c, args, p)?;
            require_intent(&mut c)?;
        }
        "search" => parse_search(&mut c, args)?,
        "range" => {
            c.mode = Mode::Range;
            let mut p = parse_store(&mut c, args, 1)?;
            if p + 3 != args.len() {
                return Err(USAGE.into());
            }
            c.target = Some(args[p].clone());
            p += 1;
            c.start_line = Some(args[p].parse().map_err(|_| "invalid start_line")?);
            p += 1;
            c.end_line = Some(args[p].parse().map_err(|_| "invalid end_line")?);
        }
        "raw" => parse_raw(&mut c, args)?,
        "exec" => parse_python_exec(&mut c, args)?,
        "transform" => parse_transform(&mut c, args)?,
        "recap" => {
            c.mode = Mode::Recap;
            let mut p = parse_store(&mut c, args, 1)?;
            while p < args.len() {
                if args[p] != "--limit" {
                    return Err(USAGE.into());
                }
                p += 1;
                c.limit = parse_value(args, &mut p, "--limit")?;
            }
            if c.limit > MAX_RECAP_EVENTS {
                return Err(format!("recap --limit is capped at {MAX_RECAP_EVENTS}"));
            }
        }
        "history" => parse_history(&mut c, args)?,
        "batch" => {
            c.mode = Mode::Batch;
            let mut p = parse_store(&mut c, args, 1)?;
            c.batch_file = Some(PathBuf::from(take(args, &mut p, "SPEC_FILE")?));
            while p < args.len() {
                if args[p] != "--intent" {
                    return Err(USAGE.into());
                }
                p += 1;
                c.intent = Some(take(args, &mut p, "--intent")?.into());
            }
            normalize_optional_intent(&mut c)?;
        }
        "list" => {
            c.mode = Mode::List;
            let mut p = parse_store(&mut c, args, 1)?;
            while p < args.len() {
                match args[p].as_str() {
                    "--workspace" if args.get(p + 1).map(String::as_str) == Some("current") => {
                        c.workspace_current = true;
                        p += 2;
                    }
                    "--limit" => {
                        p += 1;
                        c.limit = parse_value(args, &mut p, "--limit")?;
                        if c.limit > 100 {
                            return Err("list --limit is capped at 100".into());
                        }
                    }
                    _ => return Err(USAGE.into()),
                }
            }
        }
        "stats" | "verify" => {
            c.mode = if args[0] == "stats" {
                Mode::Stats
            } else {
                Mode::Verify
            };
            let p = parse_store(&mut c, args, 1)?;
            if c.mode == Mode::Verify && p + 1 != args.len() {
                return Err(USAGE.into());
            }
            if p < args.len() {
                c.target = Some(args[p].clone());
                if p + 1 != args.len() {
                    return Err(USAGE.into());
                }
            }
        }
        "prune" => parse_prune(&mut c, args)?,
        "forget" => {
            c.mode = Mode::Forget;
            let mut p = parse_store(&mut c, args, 1)?;
            c.target = Some(take(args, &mut p, "RESULT|history")?.into());
            while p < args.len() {
                match args[p].as_str() {
                    "--scope" => {
                        p += 1;
                        c.history_scope = parse_history_scope(take(args, &mut p, "--scope")?)?;
                    }
                    _ => return Err(USAGE.into()),
                }
            }
            if c.target.as_deref() != Some("history") && c.history_scope != HistoryScope::Current {
                return Err("forget --scope is valid only with history".into());
            }
        }
        _ => {
            let p = parse_exec_options(&mut c, args, 0, true)?;
            parse_command(&mut c, args, p)?;
            require_intent(&mut c)?;
        }
    }
    validate_keywords(&c.keywords)?;
    Ok(c)
}

fn parse_help_request(args: &[String]) -> Result<Option<Option<String>>, String> {
    let help_flag = |value: &str| matches!(value, "--help" | "-h");
    let boundary = args
        .iter()
        .position(|value| value == "--")
        .unwrap_or(args.len());
    let wrapper_args = &args[..boundary];
    let requested = if wrapper_args.len() == 1
        && (help_flag(&wrapper_args[0]) || wrapper_args[0] == "help")
    {
        return Ok(Some(None));
    } else if wrapper_args.len() == 2 && (wrapper_args[0] == "help" || help_flag(&wrapper_args[0]))
    {
        Some(wrapper_args[1].as_str())
    } else if wrapper_args.len() == 2 && help_flag(&wrapper_args[1]) {
        Some(wrapper_args[0].as_str())
    } else {
        None
    };
    let Some(topic) = requested else {
        return Ok(None);
    };
    let canonical = crate::help::canonical_topic(topic).ok_or_else(|| {
        format!("unknown help topic {topic:?}\nRun `pira_ctx --help` for command selection.")
    })?;
    Ok(Some(Some(canonical.to_string())))
}

fn invocation_topic(args: &[String]) -> String {
    crate::help::canonical_topic(&args[0])
        .unwrap_or("auto")
        .to_string()
}

fn usage_error(topic: &str, error: String) -> String {
    let message = if error == USAGE {
        format!("invalid {topic} usage")
    } else {
        error
    };
    format!("{message}\nRun `pira_ctx {topic} --help` for usage.")
}

fn parse_python_exec(c: &mut Config, args: &[String]) -> Result<(), String> {
    c.mode = Mode::Exec;
    let mut p = 1;
    while p < args.len() {
        match args[p].as_str() {
            "--store-dir" => {
                p += 1;
                c.store_dir = Some(take(args, &mut p, "--store-dir")?.into());
            }
            "--intent" => {
                p += 1;
                c.intent = Some(take(args, &mut p, "--intent")?.into());
            }
            "--code" => {
                p += 1;
                if c.exec_code
                    .replace(take(args, &mut p, "--code")?.into())
                    .is_some()
                {
                    return Err("choose exactly one --code CODE or --file PATH".into());
                }
            }
            "--file" => {
                p += 1;
                if c.exec_file
                    .replace(take(args, &mut p, "--file")?.into())
                    .is_some()
                {
                    return Err("choose exactly one --code CODE or --file PATH".into());
                }
            }
            "--python" => {
                p += 1;
                if c.python
                    .replace(take(args, &mut p, "--python")?.into())
                    .is_some()
                {
                    return Err("provide --python PATH at most once".into());
                }
            }
            value if c.target.is_none() && (value == "--last" || !value.starts_with('-')) => {
                c.target = Some(value.into());
                p += 1;
            }
            _ => return Err(USAGE.into()),
        }
    }
    require_intent(c)?;
    match (c.exec_code.is_some(), c.exec_file.is_some()) {
        (true, false) | (false, true) => {}
        _ => return Err("choose exactly one --code CODE or --file PATH".into()),
    }
    if c.target.is_none() {
        return Err("exec requires RESULT".into());
    }
    if c.python.as_deref().is_some_and(|value| value.is_empty()) {
        return Err("--python PATH must not be empty".into());
    }
    Ok(())
}

fn validate_keywords(keywords: &[String]) -> Result<(), String> {
    if keywords.len() > MAX_KEYWORDS {
        return Err(format!(
            "at most {MAX_KEYWORDS} --keyword values are allowed"
        ));
    }
    for keyword in keywords {
        let trimmed = keyword.trim();
        if trimmed.is_empty()
            || trimmed.len() > MAX_KEYWORD_BYTES
            || trimmed.chars().any(char::is_control)
        {
            return Err(format!(
                "each --keyword must be non-empty, single-line, and at most {MAX_KEYWORD_BYTES} UTF-8 bytes"
            ));
        }
    }
    Ok(())
}

fn require_intent(c: &mut Config) -> Result<(), String> {
    let value = c
        .intent
        .as_deref()
        .ok_or("external execution requires --intent TEXT")?;
    c.intent = Some(validate_intent(value)?.to_string());
    Ok(())
}

fn normalize_optional_intent(c: &mut Config) -> Result<(), String> {
    if let Some(value) = c.intent.as_deref() {
        c.intent = Some(validate_intent(value)?.to_string());
    }
    Ok(())
}

pub fn validate_intent(value: &str) -> Result<&str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("external execution requires --intent TEXT".into());
    }
    let bytes = trimmed.len();
    if bytes > MAX_INTENT_BYTES {
        return Err(format!(
            "intent is too long: {} characters, {bytes} UTF-8 bytes; maximum {MAX_INTENT_BYTES} bytes. Rerun with a concise, single-line immediate purpose",
            trimmed.chars().count()
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(
            "intent must be a concise, single-line immediate purpose without control characters. Rerun with a corrected intent"
                .into(),
        );
    }
    Ok(trimmed)
}
fn parse_command(c: &mut Config, args: &[String], p: usize) -> Result<(), String> {
    if args.get(p).map(String::as_str) != Some("--") || p + 1 >= args.len() {
        return Err(USAGE.into());
    }
    c.cmd = args[p + 1..].to_vec();
    Ok(())
}
fn parse_exec_options(
    c: &mut Config,
    args: &[String],
    mut p: usize,
    keywords: bool,
) -> Result<usize, String> {
    while p < args.len() {
        match args[p].as_str() {
            "--store-dir" => {
                p += 1;
                c.store_dir = Some(take(args, &mut p, "--store-dir")?.into())
            }
            "--intent" => {
                p += 1;
                c.intent = Some(take(args, &mut p, "--intent")?.into())
            }
            "--keyword" if keywords => {
                p += 1;
                c.keywords.push(take(args, &mut p, "--keyword")?.into())
            }
            "--" => break,
            _ => return Err(USAGE.into()),
        }
    }
    Ok(p)
}
fn parse_store(c: &mut Config, args: &[String], mut p: usize) -> Result<usize, String> {
    if args.get(p).map(String::as_str) == Some("--store-dir") {
        p += 1;
        c.store_dir = Some(take(args, &mut p, "--store-dir")?.into())
    }
    Ok(p)
}
fn parse_search(c: &mut Config, args: &[String]) -> Result<(), String> {
    c.mode = Mode::Search;
    let mut p = parse_store(c, args, 1)?;
    c.target = Some(take(args, &mut p, "RESULT")?.into());
    c.query = Some(take(args, &mut p, "QUERY")?.into());
    while p < args.len() {
        match args[p].as_str() {
            "--regex" => {
                c.regex = true;
                p += 1
            }
            "--context" => {
                p += 1;
                c.context = parse_value(args, &mut p, "--context")?
            }
            _ => return Err(USAGE.into()),
        }
    }
    let query = c.query.as_deref().unwrap_or_default();
    if query.is_empty() || query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control) {
        return Err(format!(
            "search query must be non-empty, single-line, and at most {MAX_QUERY_BYTES} UTF-8 bytes"
        ));
    }
    if c.context > MAX_SEARCH_CONTEXT {
        return Err(format!(
            "--context is limited to {MAX_SEARCH_CONTEXT} lines"
        ));
    }
    Ok(())
}

fn parse_history(c: &mut Config, args: &[String]) -> Result<(), String> {
    c.mode = Mode::History;
    c.limit = 10;
    let mut p = parse_store(c, args, 1)?;
    while p < args.len() {
        match args[p].as_str() {
            "--regex" => {
                c.regex = true;
                p += 1;
            }
            "--limit" => {
                p += 1;
                c.limit = parse_value(args, &mut p, "--limit")?;
            }
            "--lookback" => {
                p += 1;
                let value = take(args, &mut p, "--lookback")?;
                c.history_lookback = if value == "all" {
                    None
                } else {
                    Some(parse_bounded_usize(value, "--lookback")?)
                };
            }
            "--offset" => {
                p += 1;
                c.history_offset = parse_value(args, &mut p, "--offset")?;
            }
            "--since" => {
                p += 1;
                c.history_since = Some(take(args, &mut p, "--since")?.into());
            }
            "--until" => {
                p += 1;
                c.history_until = Some(take(args, &mut p, "--until")?.into());
            }
            "--scope" => {
                p += 1;
                c.history_scope = parse_history_scope(take(args, &mut p, "--scope")?)?;
            }
            "--details" => {
                c.history_details = true;
                p += 1;
            }
            value if !value.starts_with("--") && c.query.is_none() => {
                c.query = Some(take(args, &mut p, "QUERY")?.into());
            }
            _ => return Err(USAGE.into()),
        }
    }
    if let Some(query) = c.query.as_deref() {
        if query.is_empty() || query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control)
        {
            return Err(format!(
                "history query must be non-empty, single-line, and at most {MAX_QUERY_BYTES} UTF-8 bytes"
            ));
        }
    } else if c.regex {
        return Err("history --regex requires QUERY".into());
    }
    if c.limit == 0 || c.limit > MAX_HISTORY_RESULTS {
        return Err(format!(
            "history --limit must be between 1 and {MAX_HISTORY_RESULTS}"
        ));
    }
    if c.history_lookback
        .is_some_and(|value| value == 0 || value > MAX_HISTORY_WINDOW)
    {
        return Err(format!(
            "history --lookback must be all or between 1 and {MAX_HISTORY_WINDOW}"
        ));
    }
    if c.history_offset > MAX_HISTORY_WINDOW {
        return Err(format!(
            "history --offset must be between 0 and {MAX_HISTORY_WINDOW}"
        ));
    }
    for (name, value) in [
        ("--since", c.history_since.as_deref()),
        ("--until", c.history_until.as_deref()),
    ] {
        if value.is_some_and(|value| {
            value.is_empty() || value.len() > 64 || value.chars().any(char::is_control)
        }) {
            return Err(format!(
                "history {name} must be non-empty, single-line, and at most 64 UTF-8 bytes"
            ));
        }
    }
    Ok(())
}

fn parse_bounded_usize(value: &str, name: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{name} requires a non-negative integer or `all`"))
}

fn parse_history_scope(value: &str) -> Result<HistoryScope, String> {
    match value {
        "current" => Ok(HistoryScope::Current),
        "workspace" => Ok(HistoryScope::Workspace),
        _ => Err("--scope must be current or workspace".into()),
    }
}
fn parse_raw(c: &mut Config, args: &[String]) -> Result<(), String> {
    c.mode = Mode::Raw;
    let mut p = parse_store(c, args, 1)?;
    c.target = Some(take(args, &mut p, "RESULT")?.into());
    while p < args.len() {
        let s = match args[p].as_str() {
            "--stdout" => RawStream::Stdout,
            "--stderr" => RawStream::Stderr,
            _ => return Err(USAGE.into()),
        };
        if c.raw_stream.replace(s).is_some() {
            return Err("choose only one stream".into());
        }
        p += 1
    }
    Ok(())
}
fn parse_transform(c: &mut Config, args: &[String]) -> Result<(), String> {
    c.mode = Mode::Transform;
    let mut p = parse_store(c, args, 1)?;
    c.target = Some(take(args, &mut p, "RESULT")?.into());
    while p < args.len() {
        match args[p].as_str() {
            "--plan" => {
                p += 1;
                c.transform.plan = Some(take(args, &mut p, "--plan")?.into())
            }
            "--match" => {
                p += 1;
                c.transform
                    .matches
                    .push(take(args, &mut p, "--match")?.into())
            }
            "--exclude" => {
                p += 1;
                c.transform
                    .excludes
                    .push(take(args, &mut p, "--exclude")?.into())
            }
            "--unique" => {
                c.transform.unique = true;
                p += 1
            }
            "--count" => {
                c.transform.count = true;
                p += 1
            }
            "--head" => {
                p += 1;
                c.transform.head = Some(parse_value(args, &mut p, "--head")?)
            }
            "--tail" => {
                p += 1;
                c.transform.tail = Some(parse_value(args, &mut p, "--tail")?)
            }
            _ => return Err(USAGE.into()),
        }
    }
    if c.transform.matches.len() > MAX_TRANSFORM_PATTERNS
        || c.transform.excludes.len() > MAX_TRANSFORM_PATTERNS
    {
        return Err(format!(
            "transform accepts at most {MAX_TRANSFORM_PATTERNS} --match and --exclude patterns"
        ));
    }
    if c.transform
        .matches
        .iter()
        .chain(&c.transform.excludes)
        .any(|pattern| pattern.len() > MAX_QUERY_BYTES)
    {
        return Err(format!(
            "transform regex patterns are limited to {MAX_QUERY_BYTES} UTF-8 bytes"
        ));
    }
    Ok(())
}
fn parse_prune(c: &mut Config, args: &[String]) -> Result<(), String> {
    c.mode = Mode::Prune;
    let mut p = 1;
    while p < args.len() {
        match args[p].as_str() {
            "--store-dir" => {
                p += 1;
                c.store_dir = Some(take(args, &mut p, "--store-dir")?.into())
            }
            "--max-age-days" => {
                p += 1;
                c.max_age_days = Some(parse_value(args, &mut p, "--max-age-days")?)
            }
            "--max-store-bytes" => {
                p += 1;
                c.max_store_bytes = Some(parse_value(args, &mut p, "--max-store-bytes")?)
            }
            "--legacy-events" => {
                c.prune_legacy_events = true;
                p += 1;
            }
            _ => return Err(USAGE.into()),
        }
    }
    if c.max_age_days.is_none() && c.max_store_bytes.is_none() && !c.prune_legacy_events {
        return Err("prune requires a limit".into());
    }
    Ok(())
}
fn take<'a>(args: &'a [String], p: &mut usize, name: &str) -> Result<&'a str, String> {
    let v = args.get(*p).ok_or_else(|| format!("missing {name}"))?;
    *p += 1;
    Ok(v)
}
fn parse_value<T: std::str::FromStr>(
    args: &[String],
    p: &mut usize,
    name: &str,
) -> Result<T, String> {
    take(args, p, name)?
        .parse()
        .map_err(|_| format!("invalid {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn intent_required() {
        assert!(parse_args(&a(&["--", "echo"])).is_err());
        assert!(parse_args(&a(&["--intent", "check", "--", "echo"])).is_ok())
    }
    #[test]
    fn summary_alias() {
        assert_eq!(
            parse_args(&a(&["summary", "--intent", "x", "--", "echo"]))
                .unwrap()
                .mode,
            Mode::Capture
        )
    }
    #[test]
    fn auto_is_an_explicit_alias_for_default_execution() {
        let config = parse_args(&a(&["auto", "--intent", "x", "--", "echo"])).unwrap();
        assert_eq!(config.mode, Mode::Auto);
        assert_eq!(config.cmd, a(&["echo"]));
    }
    #[test]
    fn command_help_aliases_resolve_before_validation() {
        for args in [
            a(&["exec", "--help"]),
            a(&["help", "exec"]),
            a(&["--help", "exec"]),
            a(&["-h", "exec"]),
        ] {
            let config = parse_args(&args).unwrap();
            assert_eq!(config.mode, Mode::Help);
            assert_eq!(config.help_topic.as_deref(), Some("exec"));
        }
        assert_eq!(
            parse_args(&a(&["summary", "--help"]))
                .unwrap()
                .help_topic
                .as_deref(),
            Some("capture")
        );
        assert!(parse_args(&a(&["unknown", "--help"])).is_err());
    }
    #[test]
    fn arguments_after_program_delimiter_are_not_wrapper_help() {
        for args in [
            a(&["--intent", "x", "--", "program", "--help"]),
            a(&["auto", "--intent", "x", "--", "program", "-h"]),
            a(&["exact", "--intent", "x", "--", "program", "help"]),
            a(&["check", "--intent", "x", "--", "program", "--help"]),
            a(&["capture", "--intent", "x", "--", "program", "--help"]),
        ] {
            let config = parse_args(&args).unwrap();
            assert_ne!(config.mode, Mode::Help);
            assert_eq!(config.cmd[0], "program");
            assert!(matches!(config.cmd[1].as_str(), "--help" | "-h" | "help"));
        }
        assert_eq!(
            parse_args(&a(&[
                "exact", "--intent", "help", "--", "program", "--help",
            ]))
            .unwrap()
            .intent
            .as_deref(),
            Some("help")
        );
        assert_eq!(
            parse_args(&a(
                &["exec", "RESULT", "--intent", "x", "--code", "--help",]
            ))
            .unwrap()
            .exec_code
            .as_deref(),
            Some("--help")
        );
    }
    #[test]
    fn parse_errors_point_to_command_help_without_global_help() {
        let error = parse_args(&a(&["exact", "--"])).unwrap_err();
        assert!(error.contains("pira_ctx exact --help"));
        assert!(!error.contains("Choosing a command"));
    }
    #[test]
    fn check_is_an_intent_required_execution_mode() {
        assert!(parse_args(&a(&["check", "--", "echo"])).is_err());
        assert_eq!(
            parse_args(&a(&["check", "--intent", "validate", "--", "echo"]))
                .unwrap()
                .mode,
            Mode::Check
        );
    }

    #[test]
    fn python_exec_requires_result_intent_and_one_program_source() {
        let config = parse_args(&a(&[
            "exec",
            "--last",
            "--intent",
            "count failures",
            "--code",
            "print(MSG_EXIT)",
        ]))
        .unwrap();
        assert_eq!(config.mode, Mode::Exec);
        assert_eq!(config.target.as_deref(), Some("--last"));
        assert_eq!(config.exec_code.as_deref(), Some("print(MSG_EXIT)"));
        assert!(parse_args(&a(&["exec", "--last", "--code", "pass"])).is_err());
        assert!(parse_args(&a(&["exec", "--intent", "x", "--code", "pass"])).is_err());
        assert!(
            parse_args(&a(&[
                "exec", "--last", "--intent", "x", "--code", "pass", "--file", "a.py"
            ]))
            .is_err()
        );
        assert!(
            parse_args(&a(&[
                "exec",
                "--unknown",
                "--intent",
                "x",
                "--code",
                "pass"
            ]))
            .is_err()
        );
    }
    #[test]
    fn internal_needs_no_intent() {
        assert!(parse_args(&a(&["search", "--last", "x"])).is_ok())
    }

    #[test]
    fn recap_limit_preserves_the_structured_output_budget() {
        assert!(parse_args(&a(&["recap", "--limit", "20"])).is_ok());
        assert!(parse_args(&a(&["recap", "--limit", "21"])).is_err());
    }

    #[test]
    fn history_has_optional_filter_and_explicit_bounds() {
        let config = parse_args(&a(&[
            "history",
            "parser|build",
            "--regex",
            "--lookback",
            "750",
            "--limit",
            "12",
        ]))
        .unwrap();
        assert_eq!(config.mode, Mode::History);
        assert_eq!(config.query.as_deref(), Some("parser|build"));
        assert!(config.regex);
        assert_eq!(config.history_lookback, Some(750));
        assert_eq!(config.limit, 12);
        let recent = parse_args(&a(&["history", "--limit", "5"])).unwrap();
        assert_eq!(recent.mode, Mode::History);
        assert_eq!(recent.query, None);
        assert_eq!(recent.limit, 5);
        assert_eq!(recent.history_lookback, None);
        let all = parse_args(&a(&[
            "history",
            "parser",
            "--lookback",
            "all",
            "--offset",
            "2000",
            "--since",
            "48h",
            "--until",
            "24h",
        ]))
        .unwrap();
        assert_eq!(all.limit, 10);
        assert_eq!(all.history_lookback, None);
        assert_eq!(all.history_offset, 2000);
        assert_eq!(all.history_since.as_deref(), Some("48h"));
        assert_eq!(all.history_until.as_deref(), Some("24h"));
        assert_eq!(config.history_scope, HistoryScope::Current);
        let workspace = parse_args(&a(&["history", "--scope", "workspace", "--details"])).unwrap();
        assert_eq!(workspace.history_scope, HistoryScope::Workspace);
        assert!(workspace.history_details);
        let reordered = parse_args(&a(&["history", "--scope", "workspace", "parser"])).unwrap();
        assert_eq!(reordered.query.as_deref(), Some("parser"));
        assert_eq!(reordered.history_scope, HistoryScope::Workspace);
        assert!(parse_args(&a(&["intents", "parser"])).is_err());
        assert!(parse_args(&a(&["intent-search", "parser"])).is_err());
        assert!(parse_args(&a(&["history", "--regex"])).is_err());
        assert!(parse_args(&a(&["history", "parser", "--lookback", "0"])).is_err());
        assert!(parse_args(&a(&["history", "parser", "--lookback", "8001"])).is_err());
        assert!(parse_args(&a(&["history", "parser", "--offset", "8001"])).is_err());
        assert!(parse_args(&a(&["history", "parser", "--limit", "0"])).is_err());
        assert!(parse_args(&a(&["history", "parser", "--limit", "101"])).is_err());
    }

    #[test]
    fn intent_size_is_utf8_bytes() {
        assert!(validate_intent(&"a".repeat(256)).is_ok());
        assert!(validate_intent(&"a".repeat(257)).is_err());
        assert!(validate_intent(&"界".repeat(85)).is_ok());
        assert!(validate_intent(&"界".repeat(86)).is_err());
        assert!(validate_intent("one\ntwo").is_err());
    }

    #[test]
    fn keyword_count_and_size_are_bounded() {
        assert!(validate_keywords(&vec!["x".into(); MAX_KEYWORDS]).is_ok());
        assert!(validate_keywords(&vec!["x".into(); MAX_KEYWORDS + 1]).is_err());
        assert!(validate_keywords(&["x".repeat(MAX_KEYWORD_BYTES + 1)]).is_err());
    }
}
