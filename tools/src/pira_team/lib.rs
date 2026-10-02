mod app_server;
mod artifact;
mod lifecycle;
mod profile;
mod storage;

use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use std::time::{Duration, SystemTime, UNIX_EPOCH};

static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn cancel_worker(_: libc::c_int) {
    use std::sync::atomic::Ordering;
    CANCELLED.store(true, Ordering::SeqCst);
}

pub const POLICY: &str = include_str!("policy.md");
const CODE_FIX_POLICY: &str = include_str!("code_fix.md");
const CODE_REVIEW_POLICY: &str = include_str!("code_review.md");
const NAV_POLICY: &str = include_str!("nav_policy.md");
const HELP: &str = r#"pira_team — retained Codex workers; read-only by default
Usage: pira_team run [--model MODEL] [--effort EFFORT] [--cwd DIR]
       [--store DIR] [--timeout SECONDS] [--output artifact|answer]
       [--code-review [--allow-fix]] [--navigation nav|shell] [--format auto|markdown|text|json|csv]
       [--schema FILE] [--columns JSON_ARRAY] --task TASK
       pira_team run [--model MODEL] [--effort EFFORT] [OPTIONS] --task-file FILE
       pira_team resume RUN_ID [--allow-fix] [--task TASK] [--model MODEL] [--effort EFFORT]
       [--timeout SECONDS] [--output artifact|answer] [--store DIR]
       pira_team steer RUN_ID --task TASK [--store DIR]
       pira_team interrupt RUN_ID [--store DIR]
       pira_team read RUN_ID [RELATIVE_FILE] [--store DIR]
       pira_team path RUN_ID [RELATIVE_FILE] [--store DIR]
       pira_team help | --version

--task TEXT supplies one quoted task argument on run/resume/steer; --task=TEXT is also accepted.
--task-file FILE instead reads the INPUT task; output files are launcher-managed.
Legacy positional tasks remain accepted. Duplicate or mixed task sources are rejected.
Default stdout: JSON receipt with run_id, validated artifact path, format, logs, repair count
revision and cumulative reported usage (including repair). --output answer prints exact artifact bytes.
The main agent reads/processes artifacts; no automatic workspace publication.
Worker chooses a safe basename; only the launcher writes deliverable files. UTF-8 formats only.
--format defaults to auto. --schema requires json; external schema refs are disabled.
--columns requires csv and is an ordered JSON array of header names.
JSON syntax/schema are checked. CSV checks comma-delimited header/row shape, not
strict quoting syntax. Markdown/text have nonempty-text checks, not syntax checks.
No validation of factual correctness. One automatic format-repair attempt at most,
within each run/resume timeout (default 900s; resume retains its prior limit). Invalid candidates and diagnostics remain
in logs; exhausted repair or worker failure exits nonzero with empty stdout.

Private logs include manifest.json, policy.md, task.txt, events.jsonl, stderr.log,
candidate.txt and validation.json on invalid output; repair/ holds the second attempt.
Validated deliverables live under artifacts/. Read RUN_ID for the completed artifact;
read RUN_ID manifest.json or repair/validation.json for diagnostics, even after failure.
path RUN_ID returns the artifact path for scripts; path RUN_ID . returns its run directory.
Lookups do not start workers, inherit profiles, create storage, or modify any files.
resume continues the same conversation after completion/interruption/failure, retaining
profile, cwd, navigation, code-review mode and output contract. TASK defaults to continuing the assignment.
Explicit model/effort/output/timeout override their own fields. Prior artifacts are
immutable; later outputs live under revisions/NNNNNN/. Root manifest.json describes
the latest revision; earlier snapshots are revisions/NNNNNN/manifest.json.
read without a filename requires the latest revision to be completed.
Run/resume block and advertise their run_id on stderr. Once "active" is printed, a
second command can steer or interrupt that exact turn. Controls only acknowledge
acceptance; await the original invocation for completion. Steering cannot change
configuration and is rejected during format repair. Never blindly retry a control
with unknown delivery outcome. No automatic restart on idle/stale controls.
A per-run lock rejects competing resumes. Orphaned running state fails closed.
Legacy ephemeral runs remain readable but cannot resume their discarded conversation.
Worker session data stays private in the store; authentication is detached on exit.
Logs also include requests.jsonl and controls.jsonl; managed lookup excludes session
state and control capabilities. Normal interrupt/timeout keeps context for resume.
Store selection: --store DIR > PIRA_TEAM_DIR > OS temp/pira-team-UID on Unix
(OS temp/pira-team elsewhere). Use the same store for every command. No variable setup
is needed for the default store. Receipts retain absolute paths for compatibility;
run IDs and relative artifact names avoid copying those paths for ordinary access.
--code-review adds focused correctness and maintainability review guidance on run; resume retains it.
--allow-fix on run requires --code-review: finish a read-only review, then resume the
same worker in workspace-write mode with fix guidance. Only the final receipt/answer
is printed; the original review remains in revision 1. Each phase has its own timeout
and at most one format repair. Failed review never advances automatically.
resume RUN_ID --allow-fix enables fixing after a completed code review. Later resumes
retain write permission. Format repair is always read-only, including after fixing.
Fixes happen in the shared cwd, not a worktree; no rollback or merge is automatic.
Assign disjoint files to concurrent fix workers and validate their integration.
Failure or interruption may leave partial edits. Policies are injected directly;
workers need not read policy files. Global/project PIRA instructions stay disabled.
Navigation defaults to nav; keep pira_nav on PATH. shell omits nav guidance/use.
Requires native Codex app-server (tested with 0.159.3), file-based login or
CODEX_API_KEY. Omitted model/effort inherit the current Codex session's latest recorded
turn settings. Each explicit flag overrides its own field. Inheritance uses
CODEX_THREAD_ID and CODEX_HOME (default ~/.codex); no parent messages are forwarded.
If unavailable/ambiguous, supply explicit values; there is no global/default fallback.
Parent logs are bounded at 256 MiB and 16 MiB per record. An incomplete record fails
closed. The manifest records effective model/effort and each setting's source.
No global PIRA injection or nested delegation. Worker writes require --allow-fix. Secret avoidance is an
instructional rule, not a filesystem secrecy boundary. Full logs are ordinary files.
"#;

