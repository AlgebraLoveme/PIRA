mod app_server;
mod artifact;
mod authentication;
mod backend;
mod bookmarks;
mod build_roots;
mod lifecycle;
mod native;
mod process;
mod profile;
mod remote;
mod service_tier;
mod storage;
mod subteam;

use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use std::time::{SystemTime, UNIX_EPOCH};

static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn cancel_worker(_: libc::c_int) {
    use std::sync::atomic::Ordering;
    CANCELLED.store(true, Ordering::SeqCst);
}

const WORKER_POLICY_VERSION: u64 = 20;
const COORDINATOR_POLICY_VERSION: u64 = 22;

pub const POLICY: &str = include_str!("main.md");
const IMPLEMENTATION_POLICY: &str = include_str!("implementation.md");
const REVIEW_POLICY: &str = include_str!("review.md");
const WRITING_POLICY: &str = include_str!("../../../modules/SCIENTIFIC_WRITING.md");
const WRITING_APPLICATION: &str = include_str!("writing_application.md");
const FIGURE_POLICY: &str = include_str!("../../../modules/PUBLIC_FIGURE_STYLE.md");
const HELP: &str = include_str!("help.md");

struct Options {
    cwd: PathBuf,
    store: PathBuf,
    model: String,
    effort: String,
    service_tier: service_tier::Tier,
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
}

// Children may intentionally outlive their coordinator. A closed diagnostic pipe
// must not panic before managed lifecycle state is finalized.
fn notice(message: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

fn completion_gate(value: Option<String>) -> Result<String, String> {
    value
        .filter(|s| !s.trim().is_empty() && !s.contains('\0'))
        .ok_or_else(|| "provide a nonempty --completion-gate for the assignment".into())
}

fn tool_store(kind: &str, cwd: &Path) -> Result<PathBuf, String> {
    let key = if kind == "ctx" {
        "PIRA_CTX_STORE_DIR"
    } else {
        "PIRA_DEC_STORE_DIR"
    };
    let path = storage::configured_root(key, if kind == "ctx" { "ctx" } else { "decision" })?;
    Ok(if path.is_absolute() {
        path
    } else if kind == "dec" {
        // Match the invoking Dec CLI before changing to the worker's cwd.
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    } else {
        cwd.join(path)
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
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            let value = args.next().ok_or("missing task after --")?;
            set_once(&mut task, value.clone(), "task")?;
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
        if arg == "--inject-review" {
            set_once(&mut inject_review, true, "--inject-review")?;
            continue;
        }
        if arg == "--inject-implement" {
            set_once(&mut inject_implement, true, "--inject-implement")?;
            continue;
        }
        if arg == "--inject-write" {
            set_once(&mut inject_write, true, "--inject-write")?;
            continue;
        }
        if arg == "--inject-fig" {
            set_once(&mut inject_fig, true, "--inject-fig")?;
            continue;
        }
        if arg == "--allow-subteam" {
            set_once(&mut allow_subteam, true, "--allow-subteam")?;
            continue;
        }
        if arg == "--allow-fix" {
            set_once(&mut allow_fix, true, "--allow-fix")?;
            continue;
        }
        if arg == "--code-review" {
            set_once(&mut code_review, true, "--code-review")?;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--label" => set_once(&mut label, value.clone(), "--label")?,
            "--completion-gate" => set_once(&mut gate, value.clone(), "--completion-gate")?,
            "--cwd" => cwd = PathBuf::from(value),
            "--store" => store = Some(PathBuf::from(value)),
            "--model" => model = Some(value.clone()),
            "--effort" => effort = Some(value.clone()),
            "--service-tier" => set_once(
                &mut service_tier,
                service_tier::Tier::parse(value)?,
                "--service-tier",
            )?,
            "--task" => set_once(&mut task, value.clone(), "task")?,
            "--task-file" => set_once(&mut task_file, PathBuf::from(value), "--task-file")?,
            "--timeout" => {
                return Err("--timeout is no longer supported; use interrupt to stop a run".into());
            }
            "--output" => output = value.clone(),
            "--navigation" => navigation = value.clone(),
            "--format" => format = value.clone(),
            "--schema" => schema = Some(PathBuf::from(value)),
            "--columns" => columns = Some(value.clone()),
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
        ctx_store: tool_store("ctx", &cwd)?,
        dec_store: tool_store("dec", &cwd)?,
        dec_workspace: Some(lifecycle::decision_workspace(
            &std::env::current_dir().map_err(|e| e.to_string())?,
        )?),
        cwd,
        store,
        model,
        effort,
        profile_sources,
        service_tier: service_tier.unwrap_or(service_tier::Tier::Standard),
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

fn prepare_store(root: &Path) -> Result<PathBuf, String> {
    // Each run is exclusively created, even with concurrent callers; never reuse a prior log.
    if let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("create store parent: {e}"))?;
    }
    match private_dir(root) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("create store: {e}")),
    }
    let metadata = fs::symlink_metadata(root).map_err(|e| format!("inspect store: {e}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("store must be a real directory, not a symlink".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err("store must be user-owned and not writable by group/others; choose a private --store".into());
        }
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::geteuid() };
        for ancestor in root.ancestors() {
            let meta =
                fs::metadata(ancestor).map_err(|e| format!("inspect store ancestor: {e}"))?;
            let trusted_owner = meta.uid() == uid || meta.uid() == 0;
            let replaceable = meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0;
            if !trusted_owner || replaceable {
                return Err(format!(
                    "unsafe store ancestor {}; choose a private --store",
                    ancestor.display()
                ));
            }
        }
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    for attempt in 0..100 {
        let dir = root.join(format!("{timestamp}-{}-{attempt}", std::process::id()));
        match private_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("create run: {e}")),
        }
    }
    Err("cannot allocate unique run directory".into())
}

