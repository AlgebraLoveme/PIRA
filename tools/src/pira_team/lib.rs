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

const WORKER_POLICY_VERSION: u64 = 19;

pub const POLICY: &str = include_str!("main.md");
const IMPLEMENTATION_POLICY: &str = include_str!("implementation.md");
const REVIEW_POLICY: &str = include_str!("review.md");
const FIGURE_POLICY: &str = include_str!("../../../modules/PUBLIC_FIGURE_STYLE.md");
const HELP: &str = r#"pira_team — retained technical artifact workers for review and implementation
Usage: pira_team run --task TASK --completion-gate TEXT [--inject-review] [--inject-implement] [--inject-fig] [OPTIONS]
       pira_team resume RUN_ID [--inject-review] [--inject-implement] [--inject-fig] [--task TASK --completion-gate TEXT] [OPTIONS]
       pira_team steer RUN_ID --task TASK --completion-gate TEXT [--store DIR]
       pira_team interrupt [-r|--recursive] RUN_ID [--store DIR]
       pira_team read|path RUN_ID [RELATIVE_FILE] [--store DIR]
       pira_team bookmark RUN_REF --label LABEL [--store DIR]
       pira_team unbookmark LABEL [--store DIR]
       pira_team bookmarks [--limit N] [--store DIR]
       pira_team search QUERY [--limit N] [--store DIR]
       pira_team config show [--store DIR]
       pira_team config set --main MODEL --model WORKER --effort EFFORT [--store DIR]
       pira_team config reset --main MODEL [--store DIR]

Use one quoted --task (also --task=TEXT) or --task-file FILE; the file is INPUT.
Root-only --allow-subteam on run/resume authorizes one coordinator level; children cannot delegate.
Run options: --cwd DIR --store DIR --model MODEL --effort EFFORT --label LABEL
             --format auto|markdown|text|json|csv --schema FILE --columns JSON_ARRAY
             --output artifact|answer
Resume can override model/effort/output. Taskless resume retains the assignment's gate.
A replacement task or steer requires its dedicated --completion-gate; no semantic gate
validation is performed. Completed means the worker reports the gate satisfied.
Needs_decision and incomplete are distinct outcomes, not accepted completion.

Workers always receive main guidance including ctx/nav/dec.
--inject-review, --inject-implement and --inject-fig independently add guidance on run/resume.
--inject-fig adds public figure guidance only. No injection flag grants edit authority.
Combining --inject-review and --inject-implement on a fresh run reviews first, then
automatically appends implementation guidance in the same thread only after a successful
internal checkpoint; --inject-fig does not change these stages.
Resume continues the retained stage; standalone runs do not retroactively become staged.
Resume retains guidance and adds
only missing selections without replacing the cached base prefix. Native subagents and
global/project PIRA loading are disabled. All phases inherit verified caller execution permissions; REVIEW assignments protect
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
Bookmarks are optional management metadata, never worker input. Labels are exact,
case-sensitive, main-session-scoped names (1..128 UTF-8 bytes, no edge whitespace,
control characters, / or \; no leading @ or . / .. names). --label attaches after
creation of the initial running manifest; later startup/worker failures retain the
bookmark on that failed/interrupted run. Pre-run validation never moves a bookmark.
Bookmark write failure fails the new run before worker launch. Errors before atomic
replacement preserve prior mappings; a later directory-sync error reports replacement.
The managed bookmarks.json index is bounded to 1 MiB; no automatic pruning is done.
Contended edits wait up to 5 seconds for the metadata lock, then fail busy; readers
observe complete snapshots. This budget never limits worker startup/inference.
bookmark RUN_REF --label LABEL atomically adds/moves a label; its JSON receipt reports
previous/current run references and changed. Same-target assignment is a no-write no-op.
unbookmark LABEL removes only the mapping, never runs/artifacts; absence is a no-op.
bookmarks lists labels; search performs case-insensitive literal keyword matching on
labels only. Both return JSON matches (label, run_id, run_root) and has_more, ordered
by exact label ascending, with --limit 1..1000 (default 20). Invalid/unreadable metadata
fails visibly, never resets or reports partial results. Run ownership is verified before
listing; logs are not searched. Inaccessible pointers are hidden from listings and
previous/current receipts, but own dangling mappings may still be removed.
Use @LABEL as RUN_REF for resume/steer/interrupt/read/path/bookmark. Aliases resolve once
at invocation; plain run IDs are never interpreted as labels. Missing labels fail closed.
All run and bookmark operations require verified main caller identity, including literal
IDs and explicit stores. A run belongs to its creating main session, not workspace or
worker native thread. Resume preserves that creator. Same labels in different sessions
are independent; no global fallback. Read/path and metadata operations launch no workers
and require no backend authentication. Config remains store-wide and session-free.
Legacy global bookmarks are preserved but hidden. Legacy runs require immutable creator
metadata or an original first revision snapshot; an actively locked initial revision is
also recognized. Ambiguous historical runs remain intact but inaccessible. Ownership
checks provide Team-interface isolation only, not OS/filesystem/process security.

