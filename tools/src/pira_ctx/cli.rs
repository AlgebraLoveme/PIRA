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
    Watch,
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
    Command,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAttention {
    Return,
    Cache,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecInput {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub store_dir: Option<PathBuf>,
    pub intent: Option<String>,
    pub keywords: Vec<String>,
    pub interest: Option<String>,
    pub cmd: Vec<String>,
    pub target: Option<String>,
    pub watch_capture: Option<String>,
    pub watch_current: bool,
    pub watch_latest: bool,
    pub watch_stop: bool,
    pub watch_clear_analyzer: bool,
    pub watch_analyzer_file: Option<PathBuf>,
    pub watch_analyzer_code: Option<String>,
    pub watch_sample_every_ms: u64,
    pub watch_sample_every_set: bool,
    pub watch_attempt_timeout_ms: u64,
    pub watch_deadline_ms: Option<u64>,
    pub watch_review_after_ms: Option<u64>,
    pub watch_inactive_after_ms: Option<u64>,
    pub watch_inactive_after_set: bool,
    pub watch_unchanged_after_ms: Option<u64>,
    pub watch_unchanged_after_set: bool,
    pub watch_no_progress_after_ms: Option<u64>,
    pub watch_no_progress_after_set: bool,
    pub watch_pending_exit: i32,
    pub watch_attention: WatchAttention,
    pub watch_attention_set: bool,
    pub stats_targets: Vec<String>,
    pub stats_brief: bool,
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
    pub exec_inputs: Vec<ExecInput>,
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
            interest: None,
            cmd: vec![],
            target: None,
            watch_capture: None,
            watch_current: false,
            watch_latest: false,
            watch_stop: false,
            watch_clear_analyzer: false,
            watch_analyzer_file: None,
            watch_analyzer_code: None,
            watch_sample_every_ms: 30_000,
            watch_sample_every_set: false,
            watch_attempt_timeout_ms: 20_000,
            watch_deadline_ms: None,
            watch_review_after_ms: None,
            watch_inactive_after_ms: None,
            watch_inactive_after_set: false,
            watch_unchanged_after_ms: None,
            watch_unchanged_after_set: false,
            watch_no_progress_after_ms: None,
            watch_no_progress_after_set: false,
            watch_pending_exit: 75,
            watch_attention: WatchAttention::Return,
            watch_attention_set: false,
            stats_targets: vec![],
            stats_brief: false,
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
            exec_inputs: vec![],
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
        "watch" => parse_watch(&mut c, args)?,
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
        "stats" => {
            c.mode = Mode::Stats;
            let mut p = 1;
            while p < args.len() {
                match args[p].as_str() {
                    "--store-dir" => {
                        if c.store_dir.is_some() {
                            return Err("--store-dir may be specified only once".into());
                        }
                        p += 1;
                        c.store_dir = Some(take(args, &mut p, "--store-dir")?.into());
                    }
                    "--brief" => {
                        if c.stats_brief {
                            return Err("--brief may be specified only once".into());
                        }
                        c.stats_brief = true;
                        p += 1;
                    }
                    value if value.starts_with('-') && value != "--last" => {
                        return Err(USAGE.into());
                    }
                    _ => {
                        c.stats_targets.push(args[p].clone());
                        p += 1;
                    }
                }
            }
            if c.stats_targets.len() > 32 {
                return Err("stats accepts at most 32 results".into());
            }
            if c.stats_brief {
                if c.stats_targets.is_empty() {
                    return Err("stats --brief requires at least one RESULT".into());
                }
            } else if c.stats_targets.len() > 1 {
                return Err("detailed stats accepts one RESULT; use --brief for multiple".into());
            }
            c.target = c.stats_targets.first().cloned();
        }
        "verify" => {
            c.mode = Mode::Verify;
            let p = parse_store(&mut c, args, 1)?;
            if p + 1 != args.len() {
                return Err(USAGE.into());
            }
            c.target = Some(args[p].clone());
        }
        "command" => {
            c.mode = Mode::Command;
            let p = parse_store(&mut c, args, 1)?;
            if p + 1 != args.len() {
                return Err(USAGE.into());
            }
            c.target = Some(args[p].clone());
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
    validate_interest(c.interest.as_deref())?;
    Ok(c)
}

fn parse_watch(c: &mut Config, args: &[String]) -> Result<(), String> {
    c.mode = Mode::Watch;
    let mut p = 1;
    while p < args.len() {
        match args[p].as_str() {
            "--store-dir" => {
                p += 1;
                c.store_dir = Some(take(args, &mut p, "--store-dir")?.into());
            }
            "--capture" => {
                p += 1;
                c.watch_capture = Some(take(args, &mut p, "--capture")?.into());
            }
            "--current" => {
                c.watch_current = true;
                p += 1;
            }
            "--latest" => {
                c.watch_latest = true;
                p += 1;
            }
            "--stop" => {
                c.watch_stop = true;
                p += 1;
            }
            "--analyzer-file" | "--set-analyzer-file" => {
                p += 1;
                c.watch_analyzer_file = Some(take(args, &mut p, "--analyzer-file")?.into());
            }
            "--analyzer-code" | "--set-analyzer-code" => {
                p += 1;
                c.watch_analyzer_code = Some(take(args, &mut p, "--analyzer-code")?.into());
            }
            "--clear-analyzer" => {
                c.watch_clear_analyzer = true;
                p += 1;
            }
            "--sample-every" => {
                p += 1;
                c.watch_sample_every_set = true;
                c.watch_sample_every_ms = parse_watch_duration(
                    take(args, &mut p, "--sample-every")?,
                    100,
                    86_400_000,
                    "--sample-every",
                )?;
            }
            "--attempt-timeout" => {
                p += 1;
                c.watch_attempt_timeout_ms = parse_watch_duration(
                    take(args, &mut p, "--attempt-timeout")?,
                    1_000,
                    600_000,
                    "--attempt-timeout",
                )?;
            }
            "--deadline" => {
                p += 1;
                c.watch_deadline_ms = Some(parse_watch_duration(
                    take(args, &mut p, "--deadline")?,
                    1_000,
                    30 * 86_400_000,
                    "--deadline",
                )?);
            }
            "--review-after" => {
                p += 1;
                c.watch_review_after_ms = Some(parse_watch_duration(
                    take(args, &mut p, "--review-after")?,
                    1_000,
                    86_400_000,
                    "--review-after",
                )?);
            }
            "--inactive-after" => {
                p += 1;
                c.watch_inactive_after_set = true;
                c.watch_inactive_after_ms = parse_watch_optional_duration(
                    take(args, &mut p, "--inactive-after")?,
                    "--inactive-after",
                )?;
            }
            "--unchanged-after" => {
                p += 1;
                c.watch_unchanged_after_set = true;
                c.watch_unchanged_after_ms = parse_watch_optional_duration(
                    take(args, &mut p, "--unchanged-after")?,
                    "--unchanged-after",
                )?;
            }
            "--no-progress-after" => {
                p += 1;
                c.watch_no_progress_after_set = true;
                c.watch_no_progress_after_ms = parse_watch_optional_duration(
                    take(args, &mut p, "--no-progress-after")?,
                    "--no-progress-after",
                )?;
            }
            "--pending-exit" => {
                p += 1;
                c.watch_pending_exit = take(args, &mut p, "--pending-exit")?
                    .parse()
                    .map_err(|_| "--pending-exit must be 1..255 except 2")?;
            }
            "--attention" => {
                p += 1;
                c.watch_attention_set = true;
                c.watch_attention = match take(args, &mut p, "--attention")? {
                    "return" => WatchAttention::Return,
                    "cache" => WatchAttention::Cache,
                    _ => return Err("--attention must be return or cache".into()),
                };
            }
            "--intent" => {
                p += 1;
                c.intent = Some(take(args, &mut p, "--intent")?.into());
            }
            "--" => {
                p += 1;
                if p >= args.len() {
                    return Err("watch requires PROGRAM after --".into());
                }
                c.cmd = args[p..].to_vec();
                p = args.len();
            }
            value if !value.starts_with('-') && c.target.is_none() => {
                c.target = Some(value.into());
                p += 1;
            }
            _ => return Err(USAGE.into()),
        }
    }
    let analyzer_count = usize::from(c.watch_analyzer_file.is_some())
        + usize::from(c.watch_analyzer_code.is_some())
        + usize::from(c.watch_clear_analyzer);
    if analyzer_count > 1 {
        return Err("choose one analyzer update".into());
    }
    if c.watch_pending_exit <= 0 || c.watch_pending_exit == 2 || c.watch_pending_exit > 255 {
        return Err("--pending-exit must be 1..255 except 2".into());
    }
    let creating = c.watch_capture.is_some() || c.watch_current || !c.cmd.is_empty();
    if creating {
        let sources = usize::from(c.watch_capture.is_some())
            + usize::from(c.watch_current)
            + usize::from(!c.cmd.is_empty());
        if c.target.is_some() || sources != 1 {
            return Err("choose exactly one --current, --capture RESULT, or probe PROGRAM".into());
        }
        if c.watch_deadline_ms.is_none() {
            return Err("creating a watch requires --deadline".into());
        }
        if c.watch_latest || c.watch_stop || c.watch_clear_analyzer {
            return Err("--latest/--stop/--clear-analyzer require WATCH_ID".into());
        }
        normalize_optional_intent(c)?;
    } else {
        if c.target.is_none() {
            return Err("watch requires WATCH_ID, --capture RESULT, or PROGRAM".into());
        }
        let lifecycle_controls = usize::from(c.watch_latest) + usize::from(c.watch_stop);
        let mutating = analyzer_count == 1 || watch_configuration_update(c);
        if lifecycle_controls > 1 || (lifecycle_controls == 1 && mutating) {
            return Err("--latest/--stop cannot be combined with watch updates".into());
        }
        if c.watch_deadline_ms.is_some() || c.intent.is_some() {
            return Err("stored watch configuration cannot be changed on resume".into());
        }
        if (lifecycle_controls > 0 || mutating) && c.watch_review_after_ms.is_some() {
            return Err("--review-after is valid only when owning/resuming a watch".into());
        }
        if c.watch_attempt_timeout_ms != 20_000 || c.watch_pending_exit != 75 {
            return Err("stored watch configuration cannot be changed on resume".into());
        }
    }
    if c.watch_no_progress_after_ms.is_some()
        && c.watch_analyzer_file.is_none()
        && c.watch_analyzer_code.is_none()
        && creating
    {
        return Err("--no-progress-after requires an analyzer".into());
    }
    Ok(())
}

fn watch_configuration_update(c: &Config) -> bool {
    c.watch_sample_every_set
        || c.watch_inactive_after_set
        || c.watch_unchanged_after_set
        || c.watch_no_progress_after_set
        || c.watch_attention_set
}

fn parse_watch_optional_duration(value: &str, option: &str) -> Result<Option<u64>, String> {
    if value == "off" {
        Ok(None)
    } else {
        parse_watch_duration(value, 1_000, 30 * 86_400_000, option).map(Some)
    }
}

fn parse_watch_duration(
    value: &str,
    minimum: u64,
    maximum: u64,
    option: &str,
) -> Result<u64, String> {
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let amount: u64 = number
        .parse()
        .map_err(|_| format!("{option} requires an integer duration"))?;
    let multiplier = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(format!("{option} must use ms, s, m, h, or d")),
    };
    let milliseconds = amount
        .checked_mul(multiplier)
        .ok_or_else(|| format!("{option} is too large"))?;
    if !(minimum..=maximum).contains(&milliseconds) {
        return Err(format!("{option} is outside its supported range"));
    }
    Ok(milliseconds)
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
            "--input" => {
                p += 1;
                c.exec_inputs
                    .push(parse_exec_input(take(args, &mut p, "--input")?)?);
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
    normalize_optional_intent(c)?;
    match (c.exec_code.is_some(), c.exec_file.is_some()) {
        (true, false) | (false, true) => {}
        _ => return Err("choose exactly one --code CODE or --file PATH".into()),
    }
    if c.target.is_none() {
        if c.exec_inputs.is_empty() {
            return Err("exec requires RESULT or at least one --input NAME=RESULT".into());
        }
    } else if !c.exec_inputs.is_empty() {
        return Err("choose one RESULT or labeled --input NAME=RESULT values".into());
    }
    validate_exec_inputs(&c.exec_inputs)?;
    if c.python.as_deref().is_some_and(|value| value.is_empty()) {
        return Err("--python PATH must not be empty".into());
    }
    Ok(())
}

fn parse_exec_input(value: &str) -> Result<ExecInput, String> {
    let (name, target) = value
        .split_once('=')
        .ok_or("--input requires NAME=RESULT")?;
    if name.is_empty() || target.is_empty() {
        return Err("--input requires non-empty NAME=RESULT".into());
    }
    Ok(ExecInput {
        name: name.to_string(),
        target: target.to_string(),
    })
}

fn validate_exec_inputs(inputs: &[ExecInput]) -> Result<(), String> {
    const MAX_EXEC_INPUTS: usize = 32;
    if inputs.len() > MAX_EXEC_INPUTS {
        return Err(format!(
            "at most {MAX_EXEC_INPUTS} --input values are allowed"
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    for input in inputs {
        let mut chars = input.name.chars();
        let valid_start = chars
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic());
        let valid_rest = chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric());
        if !valid_start || !valid_rest || input.name.len() > 64 {
            return Err(
                "each --input NAME must be a unique ASCII Python identifier of at most 64 bytes"
                    .into(),
            );
        }
        if !names.insert(input.name.as_str()) {
            return Err(format!("duplicate --input name: {}", input.name));
        }
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

fn validate_interest(interest: Option<&str>) -> Result<(), String> {
    let Some(pattern) = interest else {
        return Ok(());
    };
    if pattern.is_empty()
        || pattern.len() > MAX_QUERY_BYTES
        || pattern.chars().any(char::is_control)
    {
        return Err(format!(
            "--interest must be a non-empty, single-line Rust regex of at most {MAX_QUERY_BYTES} UTF-8 bytes"
        ));
    }
    regex::Regex::new(pattern).map_err(|error| format!("invalid --interest regex: {error}"))?;
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
            "--interest" if keywords => {
                p += 1;
                let pattern = take(args, &mut p, "--interest")?.to_string();
                if c.interest.replace(pattern).is_some() {
                    return Err("--interest may be specified only once".into());
                }
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
    fn watch_creation_and_controls_are_unambiguous() {
        let probe = parse_args(&a(&[
            "watch",
            "--deadline",
            "2h",
            "--sample-every",
            "30s",
            "--",
            "check-job",
            "123",
        ]))
        .unwrap();
        assert_eq!(probe.mode, Mode::Watch);
        assert_eq!(probe.cmd, a(&["check-job", "123"]));
        assert_eq!(probe.watch_deadline_ms, Some(7_200_000));
        assert!(probe.intent.is_none());

        let capture = parse_args(&a(&[
            "watch",
            "--capture",
            "abc",
            "--deadline",
            "1h",
            "--attention",
            "cache",
        ]))
        .unwrap();
        assert_eq!(capture.watch_capture.as_deref(), Some("abc"));
        assert_eq!(capture.watch_attention, WatchAttention::Cache);

        let current = parse_args(&a(&[
            "watch",
            "--current",
            "--deadline",
            "1h",
            "--unchanged-after",
            "10m",
        ]))
        .unwrap();
        assert!(current.watch_current);

        let latest = parse_args(&a(&["watch", "watch-123", "--latest"])).unwrap();
        assert!(latest.watch_latest);
        assert!(parse_args(&a(&["watch", "watch-123", "--latest", "--stop"])).is_err());
        assert!(parse_args(&a(&["watch", "--deadline", "2h"])).is_err());
        assert!(parse_args(&a(&["watch", "--capture", "abc", "--", "probe"])).is_err());
        assert!(
            parse_args(&a(&[
                "watch",
                "--current",
                "--capture",
                "abc",
                "--deadline",
                "1h",
            ]))
            .is_err()
        );
        let update = parse_args(&a(&[
            "watch",
            "watch-123",
            "--sample-every",
            "1s",
            "--inactive-after",
            "off",
            "--unchanged-after",
            "5m",
            "--attention",
            "return",
            "--set-analyzer-code",
            "print('{}')",
        ]))
        .unwrap();
        assert!(update.watch_sample_every_set);
        assert_eq!(update.watch_sample_every_ms, 1_000);
        assert!(update.watch_inactive_after_set);
        assert_eq!(update.watch_inactive_after_ms, None);
        assert_eq!(update.watch_unchanged_after_ms, Some(300_000));
        assert!(update.watch_attention_set);
        assert!(
            parse_args(&a(&[
                "watch",
                "watch-123",
                "--latest",
                "--sample-every",
                "1s",
            ]))
            .is_err()
        );
        assert!(
            parse_args(&a(&[
                "watch",
                "--deadline",
                "1h",
                "--no-progress-after",
                "5m",
                "--",
                "probe",
            ]))
            .is_err()
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
        assert!(parse_args(&a(&["exec", "--last", "--code", "pass"])).is_ok());
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

        let labeled = parse_args(&a(&[
            "exec",
            "--input",
            "tree=abc",
            "--input",
            "tests=def",
            "--intent",
            "aggregate captures",
            "--file",
            "-",
        ]))
        .unwrap();
        assert!(labeled.target.is_none());
        assert_eq!(
            labeled.exec_inputs,
            vec![
                ExecInput {
                    name: "tree".into(),
                    target: "abc".into(),
                },
                ExecInput {
                    name: "tests".into(),
                    target: "def".into(),
                },
            ]
        );
        assert_eq!(
            labeled.exec_file.as_deref(),
            Some(std::path::Path::new("-"))
        );
        assert!(
            parse_args(&a(&[
                "exec",
                "abc",
                "--input",
                "other=def",
                "--intent",
                "x",
                "--code",
                "pass",
            ]))
            .is_err()
        );
        assert!(
            parse_args(&a(&[
                "exec",
                "--input",
                "bad-name=abc",
                "--intent",
                "x",
                "--code",
                "pass",
            ]))
            .is_err()
        );
        assert!(
            parse_args(&a(&[
                "exec", "--input", "same=abc", "--input", "same=def", "--intent", "x", "--code",
                "pass",
            ]))
            .is_err()
        );
    }
    #[test]
    fn internal_needs_no_intent() {
        assert!(parse_args(&a(&["search", "--last", "x"])).is_ok());
        let command = parse_args(&a(&["command", "--store-dir", "/tmp/store", "abc"])).unwrap();
        assert_eq!(command.mode, Mode::Command);
        assert_eq!(command.target.as_deref(), Some("abc"));
    }

    #[test]
    fn stats_brief_accepts_bounded_multiple_results() {
        let detailed = parse_args(&a(&["stats", "--last"])).unwrap();
        assert!(!detailed.stats_brief);
        assert_eq!(detailed.target.as_deref(), Some("--last"));

        let brief = parse_args(&a(&[
            "stats",
            "one",
            "--brief",
            "--store-dir",
            "/tmp/store",
            "two",
        ]))
        .unwrap();
        assert!(brief.stats_brief);
        assert_eq!(brief.stats_targets, ["one", "two"]);
        assert_eq!(
            brief.store_dir.as_deref(),
            Some(std::path::Path::new("/tmp/store"))
        );

        assert!(parse_args(&a(&["stats", "--brief"])).is_err());
        assert!(parse_args(&a(&["stats", "one", "two"])).is_err());
        assert!(parse_args(&a(&["stats", "--brief", "--brief", "one"])).is_err());

        let mut oversized = vec!["stats".to_string(), "--brief".to_string()];
        oversized.extend((0..33).map(|index| format!("result-{index}")));
        assert!(parse_args(&oversized).is_err());
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

    #[test]
    fn interest_regex_is_validated_before_program_arguments() {
        let config = parse_args(&a(&[
            "auto",
            "--intent",
            "inspect selected diagnostics",
            "--interest",
            "(?i)error|failed",
            "--",
            "program",
            "--interest",
            "child-value",
        ]))
        .unwrap();
        assert_eq!(config.interest.as_deref(), Some("(?i)error|failed"));
        assert_eq!(config.cmd, a(&["program", "--interest", "child-value"]));

        let invalid = parse_args(&a(&[
            "--intent",
            "inspect selected diagnostics",
            "--interest",
            "[",
            "--",
            "program",
        ]))
        .unwrap_err();
        assert!(invalid.contains("invalid --interest regex"));

        let duplicate = parse_args(&a(&[
            "--intent",
            "inspect selected diagnostics",
            "--interest",
            "error",
            "--interest",
            "failed",
            "--",
            "program",
        ]))
        .unwrap_err();
        assert!(duplicate.contains("only once"));
    }
}
