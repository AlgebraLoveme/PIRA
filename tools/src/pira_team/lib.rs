mod app_server;
mod artifact;
mod backend;
mod build_roots;
mod lifecycle;
mod profile;
mod storage;

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

const WORKER_POLICY_VERSION: u64 = 12;

pub const POLICY: &str = include_str!("main.md");
const IMPLEMENTATION_POLICY: &str = include_str!("implementation.md");
const REVIEW_POLICY: &str = include_str!("review.md");
const HELP: &str = r#"pira_team — retained technical artifact workers for review and implementation
Usage: pira_team run --task TASK --completion-gate TEXT [--inject-review] [--inject-implement] [OPTIONS]
       pira_team resume RUN_ID [--task TASK --completion-gate TEXT] [OPTIONS]
       pira_team steer RUN_ID --task TASK --completion-gate TEXT [--store DIR]
       pira_team interrupt RUN_ID [--store DIR]
       pira_team read|path RUN_ID [RELATIVE_FILE] [--store DIR]

Use one quoted --task (also --task=TEXT) or --task-file FILE; the file is INPUT.
Run options: --cwd DIR --store DIR --model MODEL --effort EFFORT
             --format auto|markdown|text|json|csv --schema FILE --columns JSON_ARRAY
             --output artifact|answer
Resume can override model/effort/output. Taskless resume retains the assignment's gate.
A replacement task or steer requires its dedicated --completion-gate; no semantic gate
validation is performed. Completed means the worker reports the gate satisfied.
Needs_decision and incomplete are distinct outcomes, not accepted completion.

Workers always receive main guidance including ctx/nav/dec.
--inject-review and --inject-implement independently add task guidance on run/resume.
Combined launches review first, then automatically append implementation guidance in the
same thread only after a successful internal checkpoint. Neither flag grants edit authority.
Resume continues the retained stage; standalone runs do not retroactively become staged.
Resume retains guidance and adds
only missing selections without replacing the cached base prefix. Native subagents and
global/project PIRA loading are disabled. All phases use workspace-write; REVIEW assignments protect
project artifacts, including tests, configuration, docs, lockfiles and expected outputs, by instruction.
Build/test outputs, authorized cache updates, scratch and managed handoffs remain permitted;
commands rewriting protected artifacts require check-only mode or a disposable copy. IMPLEMENTATION assignments authorize only their owned files. Fixes are implementation.
Secrets exclusion and file ownership are instructional, not hard read/file-level boundaries.
No rollback/merge is automatic; failure/interruption may leave partial edits.
--code-review and --allow-fix are deprecated aliases with no permission/eligibility gate;
explicit task instructions prevail. --navigation nav remains accepted; shell is deprecated.

The launcher gives the worker a direct file path in artifacts/ for its handoff. Each resume
has a new immutable-by-contract revision artifact. Default stdout is a JSON receipt with
only run_id, status, run_root and handoff_path. --output answer prints exact
handoff bytes. Nothing is automatically published into the project workspace.
read RUN_ID manifest.json retrieves usage, revisions, repairs, format, completion gate and
artifact/log references; path RUN_ID manifest.json returns its path. Earlier revision metadata
is available at revisions/NNNNNN/manifest.json. Repair and incomplete-accounting warnings
remain on stderr; routine diagnostic metadata stays in manifests.
path RUN_ID returns the handoff; path RUN_ID . returns the run root for scripts.
read/path allow safe relative diagnostic/artifact files and exclude backend credentials/state.
read without a relative filename requires a completed, needs_decision or incomplete outcome.
Store: --store > PIRA_TEAM_DIR > persistent PIRA parent/team.
Parent: macOS HOME/Library/Application Support/PIRA; Linux absolute, nonempty
XDG_DATA_HOME/pira or HOME/.local/share/pira; Windows LOCALAPPDATA/PIRA. Ctx and Dec default to ctx and
decision under the same parent; their explicit environment overrides remain supported.
Older temporary-store runs require --store OLD_STORE or PIRA_TEAM_DIR until explicitly
migrated. Retained runs embed absolute paths: do not simply move/merge run directories.
Use the same store across commands. Read/path never launch a worker.