--format auto lets the worker declare markdown/text/json/csv for its extensionless handoff.
Explicit formats assign a matching extension. --schema requires json; --columns requires csv
and a JSON array of ordered headers. External schema references are disabled. Validation
checks file delivery and format only, not findings or the completion gate. Decision/incomplete
outcomes bypass the report schema/columns. One file-format repair at most, preserving context
and substantive work; failure exits nonzero with diagnostic paths. Repair keeps current caller permissions
but is instructed to edit only the handoff. Invalid file candidates are retained in repair logs.

Run/resume block without an execution deadline. Stream stderr for run_id and active-turn
notice; await the original invocation, not log polling. A live hung worker can be interrupted.
Controls acknowledge acceptance, not completion; never blindly retry unknown delivery.
Steering is unavailable during startup/repair. Normal interruption retains the conversation.
Steering validates current caller permissions and requires the same represented sandbox policy
as the active endpoint; changed or unverifiable policies require interrupt then explicit resume.
Interruption remains available with verified caller identity even if execution policy changes.
Concurrent resumes and orphaned running state fail closed. Legacy ephemeral runs cannot resume.
Legacy retained runs need an explicit completion gate on first migration; their history is kept.
The original base prefix is retained; current worker instructions are directly injected on
migration, and current handoff/gate are supplied each turn. Cache hits are not guaranteed.

