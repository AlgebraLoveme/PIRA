mod artifact;
mod authentication;
mod bookmarks;
mod cli;
mod harness;
mod lifecycle;
mod native;
mod profile;
mod remote;
mod retention;
mod runtime;
mod storage;
mod subteam;

use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::fs::{self, File, OpenOptions};

use std::path::{Path, PathBuf};

#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn cancel_worker(_: libc::c_int) {
    use std::sync::atomic::Ordering;
    CANCELLED.store(true, Ordering::SeqCst);
}

const WORKER_POLICY_VERSION: u64 = 21;
const COORDINATOR_POLICY_VERSION: u64 = 22;

pub const POLICY: &str = include_str!("docs/main.md");
const IMPLEMENTATION_POLICY: &str = include_str!("docs/implementation.md");
const REVIEW_POLICY: &str = include_str!("docs/review.md");
const WRITING_POLICY: &str = include_str!("../../../modules/SCIENTIFIC_WRITING.md");
const WRITING_APPLICATION: &str = include_str!("docs/writing_application.md");
const FIGURE_POLICY: &str = include_str!("../../../modules/PUBLIC_FIGURE_STYLE.md");

struct Options {
    cwd: PathBuf,
    store: PathBuf,
    model: String,
    effort: String,
    service_tier: profile::service_tier::Tier,
    profile_sources: Value,
    execution: profile::Execution,
    task: String,
    label: Option<String>,
    output: String,
    navigation: String,
    completion_gate: String,
    inject_review: bool,
    inject_implement: bool,
    inject_fig: bool,
    inject_write: bool,
    allow_subteam: bool,
    authentication: authentication::Authentication,
    ctx_store: PathBuf,
    dec_store: PathBuf,
    dec_workspace: Option<PathBuf>,
    contract: artifact::Contract,
    temp_workspace: Option<PathBuf>,
}

// Children may intentionally outlive their coordinator. A closed diagnostic pipe
// must not panic before managed lifecycle state is finalized.
fn notice(message: std::fmt::Arguments<'_>) {
    write_notice(&mut std::io::stderr().lock(), message);
}

fn write_notice(output: &mut impl std::io::Write, message: std::fmt::Arguments<'_>) {
    let _ = writeln!(output, "{message}");
}

fn completion_gate(value: Option<String>) -> Result<String, String> {
    value
        .filter(|s| !s.trim().is_empty() && !s.contains('\0'))
        .ok_or_else(|| "provide a nonempty --completion-gate for the assignment".into())
}

fn tool_store(kind: &str) -> Result<PathBuf, String> {
    let key = if kind == "ctx" {
        "PIRA_CTX_STORE_DIR"
    } else {
        "PIRA_DEC_STORE_DIR"
    };
    let path = storage::configured_root(key, if kind == "ctx" { "ctx" } else { "decision" })?;
    Ok(if path.is_absolute() {
        path
    } else {
        // Resolve both overrides before changing to the worker's cwd.
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("provide {name} only once"));
    }
    *slot = Some(value);
    Ok(())
}

fn task_input(
    task: Option<String>,
    file: Option<PathBuf>,
    default: Option<&str>,
) -> Result<String, String> {
    if task.is_some() && file.is_some() {
        return Err("--task/positional task and --task-file are mutually exclusive".into());
    }
    let text = match file {
        Some(path) => fs::read_to_string(path).map_err(|e| format!("read task: {e}"))?,
        None => task
            .or_else(|| default.map(str::to_owned))
            .ok_or("missing --task or --task-file")?,
    };
    if text.trim().is_empty() {
        return Err("task must be nonempty".into());
    }
    Ok(text)
}

fn parse(args: &[String]) -> Result<Options, String> {
    parse_using(args, profile::load)
}