--format auto lets the worker declare markdown/text/json/csv for its extensionless handoff.
Explicit formats assign a matching extension. --schema requires json; --columns requires csv
and a JSON array of ordered headers. External schema references are disabled. Validation
checks file delivery and format only, not findings or the completion gate. Decision/incomplete
outcomes bypass the report schema/columns. One file-format repair at most, preserving context
and substantive work; failure exits nonzero with diagnostic paths. Repair stays workspace-write
but is instructed to edit only the handoff. Invalid file candidates are retained in repair logs.

Run/resume block without an execution deadline. Stream stderr for run_id and active-turn
notice; await the original invocation, not log polling. A live hung worker can be interrupted.
Controls acknowledge acceptance, not completion; never blindly retry unknown delivery.
Steering is unavailable during startup/repair. Normal interruption retains the conversation.
Concurrent resumes and orphaned running state fail closed. Legacy ephemeral runs cannot resume.
Legacy retained runs need an explicit completion gate on first migration; their history is kept.
The original base prefix is retained; current worker instructions are directly injected on
migration, and current handoff/gate are supplied each turn. Cache hits are not guaranteed.

Model/effort inherit the main's latest recorded profile; resume retains them. Explicit flags
independently override them. If inheritance is unavailable/ambiguous, supply explicit values.
Requires native Codex app-server with the published Team protocol and strict-config support,
plus existing local file auth or CODEX_API_KEY. Preflight generates native schemas (10s limit);
missing API fields/methods fail before model work. Sent requests are schema-validated.
Direct stdio is separate from the interactive shared daemon; its version/features are not
Team compatibility evidence. This checks published API inventory and outgoing request shapes,
not model access, notification semantics or actual shell sandbox behavior. Authentication
is launcher-managed and detached after each invocation. All raw logs remain tool-managed.
Worker shells inherit the invoking environment on launch/resume, except session/store identity,
authentication sockets and named credential patterns. Team supplies its own handoff/guard/stores.
Unknown variables pass through; this is not a secret detector. Existing absolute directories in
CARGO_HOME, CARGO_TARGET_DIR, PIP_CACHE_DIR, UV_CACHE_DIR, PYTHONPYCACHEPREFIX, MYPY_CACHE_DIR,
npm_config_cache, NPM_CONFIG_CACHE, YARN_CACHE_FOLDER, GOCACHE, GOMODCACHE, GRADLE_USER_HOME,
CCACHE_DIR, SCCACHE_DIR and XDG_CACHE_HOME automatically grant build/cache writes without prompts.
PIRA_TEAM_BUILD_ROOTS adds a JSON array of existing absolute directories for other build systems.
Invalid explicit roots fail; unusable ambient roots grant nothing. Filesystem roots are excluded;
physical paths are deduplicated. Run/resume/repair use current invoking configuration and record
build_roots in manifests. Other inherited variables grant no extra writes. No cache deletion is
authorized; task/ownership constraints still apply. Unix cancellation excludes detached descendants.
"#;