Execution permissions always inherit the matching caller session's latest recorded context,
including on resume; explicit model/effort do not bypass permission verification. Missing,
malformed or unrepresentable policies fail closed. Only approval-never is supported; Team
cannot handle interactive approvals. Read-only callers cannot supply required managed writes.
Fine-grained restricted permission profiles are unsupported by this native API. Workspace-write
callers must place managed Team/tool stores and configured build roots within their writable scope;
Team does not add filesystem/network authority. Full access does not expand task ownership.
On new runs, caller model exactly gpt-6-astra (at any effort) maps to gpt-6.1-sol/high.
Recognized callers gpt-6.1-sol, gpt-6-sol, gpt-6-luna and gpt-5.6-sol inherit model/effort
unchanged. Unknown or missing caller model identity falls back to gpt-6.1-sol/high, not the
caller's effort. These bundled defaults apply unless config supplies an exact caller-model
override; no aliases or network discovery are used. Config show displays bundled policy,
overrides and their file path. Set requires both fields; reset removes only that caller's
override. Config uses worker_defaults.json in the selected Team store, needs no Codex
session, and launches no worker. Invalid config fails visibly without being overwritten;
fix/restore the reported file before retrying. Caller discovery requires a matching Codex session
and verified execution context; fallback does not enable Claude/non-Codex session discovery.
Explicit --model and --effort independently override their respective fields, including
partial overrides. Resume retains stored model/effort unless explicitly overridden; it does
not reapply caller defaults. Missing effort for a recognized caller requires --effort;
missing or ambiguous caller execution context always fails, even with explicit profile flags.
Requires native Codex app-server with the published Team protocol and strict-config support,
plus existing local file auth or CODEX_API_KEY. Preflight generates native schemas without an execution deadline;
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
CCACHE_DIR, SCCACHE_DIR and XDG_CACHE_HOME are used for build/cache writes only within caller writable scope.
PIRA_TEAM_BUILD_ROOTS adds a JSON array of existing absolute directories for other build systems.
Invalid explicit roots fail; unusable ambient roots grant nothing. Filesystem roots are excluded;
physical paths are deduplicated. Run/resume/repair use current invoking configuration and record
build_roots in manifests. Other inherited variables grant no extra writes. No cache deletion is
authorized; task/ownership constraints still apply. Unix cancellation excludes detached descendants.
Subteams: exactly main -> coordinator -> ordinary children. Authorization is retained and
bound to the actual native backend ancestry and verified thread, not environment assertions.
A coordinator cannot grant --allow-subteam or change config, escape its retained store,
or access siblings, ancestors or root runs. Tags and child content belong to the immediate
parent only. Root read of a coordinator manifest includes descendant activity/usage summaries,
not content. usage is own cumulative usage; descendants.usage is separate, summed once per
retained child across resumes (usage_complete remains explicit).
interrupt is target-only and reports active_children_left_running; -r/--recursive closes a
subtree admission gate before stopping and confirms owner exit or reports explicit failures.
Root recursive interrupt may control descendants but grants no read/tag access. A later
explicit root resume clears the gate only after all owners settle; old children cannot clear
it. A crashed owner with an unconfirmed backend is an explicit failure, never a PID-only kill.
Coordinators must integrate and finish required child work; completed is rejected while any
child remains active. Incomplete/needs_decision preserve child lifecycle visibility.
Legacy runs gain no subteam permission without an explicit root grant. Original cached policy
prefixes are preserved; migration injects current rules. These are Team-interface controls,
not filesystem security against same-user manual tampering.
Windows backend scopes use owned job handles; child launchers explicitly break away from
the coordinator job without escaping caller-owned outer jobs. Backend authentication for
children uses a local OS-peer-verified, one-time launcher handoff and remains in launcher
memory/backend environments only. It is never a shell setting, task, policy, artifact or
control/event-log value; file-backed authentication and shell exclusions are preserved.
Delegated backend executable and provider/proxy/loader selectors remain coordinator-selected;
a worker cannot redirect its inherited authentication through environment overrides. Ordinary
task/build variables remain inherited.
"#;

struct Options {
    cwd: PathBuf,
    store: PathBuf,
    model: String,
    effort: String,
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
    let mut output = "artifact".to_string();
    let mut format = "auto".to_string();
    let (mut schema, mut columns) = (None, None);
    let mut navigation = "nav".to_string();
    let mut gate = None;
    let mut label = None;
    let mut inject_review = None;
    let mut inject_implement = None;
    let mut inject_fig = None;
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
        execution,
        task,
        label,
        output,
        navigation,
        completion_gate,
        inject_review: inject_review.unwrap_or(false),
        inject_implement: inject_implement.unwrap_or(false),
        inject_fig: inject_fig.unwrap_or(false),
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
    let outcome = if matches!(args[0].as_str(), "read" | "path") {
        storage::access(&args)
    } else if matches!(
        args[0].as_str(),
        "bookmark" | "unbookmark" | "bookmarks" | "search"
    ) {
        bookmarks::command(&args)
    } else if args[0] == "config" {
        profile::defaults::command(&args, storage::default_root)
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
        assert_eq!(WORKER_POLICY_VERSION, 19);
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
}