fn parse_using(
    args: &[String],
    parent: impl FnOnce() -> Result<Value, String>,
) -> Result<Options, String> {
    if args.first().map(String::as_str) != Some("run") {
        return Err("expected run; use pira_team help".into());
    }
    let mut cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut store = None;
    let (mut model, mut effort, mut task, mut task_file) = (None, None, None, None);
    let mut service_tier = None;
    let mut output = "artifact".to_string();
    let mut format = "auto".to_string();
    let (mut schema, mut columns) = (None, None);
    let mut navigation = "nav".to_string();
    let mut gate = None;
    let mut label = None;
    let mut inject_review = None;
    let mut inject_implement = None;
    let mut inject_fig = None;
    let mut inject_write = None;
    let mut allow_subteam = None;
    let mut code_review = None;
    let mut allow_fix = None;
    let mut args = cli::tokens(&args[1..]);
    while let Some(token) = args.next() {
        let arg = token.raw;
        if arg == "--" {
            let value = args.next().ok_or("missing task after --")?.raw;
            set_once(&mut task, value.to_owned(), "task")?;
            if args.next().is_some() {
                return Err("provide exactly one task".into());
            }
            break;
        }
        if !arg.starts_with('-') {
            set_once(&mut task, arg.clone(), "task")?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--task=") {
            set_once(&mut task, value.to_owned(), "task")?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--completion-gate=") {
            set_once(&mut gate, value.to_owned(), "--completion-gate")?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--label=") {
            set_once(&mut label, value.to_owned(), "--label")?;
            continue;
        }
        let boolean = match token.flag {
            Some(cli::Flag::InjectReview) => Some((&mut inject_review, "--inject-review")),
            Some(cli::Flag::InjectImplement) => Some((&mut inject_implement, "--inject-implement")),
            Some(cli::Flag::InjectWrite) => Some((&mut inject_write, "--inject-write")),
            Some(cli::Flag::InjectFig) => Some((&mut inject_fig, "--inject-fig")),
            Some(cli::Flag::AllowSubteam) => Some((&mut allow_subteam, "--allow-subteam")),
            Some(cli::Flag::AllowFix) => Some((&mut allow_fix, "--allow-fix")),
            Some(cli::Flag::CodeReview) => Some((&mut code_review, "--code-review")),
            _ => None,
        };
        if let Some((slot, name)) = boolean {
            set_once(slot, true, name)?;
            continue;
        }
        let value = token
            .value
            .ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--label" => set_once(&mut label, value.to_owned(), "--label")?,
            "--completion-gate" => set_once(&mut gate, value.to_owned(), "--completion-gate")?,
            "--cwd" => cwd = PathBuf::from(value),
            "--store" => store = Some(PathBuf::from(value)),
            "--model" => model = Some(value.to_owned()),
            "--effort" => effort = Some(value.to_owned()),
            "--service-tier" => set_once(
                &mut service_tier,
                profile::service_tier::Tier::parse(value)?,
                "--service-tier",
            )?,
            "--task" => set_once(&mut task, value.to_owned(), "task")?,
            "--task-file" => set_once(&mut task_file, PathBuf::from(value), "--task-file")?,
            "--timeout" => {
                return Err("--timeout is no longer supported; use interrupt to stop a run".into());
            }
            "--output" => output = value.to_owned(),
            "--navigation" => navigation = value.to_owned(),
            "--format" => format = value.to_owned(),
            "--schema" => schema = Some(PathBuf::from(value)),
            "--columns" => columns = Some(value.to_owned()),
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    if allow_fix.is_some() || code_review.is_some() {
        eprintln!(
            "pira_team: --code-review/--allow-fix are deprecated; task instructions control review or implementation"
        );
    }
    if navigation == "shell" {
        eprintln!(
            "pira_team: --navigation shell is deprecated; normal ctx/nav/dec guidance is always injected"
        );
    }
    if let Some(text) = &label {
        bookmarks::validate_label(text)?;
    }
    let completion_gate = completion_gate(gate)?;
    let task = task_input(task, task_file, None)?;
    let store = store.map(Ok).unwrap_or_else(storage::default_root)?;
    let (model, effort, profile_sources, execution) = profile::resolve_with_store(
        model,
        effort,
        parent().map_err(|e| format!("cannot inherit verified caller execution/profile: {e}"))?,
        &store,
    )?;
    if task.trim().is_empty() {
        return Err("task must be nonempty".into());
    }
    if !["answer", "artifact"].contains(&output.as_str()) {
        return Err("--output must be answer or artifact".into());
    }
    if !["nav", "shell"].contains(&navigation.as_str()) {
        return Err("--navigation must be nav or shell".into());
    }
    let cwd = cwd
        .canonicalize()
        .map_err(|e| format!("working directory: {e}"))?;
    if !cwd.is_dir() {
        return Err("working directory is not a directory".into());
    }
    Ok(Options {
        temp_workspace: None,
        ctx_store: tool_store("ctx")?,
        dec_store: tool_store("dec")?,
        dec_workspace: Some(lifecycle::decision_workspace(
            &std::env::current_dir().map_err(|e| e.to_string())?,
        )?),
        cwd,
        store,
        model,
        effort,
        profile_sources,
        service_tier: service_tier.unwrap_or(profile::service_tier::Tier::Standard),
        execution,
        task,
        label,
        output,
        navigation,
        completion_gate,
        inject_review: inject_review.unwrap_or(false),
        inject_implement: inject_implement.unwrap_or(false),
        inject_fig: inject_fig.unwrap_or(false),
        inject_write: inject_write.unwrap_or(false),
        allow_subteam: allow_subteam.unwrap_or(false),
        authentication: authentication::Authentication::Caller,
        contract: artifact::Contract::new(format, schema.as_deref(), columns.as_deref())?,
    })
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder.create(path)
}

fn create(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn worker_context_environment(
    options: &Options,
    run: &Path,
    dir: &Path,
    handoff: &Path,
) -> Vec<(&'static str, PathBuf)> {
    let mut environment = vec![
        ("PIRA_CTX_STORE_DIR", options.ctx_store.clone()),
        ("PIRA_DEC_STORE_DIR", options.dec_store.clone()),
    ];
    if let Some(workspace) = &options.dec_workspace {
        environment.push(("PIRA_DEC_WORKSPACE_DIR", workspace.clone()));
    }
    environment.extend([
        ("PIRA_TEAM_HANDOFF", handoff.to_owned()),
        ("PIRA_TEAM_CONTEXT", run.to_owned()),
        ("PIRA_TEAM_ATTEMPT", dir.to_owned()),
    ]);
    environment
}

fn worker_temp_environment(options: &Options) -> [(&'static str, PathBuf); 4] {
    let scratch = options
        .temp_workspace
        .as_ref()
        .expect("prepared worker temp");
    [
        ("TMPDIR", scratch.clone()),
        ("TMP", scratch.clone()),
        ("TEMP", scratch.clone()),
        // zsh uses TMPPREFIX rather than TMPDIR for heredocs.
        ("TMPPREFIX", scratch.join("zsh")),
    ]
}

/// Execute the Team CLI and return its process exit code.
pub fn run() -> i32 {
    if let Some(code) = harness::Harness::shell_entry() {
        return code;
    }
    #[cfg(unix)]
    unsafe {
        libc::signal(
            libc::SIGTERM,
            cancel_worker as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            cancel_worker as *const () as libc::sighandler_t,
        );
    }
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(outcome) = cli::help::dispatch(&args) {
        return match outcome {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(error) => {
                notice(format_args!("pira_team: {error}"));
                1
            }
        };
    }
    if args == ["--version"] {
        println!("pira_team {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if args.first().map(String::as_str) == Some("__cleanup") {
        return retention::background(&args);
    }
    if let Some(outcome) = remote::dispatch(&mut args) {
        return match outcome {
            Ok(()) => 0,
            Err(error) => {
                notice(format_args!("pira_team: {error}"));
                1
            }
        };
    }
    if let Err(error) = subteam::authorize(&args) {
        notice(format_args!("pira_team: {error}"));
        return 1;
    }
    if matches!(args[0].as_str(), "run" | "resume")
        && let Ok(Some(runtime)) = subteam::caller_runtime()
    {
        match native::process::isolate_launcher(&args, &runtime) {
            Ok(Some(code)) => return code,
            Ok(None) => {}
            Err(error) => {
                notice(format_args!("pira_team: {error}"));
                return 1;
            }
        }
    }
    let outcome = if args[0] == "wait" {
        storage::wait(&args)
    } else if matches!(args[0].as_str(), "read" | "path") {
        storage::access(&args)
    } else if matches!(
        args[0].as_str(),
        "bookmark" | "unbookmark" | "bookmarks" | "search"
    ) {
        bookmarks::command(&args)
    } else if args[0] == "cleanup" {
        retention::command(&args)
    } else if args[0] == "config" {
        profile::defaults::command(&args, storage::default_root)
    } else if matches!(args[0].as_str(), "resume" | "steer" | "interrupt" | "tier") {
        lifecycle::existing(&args)
    } else {
        parse(&args).and_then(lifecycle::launch)
    };
    match outcome {
        Ok(()) => 0,
        Err(error) => {
            notice(format_args!("pira_team: {error}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::codex::command;
    use std::process::Command;

    pub(crate) fn temp_path(prefix: &str) -> PathBuf {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).unwrap();
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{:032x}",
            std::process::id(),
            u128::from_ne_bytes(bytes)
        ))
    }

    pub(super) fn options(extra: &[&str]) -> Result<Options, String> {
        let mut args = vec![
            "run",
            "--model",
            "test-model",
            "--effort",
            "high",
            "--completion-gate",
            "Fixture complete",
        ];
        args.extend(extra);
        parse_using(
            &args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
            || {
                Ok(json!({
            "approval_policy":"never", "cwd":std::env::current_dir().unwrap(),
            "sandbox_policy":{"type":"workspace-write","writable_roots":[],"network_access":false,"exclude_tmpdir_env_var":true,"exclude_slash_tmp":true}}))
            },
        ).map(|mut options| {
            options.temp_workspace = Some(std::env::temp_dir().join("team-command-fixture"));
            options
        })
    }
    #[test]
    fn writing_guidance_is_independent_and_rejects_duplicate_flags() {
        assert!(!options(&["task"]).unwrap().inject_write);
        for mask in 0..16 {
            let mut args = vec!["task"];
            for (bit, flag) in [
                "--inject-review",
                "--inject-implement",
                "--inject-fig",
                "--inject-write",
            ]
            .iter()
            .enumerate()
            {
                if mask & (1 << bit) != 0 {
                    args.push(flag);
                }
            }
            let options = options(&args).unwrap();
            assert_eq!(options.inject_review, mask & 1 != 0);
            assert_eq!(options.inject_implement, mask & 2 != 0);
            assert_eq!(options.inject_fig, mask & 4 != 0);
            assert_eq!(options.inject_write, mask & 8 != 0);
        }
        assert_eq!(
            options(&["task", "--inject-write", "--inject-write"])
                .err()
                .unwrap(),
            "provide --inject-write only once"
        );
        assert!(cli::help::topic("run").unwrap().contains("--inject-write"));
        assert_eq!(
            WRITING_POLICY,
            include_str!("../../../modules/SCIENTIFIC_WRITING.md")
        );
    }

    #[test]
    fn help_run_examples_keep_documented_defaults() {
        let review = options(&["--task", "Review src/parser.rs", "--inject-review"]).unwrap();
        assert_eq!(review.task, "Review src/parser.rs");
        assert!(review.inject_review);
        assert!(!review.inject_implement);
        assert_eq!(review.output, "artifact");
        assert_eq!(review.contract.format, "auto");
        assert_eq!(review.service_tier.name(), "standard");
        assert_eq!(review.navigation, "nav");
        assert!(!review.allow_subteam);
        let dash_task = options(&["--", "-task starting with a dash"]).unwrap();
        assert_eq!(dash_task.task, "-task starting with a dash");
    }

    #[test]
    fn help_looking_task_values_keep_runtime_parsing() {
        for args in [
            vec!["--task", "--help"],
            vec!["--task", "-h"],
            vec!["--task=--help"],
            vec!["--", "--help"],
        ] {
            let expected = if args.contains(&"-h") { "-h" } else { "--help" };
            assert_eq!(options(&args).unwrap().task, expected);
        }
    }

    #[test]
    fn safety_review_is_always_injected_independent_of_implementation_guidance() {
        let safety = POLICY
            .split("## Safety")
            .nth(1)
            .unwrap()
            .split("## Handoff")
            .next()
            .unwrap();
        for expected in [
            "task start",
            "before high-impact actions",
            "exact prefix `Safety:`",
            "scope/blast radius",
            "destructive risk",
            "secrets/privacy",
            "rollback",
            "needs_decision to the main",
            "does not authorize unrelated actions",
        ] {
            assert!(safety.contains(expected), "{expected}");
        }
        assert_eq!(WORKER_POLICY_VERSION, 21);
        assert!(!IMPLEMENTATION_POLICY.contains("exact prefix `Safety:`"));
    }

    #[test]
    fn rejects_ambiguous_and_write_interfaces() {
        for args in [
            &["a", "b"][..],
            &["--write", "file", "task"],
            &["--timeout", "0", "task"],
            &["--task-file", "x", "task"],
        ] {
            assert!(options(args).is_err());
        }
        assert_eq!(options(&["--", "-task"]).unwrap().task, "-task");
        assert_eq!(options(&["--task", "-task"]).unwrap().task, "-task");
        assert_eq!(options(&["--task=-task"]).unwrap().task, "-task");
        assert!(options(&["--task", "one", "two"]).is_err());
        assert!(options(&["--task-file", "one", "--task-file", "two"]).is_err());
    }

    include!("tests/project_trust.rs");
}

// Preserve the native-CI test selector without retaining a production backend module.
#[cfg(test)]
mod backend {
    mod tests {
        #[test]
        #[ignore = "requires the installed native Codex CLI; no auth or model calls"]
        fn installed_native_schema_supports_team_requests() {
            crate::harness::codex::installed_native_schema_supports_team_requests();
        }
    }
}