fn create(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

struct IsolatedHome(PathBuf);

impl IsolatedHome {
    fn new(run: &Path, authentication: &authentication::Authentication) -> Result<Self, String> {
        let path = run.join("codex-home");
        match private_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                lifecycle::private_path(&path, true)?
            }
            Err(e) => return Err(format!("create isolated Codex home: {e}")),
        }
        // Remove only a stale launcher-owned link, never its credential target.
        let auth = path.join("auth.json");
        if fs::symlink_metadata(&auth).is_ok() {
            fs::remove_file(auth).map_err(|e| format!("remove stale auth link: {e}"))?;
        }
        let home = Self(path);
        if authentication.uses_api_key() {
            return Ok(home);
        }
        let source = match authentication.file_source() {
            Some(source) => Some(source.to_owned()),
            None if subteam::caller_run()?.is_none() => authentication::caller_file()?,
            None => None,
        };
        if let Some(source) = source {
            // Link authentication only, never configuration, skills or history.
            #[cfg(unix)]
            std::os::unix::fs::symlink(&source, home.0.join("auth.json"))
                .map_err(|_| "link selected Codex authentication")?;
            #[cfg(windows)]
            fs::hard_link(&source, home.0.join("auth.json"))
                .map_err(|_| "link selected Codex authentication; use CODEX_API_KEY if linking is unavailable")?;
        }
        Ok(home)
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        // Retain only the worker's session; detach authentication after every revision.
        if let Err(error) = fs::remove_file(self.0.join("auth.json"))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("pira_team: detach worker authentication: {error}");
        }
    }
}