struct Options {
    cwd: PathBuf,
    store: PathBuf,
    model: String,
    effort: String,
    profile_sources: Value,
    task: String,
    timeout: Duration,
    output: String,
    navigation: String,
    code_review: bool,
    allow_fix: bool,
    mode: Mode,
    contract: artifact::Contract,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    ReadOnly,
    Fix,
}

impl Mode {
    fn sandbox(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Fix => "workspace-write",
        }
    }
    fn sandbox_type(self) -> &'static str {
        match self {
            Self::ReadOnly => "readOnly",
            Self::Fix => "workspaceWrite",
        }
    }
    fn instructions(self) -> &'static str {
        match self {
            Self::ReadOnly => {
                "You are a read-only worker. Do not write, create, or delete files. Complete the assigned task and return the deliverable in your final answer; the launcher handles storage."
            }
            Self::Fix => {
                "You are in the authorized fix phase. You may edit assigned code and tests within the working directory. Return the deliverable in your final answer; the launcher handles its storage."
            }
        }
    }
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
    let mut store = storage::default_root();
    let (mut model, mut effort, mut task, mut task_file) = (None, None, None, None);
    let mut timeout = 900;
    let mut output = "artifact".to_string();
    let mut format = "auto".to_string();
    let (mut schema, mut columns) = (None, None);
    let mut navigation = "nav".to_string();
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
            "--cwd" => cwd = PathBuf::from(value),
            "--store" => store = PathBuf::from(value),
            "--model" => model = Some(value.clone()),
            "--effort" => effort = Some(value.clone()),
            "--task" => set_once(&mut task, value.clone(), "task")?,
            "--task-file" => set_once(&mut task_file, PathBuf::from(value), "--task-file")?,
            "--timeout" => timeout = value.parse::<u64>().map_err(|_| "invalid timeout")?,
            "--output" => output = value.clone(),
            "--navigation" => navigation = value.clone(),
            "--format" => format = value.clone(),
            "--schema" => schema = Some(PathBuf::from(value)),
            "--columns" => columns = Some(value.clone()),
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    if allow_fix.unwrap_or(false) && !code_review.unwrap_or(false) {
        return Err("--allow-fix requires --code-review on run".into());
    }
    let task = task_input(task, task_file, None)?;
    let (model, effort, profile_sources) = profile::resolve(model, effort)?;
    if timeout == 0 || task.trim().is_empty() {
        return Err("timeout and task must be nonempty".into());
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
        cwd,
        store,
        model,
        effort,
        profile_sources,
        task,
        timeout: Duration::from_secs(timeout),
        output,
        navigation,
        code_review: code_review.unwrap_or(false),
        allow_fix: allow_fix.unwrap_or(false),
        mode: Mode::ReadOnly,
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

fn command(options: &Options, run: &Path, dir: &Path, mode: Mode) -> Command {
    let mut cmd = Command::new("codex");
    cmd.args(["app-server", "--strict-config", "--stdio"]);
    for setting in [
        format!("model={}", json!(options.model)),
        format!("model_reasoning_effort={}", json!(options.effort)),
        format!("sandbox_mode={}", json!(mode.sandbox())),
        format!("model_instructions_file={}", json!(dir.join("policy.md"))),
        "developer_instructions=\"\"".into(),
        "project_doc_max_bytes=0".into(),
        "approval_policy=\"never\"".into(),
        "features.multi_agent=false".into(),
        "features.memories=false".into(),
        "features.apps=false".into(),
        "features.plugins=false".into(),
        "features.hooks=false".into(),
        "features.skip_host_skill_discovery=true".into(),
        "features.image_generation=false".into(),
        "features.browser_use=false".into(),
        "features.computer_use=false".into(),
        format!(
            "projects.{}.trust_level=\"untrusted\"",
            json!(options.cwd.to_string_lossy())
        ),
        "web_search=\"disabled\"".into(),
        "shell_environment_policy.inherit=\"core\"".into(),
        "shell_environment_policy.set.PIRA_TEAM_CHILD=\"1\"".into(),
    ] {
        cmd.arg("-c").arg(setting);
    }
    cmd.env("PIRA_TEAM_CHILD", "1")
        .env_remove("CODEX_THREAD_ID");
    cmd.env("CODEX_HOME", run.join("codex-home"));
    cmd.current_dir(dir).stdin(Stdio::piped());
    cmd
}

// Reused from the archived Team app-server transport: isolate/terminate the whole process tree.
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
        let mut args = vec!["run", "--model", "test-model", "--effort", "high"];
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
            let cmd = command(&opt, run, &dir, Mode::ReadOnly);
            let home = cmd
                .get_envs()
                .find(|(key, _)| *key == "CODEX_HOME")
                .unwrap()
                .1
                .unwrap();
            assert_eq!(home, run.join("codex-home").as_os_str());
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
            Mode::ReadOnly,
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for expected in [
            "app-server",
            "--strict-config",
            "sandbox_mode=\"read-only\"",
            "project_doc_max_bytes=0",
            "approval_policy=\"never\"",
            "features.multi_agent=false",
        ] {
            assert!(args.iter().any(|a| a == expected), "{expected}");
        }
        assert!(
            !args
                .iter()
                .any(|a| a.contains("danger-full-access") || a == "resume")
        );
    }
}