struct Options {
    cwd: PathBuf,
    store: PathBuf,
    model: String,
    effort: String,
    profile_sources: Value,
    task: String,
    output: String,
    navigation: String,
    completion_gate: String,
    inject_review: bool,
    inject_implement: bool,
    ctx_store: PathBuf,
    dec_store: PathBuf,
    contract: artifact::Contract,
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
    if args.first().map(String::as_str) != Some("run") {
        return Err("expected run; use pira_team help".into());
    }
    let mut cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut store = None;
    let (mut model, mut effort, mut task, mut task_file) = (None, None, None, None);
    let mut output = "artifact".to_string();
    let mut format = "auto".to_string();
    let (mut schema, mut columns) = (None, None);
    let mut navigation = "nav".to_string();
    let mut gate = None;
    let mut inject_review = None;
    let mut inject_implement = None;
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
        if arg == "--inject-review" {
            set_once(&mut inject_review, true, "--inject-review")?;
            continue;
        }
        if arg == "--inject-implement" {
            set_once(&mut inject_implement, true, "--inject-implement")?;
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
            "--completion-gate" => set_once(&mut gate, value.clone(), "--completion-gate")?,
            "--cwd" => cwd = PathBuf::from(value),
            "--store" => store = Some(PathBuf::from(value)),
            "--model" => model = Some(value.clone()),
            "--effort" => effort = Some(value.clone()),
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
    let completion_gate = completion_gate(gate)?;
    let task = task_input(task, task_file, None)?;
    let (model, effort, profile_sources) = profile::resolve(model, effort)?;
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
        cwd,
        store: store.map(Ok).unwrap_or_else(storage::default_root)?,
        model,
        effort,
        profile_sources,
        task,
        output,
        navigation,
        completion_gate,
        inject_review: inject_review.unwrap_or(false),
        inject_implement: inject_implement.unwrap_or(false),
        contract: artifact::Contract::new(format, schema.as_deref(), columns.as_deref())?,
    })
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
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
    fn new(run: &Path) -> Result<Self, String> {
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
        if std::env::var_os("CODEX_API_KEY").is_some_and(|value| !value.is_empty()) {
            return Ok(home);
        }
        let source = std::env::var_os("CODEX_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|p| PathBuf::from(p).join(".codex"))
            });
        if let Some(source) = source {
            let auth = source.join("auth.json");
            match auth.canonicalize() {
                Ok(auth) => {
                    // Link only authentication, never configuration, instructions, skills or history.
                    // Codex, not the model or Team, reads authentication material.
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&auth, home.0.join("auth.json"))
                        .map_err(|e| format!("link Codex authentication: {e}"))?;
                    #[cfg(not(unix))]
                    fs::hard_link(&auth, home.0.join("auth.json"))
                        .map_err(|e| format!("link Codex authentication: {e}; use CODEX_API_KEY if linking is unavailable"))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("resolve Codex authentication: {e}")),
            }
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
    let mut cmd = Command::new("codex");
    cmd.args(["app-server", "--strict-config", "--stdio"]);
    for setting in backend::contract()["config"].as_array().unwrap() {
        cmd.arg("-c").arg(setting.as_str().unwrap());
    }
    for setting in [
        format!("model={}", json!(options.model)),
        format!("model_reasoning_effort={}", json!(options.effort)),
        format!("model_instructions_file={}", json!(dir.join("policy.md"))),
        format!(
            "projects.{}.trust_level=\"untrusted\"",
            json!(options.cwd.to_string_lossy())
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
    cmd.arg("-c").arg(format!(
        "shell_environment_policy.set.PIRA_TEAM_HANDOFF={}",
        json!(handoff)
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
            "PIRA_CTX_STORE_DIR",
            "PIRA_DEC_STORE_DIR",
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

// Unix cancellation isolates/terminates the owned process group, excluding detached descendants.
#[cfg(unix)]
fn isolate(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}
#[cfg(not(unix))]
fn isolate(_: &mut Command) {}

fn terminate(pid: u32) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(pid) {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        print!("{HELP}");
        return 0;
    }
    if args == ["--version"] {
        println!("pira_team {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if std::env::var_os("PIRA_TEAM_CHILD").is_some() {
        eprintln!("pira_team: nested delegation is forbidden");
        return 1;
    }
    let outcome = if matches!(args[0].as_str(), "read" | "path") {
        storage::access(&args)
    } else if matches!(args[0].as_str(), "resume" | "steer" | "interrupt") {
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
    fn options(extra: &[&str]) -> Result<Options, String> {
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
        parse(&args.into_iter().map(str::to_owned).collect::<Vec<_>>())
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
}