fn command(options: &Options, run: &Path, dir: &Path, handoff: &Path) -> Command {
    let mut cmd = Command::new(options.authentication.program());
    options.authentication.apply(&mut cmd);
    cmd.args(["app-server", "--strict-config", "--stdio"]);
    for setting in backend::contract()["config"].as_array().unwrap() {
        cmd.arg("-c").arg(setting.as_str().unwrap());
    }
    for (key, value) in options.execution.config().as_object().unwrap() {
        for (name, value) in value.as_object().unwrap() {
            cmd.arg("-c").arg(format!("{key}.{name}={value}"));
        }
    }
    for setting in [
        format!("sandbox_mode={}", json!(options.execution.mode)),
        "approval_policy=\"never\"".to_owned(),
        "features.fast_mode=true".to_owned(),
        format!("model={}", json!(options.model)),
        format!("model_reasoning_effort={}", json!(options.effort)),
        format!("model_instructions_file={}", json!(dir.join("policy.md"))),
        format!(
            "projects={{{}={{trust_level=\"untrusted\"}}}}",
            // CLI keys split on every dot; only the value supports TOML quoted keys.
            // JSON strings are TOML basic strings except for literal DEL.
            json!(options.cwd.to_string_lossy())
                .to_string()
                .replace('\u{7f}', "\\u007f")
        ),
    ] {
        cmd.arg("-c").arg(setting);
    }
    for (key, value) in [
        ("PIRA_CTX_STORE_DIR", &options.ctx_store),
        ("PIRA_DEC_STORE_DIR", &options.dec_store),
    ] {
        cmd.arg("-c").arg(format!(
            "shell_environment_policy.set.{key}={}",
            json!(value)
        ));
    }
    if let Some(workspace) = &options.dec_workspace {
        cmd.arg("-c").arg(format!(
            "shell_environment_policy.set.PIRA_DEC_WORKSPACE_DIR={}",
            json!(workspace)
        ));
    }
    cmd.arg("-c").arg(format!(
        "shell_environment_policy.set.PIRA_TEAM_HANDOFF={}",
        json!(handoff)
    ));
    cmd.arg("-c").arg(format!(
        "shell_environment_policy.set.PIRA_TEAM_CONTEXT={}",
        json!(run)
    ));
    // Keep unknown task configuration without serializing its values into argv/logs.
    // Backend authentication remains available to Codex, not to worker shells.
    cmd.arg("-c")
        .arg("shell_environment_policy.ignore_default_excludes=true");
    cmd.arg("-c").arg(format!(
        "shell_environment_policy.exclude={}",
        json!([
            "CODEX_HOME",
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "CLAUDE_CODE_SESSION_ID",
            "PIRA_CTX_THREAD_ID",
            "PIRA_TEAM_DIR",
            "PIRA_TEAM_HANDOFF",
            "PIRA_TEAM_CHILD",
            "PIRA_TEAM_CONTEXT",
            "PIRA_CTX_STORE_DIR",
            "PIRA_DEC_STORE_DIR",
            "PIRA_DEC_WORKSPACE_DIR",
            "CODEX_API_KEY",
            "OPENAI_API_KEY",
            "OPENAI_ADMIN_KEY",
            "*_API_KEY",
            "*_TOKEN",
            "*_SECRET",
            "*_SECRET_KEY",
            "*_PASSWORD",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "SSH_AUTH_SOCK",
            "SSH_AGENT_PID",
        ])
    ));
    for key in [
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "PIRA_CTX_THREAD_ID",
        "CLAUDE_CODE_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }
    // Worker temporary defaults must match the managed writable root, not ambient host paths.
    let scratch = run.join("scratch");
    for (key, path) in [
        ("TMPDIR", scratch.clone()),
        ("TMP", scratch.clone()),
        ("TEMP", scratch.clone()),
        // zsh uses TMPPREFIX rather than TMPDIR for heredocs.
        ("TMPPREFIX", scratch.join("zsh")),
    ] {
        cmd.env(key, &path);
        cmd.arg("-c").arg(format!(
            "shell_environment_policy.set.{key}={}",
            json!(path)
        ));
    }
    cmd.env("PIRA_TEAM_CHILD", "1");
    cmd.env("CODEX_HOME", run.join("codex-home"));
    cmd.current_dir(dir).stdin(Stdio::piped());
    cmd
}

fn worker_path(launcher: &Path, inherited: Option<&std::ffi::OsStr>) -> Result<String, String> {
    let directory = launcher.parent().ok_or("Team launcher has no directory")?;
    let mut paths = vec![directory.to_path_buf()];
    if let Some(inherited) = inherited {
        paths.extend(std::env::split_paths(inherited).filter(|path| path != directory));
    }
    std::env::join_paths(paths)
        .map_err(|_| "cannot represent Team launcher directory in worker PATH")?
        .into_string()
        .map_err(|_| "worker PATH must be UTF-8 for native shell configuration".into())
}

fn set_worker_launcher(command: &mut Command) -> Result<(), String> {
    let launcher = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|_| "cannot resolve current Team launcher for worker shells")?;
    let path = worker_path(&launcher, std::env::var_os("PATH").as_deref())?;
    // Match executable-bound child authentication without changing the backend
    // process PATH or its independently retained executable/provider selection.
    command.arg("-c").arg(format!(
        "shell_environment_policy.set.PATH={}",
        json!(path).to_string().replace('\u{7f}', "\\u007f")
    ));
    Ok(())
}

/// Execute the Team CLI and return its process exit code.
pub fn run() -> i32 {
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
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        print!("{HELP}");
        return 0;
    }
    if args == ["--version"] {
        println!("pira_team {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if let Some(outcome) = remote::dispatch(&mut args) {
        return match outcome {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("pira_team: {error}");
                1
            }
        };
    }
    if let Err(error) = subteam::authorize(&args) {
        eprintln!("pira_team: {error}");
        return 1;
    }
    if matches!(args[0].as_str(), "run" | "resume")
        && let Ok(Some(runtime)) = subteam::caller_runtime()
    {
        match process::isolate_launcher(&args, &runtime) {
            Ok(Some(code)) => return code,
            Ok(None) => {}
            Err(error) => {
                eprintln!("pira_team: {error}");
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
            eprintln!("pira_team: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_path_prefers_parent_launcher_and_preserves_ambient_order() {
        let launcher = Path::new("/staged team/bin/pira_team");
        let inherited = std::env::join_paths([
            Path::new("/ambient/bin"),
            Path::new("/other tools/bin"),
            Path::new("/staged team/bin"),
        ])
        .unwrap();
        let path = worker_path(launcher, Some(&inherited)).unwrap();
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            [
                PathBuf::from("/staged team/bin"),
                PathBuf::from("/ambient/bin"),
                PathBuf::from("/other tools/bin"),
            ]
        );
        assert_eq!(
            std::env::split_paths(&worker_path(launcher, None).unwrap()).collect::<Vec<_>>(),
            [PathBuf::from("/staged team/bin")]
        );
    }

    #[test]
    fn worker_launcher_setting_does_not_redirect_backend_process() {
        let backend = Path::new("/ambient/bin/selected-codex");
        let mut command = Command::new(backend);
        command.env("UNCHANGED_TOOL_SETTING", "fixture");
        set_worker_launcher(&mut command).unwrap();
        assert_eq!(command.get_program(), backend.as_os_str());
        assert!(!command.get_envs().any(|(name, _)| name == "PATH"));
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == "UNCHANGED_TOOL_SETTING"
                    && value == Some(std::ffi::OsStr::new("fixture")))
        );
        let setting = command
            .get_args()
            .find_map(|arg| {
                arg.to_str()?
                    .strip_prefix("shell_environment_policy.set.PATH=")
            })
            .unwrap();
        let path: String = serde_json::from_str(setting).unwrap();
        assert_eq!(
            std::env::split_paths(&path).next().unwrap(),
            std::env::current_exe()
                .unwrap()
                .canonicalize()
                .unwrap()
                .parent()
                .unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn worker_path_executes_staged_launcher_instead_of_different_ambient_copy() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "pira-team-launcher-path-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        private_dir(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let staged = root.join("staged parent");
        let ambient = root.join("ambient launcher");
        for directory in [&staged, &ambient] {
            private_dir(directory).unwrap();
            let program = directory.join("pira_team");
            fs::write(&program, "#!/bin/sh\nprintf '%s' \"$0\"\n").unwrap();
            fs::set_permissions(program, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let selected = |path: &std::ffi::OsStr| {
            let output = Command::new("pira_team")
                .env("PATH", path)
                .output()
                .unwrap();
            assert!(output.status.success());
            PathBuf::from(String::from_utf8(output.stdout).unwrap())
                .canonicalize()
                .unwrap()
        };
        let inherited = std::env::join_paths([&ambient]).unwrap();
        assert_eq!(selected(&inherited), ambient.join("pira_team"));
        let path = worker_path(&staged.join("pira_team"), Some(&inherited)).unwrap();
        assert_eq!(
            selected(std::ffi::OsStr::new(&path)),
            staged.join("pira_team")
        );
    }

    #[cfg(unix)]
    #[test]
    fn worker_path_rejects_unrepresentable_native_shell_environment() {
        use std::os::unix::ffi::OsStringExt;
        let inherited = std::ffi::OsString::from_vec(vec![b'/', 0xff]);
        assert!(
            worker_path(Path::new("/staged/pira_team"), Some(&inherited))
                .unwrap_err()
                .contains("UTF-8")
        );
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
        )
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
        assert!(HELP.contains("--inject-write"));
        assert_eq!(
            WRITING_POLICY,
            include_str!("../../../modules/SCIENTIFIC_WRITING.md")
        );
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
        assert_eq!(WORKER_POLICY_VERSION, 20);
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
    #[test]
    fn command_keeps_session_home_separate_from_revision_logs() {
        let opt = options(&["review"]).unwrap();
        let run = Path::new("/tmp/worker run");
        for dir in [run.to_owned(), run.join("revisions/000002/repair")] {
            let cmd = command(&opt, run, &dir, &dir.join("artifacts/handoff"));
            let home = cmd
                .get_envs()
                .find(|(key, _)| *key == "CODEX_HOME")
                .unwrap()
                .1
                .unwrap();
            assert_eq!(home, run.join("codex-home").as_os_str());
            for key in ["TMPDIR", "TMP", "TEMP", "TMPPREFIX"] {
                let path = if key == "TMPPREFIX" {
                    // Match native component separators in the raw environment value;
                    // joining "scratch/zsh" retains the slash on Windows.
                    run.join("scratch").join("zsh")
                } else {
                    run.join("scratch")
                };
                let actual = cmd
                    .get_envs()
                    .find(|(name, _)| *name == key)
                    .and_then(|(_, value)| value);
                assert_eq!(actual, Some(path.as_os_str()), "backend environment {key}");
                let setting = format!("shell_environment_policy.set.{key}={}", json!(path));
                assert!(cmd.get_args().any(|arg| arg == setting.as_str()));
            }
            assert_eq!(cmd.get_current_dir(), Some(dir.as_path()));
            let policy = format!("model_instructions_file={}", json!(dir.join("policy.md")));
            assert!(cmd.get_args().any(|arg| arg == policy.as_str()));
        }
    }

    #[test]
    fn launch_always_isolates_instructions_and_permissions() {
        let opt = options(&["review"]).unwrap();
        let cmd = command(
            &opt,
            Path::new("/tmp/worker run"),
            Path::new("/tmp/policy run"),
            Path::new("/tmp/policy run/artifacts/handoff"),
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for expected in [
            "app-server",
            "--strict-config",
            "sandbox_mode=\"workspace-write\"",
            "project_doc_max_bytes=0",
            "approval_policy=\"never\"",
            "agents.enabled=false",
            "features.multi_agent=false",
            "shell_environment_policy.set.PIRA_TEAM_CHILD=\"1\"",
        ] {
            assert!(args.iter().any(|a| a == expected), "{expected}");
        }
        assert!(cmd.get_envs().any(
            |(key, value)| key == "PIRA_TEAM_CHILD" && value == Some(std::ffi::OsStr::new("1"))
        ));
        assert!(
            !args
                .iter()
                .any(|a| a.contains("danger-full-access") || a == "resume")
        );
    }
    include!("project_trust_tests.rs");
}
