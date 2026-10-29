//! Bounded immediate-parent ownership and serialized subtree lifecycle admission.
use crate::{
    CANCELLED,
    lifecycle::{read_json, save_json},
    profile,
};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{OnceLock, atomic::Ordering},
    time::{Duration, Instant},
};

const MARKER: &str = "shell_environment_policy.set.PIRA_TEAM_CONTEXT=";
const LOCK_BUDGET: Duration = Duration::from_secs(5);

// Discovery hints never grant authority. Verify the actual backend PID/birth,
// retained revision and native thread, including when every hint is stripped.
fn discover() -> Result<Option<Caller>, String> {
    discover_from(std::process::id())
}

#[cfg(any(target_os = "linux", windows, test))]
fn marker(arguments: &[String]) -> Result<Option<PathBuf>, String> {
    if !arguments
        .windows(3)
        .any(|a| a == ["app-server", "--strict-config", "--stdio"])
    {
        return Ok(None);
    }
    arguments
        .iter()
        .find_map(|arg| arg.strip_prefix(MARKER))
        .map(|value| {
            serde_json::from_str::<PathBuf>(value).map_err(|_| "invalid Team context marker".into())
        })
        .transpose()
}

#[cfg(target_os = "linux")]
fn inspect(pid: u32) -> Result<(crate::native::Identity, Option<PathBuf>), String> {
    inspect_linux_using(
        pid,
        crate::native::metadata,
        crate::native::arguments,
        crate::native::executable,
    )
}

#[cfg(any(target_os = "linux", test))]
fn inspect_linux_using(
    pid: u32,
    mut metadata: impl FnMut(u32) -> std::io::Result<crate::native::Metadata>,
    arguments: impl FnOnce(u32) -> std::io::Result<Vec<String>>,
    executable: impl FnOnce(u32) -> std::io::Result<PathBuf>,
) -> Result<(crate::native::Identity, Option<PathBuf>), String> {
    let before = match metadata(pid) {
        Ok(metadata) => metadata,
        Err(error) if crate::native::missing(&error) => crate::native::Metadata {
            pid,
            parent: 0,
            birth: 0,
            alive: false,
        },
        Err(_) => return Err("cannot verify process identity".into()),
    };
    if !before.alive {
        return Ok((before.with_executable(PathBuf::new()), None));
    }
    let arguments = arguments(pid).map_err(|_| "cannot verify Team process ancestry")?;
    let candidate = marker(&arguments);
    let mut path = PathBuf::new();
    let run = match candidate {
        Ok(None) => None,
        candidate => {
            // No SSH/name/UID boundary is trusted. Only a possible backend
            // marker needs exe access; unmarked ancestors are still traversed.
            path = executable(pid).map_err(|_| "cannot verify process identity")?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if name.starts_with("codex") || name.starts_with("python") {
                candidate?
            } else {
                None
            }
        }
    };
    if metadata(pid).map_err(|_| "cannot verify process identity")? != before {
        return Err("process identity changed during ancestry inspection".into());
    }
    Ok((before.with_executable(path), run))
}

#[cfg(not(target_os = "linux"))]
fn inspect(pid: u32) -> Result<(crate::native::Identity, Option<PathBuf>), String> {
    let identity = match crate::native::identity(pid) {
        Ok(identity) => identity,
        Err(error) if crate::native::missing(&error) => {
            return Ok((
                crate::native::Identity {
                    pid,
                    parent: 0,
                    birth: 0,
                    executable: PathBuf::new(),
                    alive: false,
                },
                None,
            ));
        }
        Err(_) => return Err("cannot verify process identity".into()),
    };
    let executable = identity
        .executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !executable.starts_with("codex") && !executable.starts_with("python") {
        return Ok((identity, None));
    }
    #[cfg(windows)]
    let run = {
        let args = crate::native::windows::arguments(pid)
            .map_err(|_| "cannot inspect native Windows process arguments")?;
        marker(&args)?
    };
    #[cfg(unix)]
    let run = {
        let output = std::process::Command::new("/bin/ps")
            .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
            .output()
            .map_err(|_| "cannot verify Team process ancestry")?;
        if !output.status.success() {
            return Err("cannot verify Team process ancestry".into());
        }
        let command = String::from_utf8(output.stdout).map_err(|_| "invalid process ancestry")?;
        if command.contains("app-server --strict-config --stdio") {
            command
                .split_once(MARKER)
                .map(|(_, value)| {
                    serde_json::Deserializer::from_str(value)
                        .into_iter::<PathBuf>()
                        .next()
                        .ok_or("missing Team context")?
                        .map_err(|_| "invalid Team context marker")
                })
                .transpose()?
        } else {
            None
        }
    };
    Ok((identity, run))
}

fn discover_from(pid: u32) -> Result<Option<Caller>, String> {
    discover_using(pid, inspect)
}
fn discover_using(
    mut pid: u32,
    mut inspect: impl FnMut(u32) -> Result<(crate::native::Identity, Option<PathBuf>), String>,
) -> Result<Option<Caller>, String> {
    const MISMATCH: &str = "Team runtime does not match actual worker ancestry";
    let initial = pid;
    let mut child_birth = u64::MAX;
    let mut caller: Option<Caller> = None;
    for _ in 0..64 {
        let (identity, candidate) = inspect(pid)?;
        if !identity.alive || identity.birth > child_birth {
            return if caller.is_some() {
                Err(MISMATCH.into())
            } else {
                Ok(None)
            };
        }
        if pid != initial
            && let Some(path) = candidate
        {
            let run = path
                .canonicalize()
                .map_err(|_| "Team context unavailable")?;
            if let Some(bound) = &caller {
                if bound.run != run {
                    return Err(MISMATCH.into());
                }
            } else {
                let runtime = read_json(&run.join("runtime.json"))?;
                let manifest = read_json(&run.join("manifest.json"))?;
                let revision = runtime["revision"].as_u64().ok_or(MISMATCH)?;
                if Some(revision) != manifest["revision"].as_u64() {
                    return Err(MISMATCH.into());
                }
                caller = Some(Caller { run, runtime });
            }
        }
        // Persistent wrappers own the backend scope; their marked descendants
        // identify the context, but only the exact retained owner grants authority.
        if let Some(bound) = &caller
            && bound.runtime["backend_pid"] == pid
        {
            if bound.runtime["backend_birth"].as_u64() != Some(identity.birth) {
                return Err(MISMATCH.into());
            }
            return Ok(caller);
        }
        child_birth = identity.birth;
        pid = identity.parent;
        if pid <= if cfg!(windows) { 4 } else { 1 } {
            return if caller.is_some() {
                Err(MISMATCH.into())
            } else {
                Ok(None)
            };
        }
    }
    Err("Team ancestry exceeds 64 levels".into())
}

#[derive(Clone)]
struct Caller {
    run: PathBuf,
    runtime: Value,
}
fn caller_binding() -> Result<Option<Caller>, String> {
    if crate::remote::delegated_context().is_some() {
        return Ok(None);
    }
    static CALLER: OnceLock<Result<Option<Caller>, String>> = OnceLock::new();
    CALLER.get_or_init(discover).clone()
}
pub fn caller_runtime() -> Result<Option<Value>, String> {
    Ok(caller_binding()?.map(|caller| caller.runtime))
}
pub fn caller_run() -> Result<Option<PathBuf>, String> {
    Ok(caller_binding()?.map(|caller| caller.run))
}
fn current_caller(run: &Path) -> Result<Value, String> {
    let caller = caller_binding()?.ok_or("verified worker ancestry required")?;
    let runtime = read_json(&run.join("runtime.json"))?;
    let manifest = read_json(&run.join("manifest.json"))?;
    if caller.run != run || caller.runtime != runtime || runtime["revision"] != manifest["revision"]
    {
        return Err("stale coordinator invocation cannot cross a resume/interruption epoch".into());
    }
    Ok(manifest)
}

pub fn authorize(args: &[String]) -> Result<(), String> {
    let Some(run) = caller_run()? else {
        if std::env::var_os("PIRA_TEAM_CHILD").is_some()
            || std::env::var_os("PIRA_TEAM_CONTEXT").is_some()
        {
            return Err("nested delegation is forbidden without verified worker ancestry; environment assertions grant nothing".into());
        }
        return Ok(());
    };
    let manifest = current_caller(&run)?;
    let context = profile::load()?;
    if manifest["thread_id"] != context["caller_thread_id"]
        || manifest["allow_subteam"] != true
        || !manifest["parent_run_id"].is_null()
    {
        return Err(
            "nested delegation is forbidden: caller is not an authorized coordinator".into(),
        );
    }
    if args[0] == "config" {
        return Err("coordinators cannot change config or grant --allow-subteam".into());
    }
    invocation_options(args, run.parent().ok_or("missing coordinator store")?)?;
    let tree = read_tree(&run)?;
    if tree["blocked"] != false
        || tree["members"][run.file_name().unwrap().to_str().unwrap()]["active"] != true
    {
        return Err("coordinator subtree is stopped or interrupted".into());
    }
    Ok(())
}

// Skip option values and task literals; data that names a flag is not a grant.
fn invocation_options(args: &[String], store: &Path) -> Result<(), String> {
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') || arg.contains('=') {
            continue;
        }
        if arg == "--allow-subteam" {
            return Err("coordinators cannot grant --allow-subteam".into());
        }
        if [
            "--inject-review",
            "--inject-implement",
            "--inject-fig",
            "--inject-write",
            "--allow-fix",
            "--code-review",
            "-r",
            "--recursive",
        ]
        .contains(&arg.as_str())
        {
            continue;
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("missing value for {arg}"))?;
        if arg == "--store"
            && Path::new(value)
                .canonicalize()
                .map_err(|_| "coordinator store unavailable")?
                != store
        {
            return Err("coordinator cannot escape its retained Team store".into());
        }
    }
    Ok(())
}

pub fn default_store() -> Result<Option<PathBuf>, String> {
    Ok(caller_run()?.map(|run| run.parent().unwrap().to_owned()))
}

pub fn bind(
    run: &Path,
    revision: &Value,
    scope: &crate::process::Scope,
    attempt_dir: &Path,
) -> Result<(), String> {
    let backend =
        crate::native::identity(scope.pid).map_err(|_| "cannot bind native backend identity")?;
    let launcher = crate::native::current()?;
    #[cfg(windows)]
    let backend_job_session = Some(
        crate::native::windows::session_id(scope.pid)
            .map_err(|_| "cannot bind backend job OS session")?,
    );
    #[cfg(not(windows))]
    let backend_job_session: Option<u32> = None;
    save_json(
        &run.join("runtime.json"),
        &json!({"backend_pid":scope.pid,"backend_birth":backend.birth,
        "launcher_pid":launcher.pid,"launcher_birth":launcher.birth,"backend_job":scope.job_name(),
        "revision":revision,"attempt_logs":attempt_dir,"backend_job_session":backend_job_session}),
    )
}

/// Redeem once per admitted child revision; a control token alone is insufficient.
pub fn claim_authentication(
    root: &Path,
    revision: &Value,
    peer: u32,
    request: &Value,
) -> Result<(), String> {
    let identity = crate::native::identity(peer).map_err(|_| "cannot verify credential client")?;
    let own = crate::native::current()?;
    let executable = identity
        .executable
        .canonicalize()
        .map_err(|_| "cannot resolve credential peer executable")?;
    let own_executable = own
        .executable
        .canonicalize()
        .map_err(|_| "cannot resolve Team executable")?;
    if !identity.alive || executable != own_executable {
        return Err("credential peer is not the Team launcher".into());
    }
    let caller = discover_from(peer)?.ok_or("credential client lacks coordinator ancestry")?;
    let _lock = lock(root)?;
    let manifest = read_json(&root.join("manifest.json"))?;
    let runtime = read_json(&root.join("runtime.json"))?;
    let mut tree = read_tree(root)?;
    let child = request["child"]
        .as_str()
        .ok_or("missing credential child")?;
    crate::storage::validate_run_id(child)?;
    let member = &tree["members"][child];
    let retained = read_json(
        &root
            .parent()
            .ok_or("missing Team store")?
            .join(child)
            .join("manifest.json"),
    )?;
    if crate::CANCELLED.load(Ordering::SeqCst)
        || caller.run != root
        || caller.runtime != runtime
        || request["runtime"] != runtime
        || manifest["revision"] != *revision
        || runtime["revision"] != *revision
        || manifest["allow_subteam"] != true
        || !manifest["parent_run_id"].is_null()
        || tree["blocked"] != false
        || tree["members"][id(root)]["active"] != true
        || tree["epoch"] != request["epoch"]
        || member["active"] != true
        || member["blocked"] == true
        || member["revision"] != request["revision"]
        || member["launcher_pid"] != peer
        || member["launcher_birth"] != identity.birth
        || member["credential_claimed"] == true
        || retained["status"] != "running"
        || retained["revision"] != request["revision"]
        || retained["parent_run_id"] != id(root)
        || retained["creator_main_session_id"] != manifest["thread_id"]
        || retained["allow_subteam"] == true
    {
        return Err("credential request is outside the active admitted subtree/runtime".into());
    }
    tree["members"][child]["credential_claimed"] = json!(true);
    save_json(&root.join("tree.json"), &tree)
}

fn tree_root(run: &Path, manifest: &Value) -> Result<PathBuf, String> {
    match &manifest["parent_run_id"] {
        Value::String(id) => {
            crate::storage::validate_run_id(id)?;
            Ok(run.parent().ok_or("missing store")?.join(id))
        }
        Value::Null => Ok(run.to_owned()),
        _ => Err("invalid retained parent relationship".into()),
    }
}

fn read_tree(root: &Path) -> Result<Value, String> {
    let tree = read_json(&root.join("tree.json"))?;
    if tree["version"] != 1
        || tree["blocked"].as_bool().is_none()
        || tree["epoch"].as_u64().is_none()
    {
        return Err("invalid or unsupported subtree metadata".into());
    }
    for (id, member) in tree["members"]
        .as_object()
        .ok_or("invalid subtree members")?
    {
        crate::storage::validate_run_id(id)?;
        if member["active"].as_bool().is_none()
            || member["revision"].as_u64().is_none()
            || (!member["blocked"].is_null() && member["blocked"].as_bool().is_none())
        {
            return Err("invalid retained subtree member".into());
        }
    }
    Ok(tree)
}

#[cfg(test)]
thread_local! {
    static AFTER_BOUND_CHECK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static ON_LOCK_CONTENTION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(super) fn after_bound_check(hook: impl FnOnce() + 'static) {
    AFTER_BOUND_CHECK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}
#[cfg(test)]
pub(super) fn on_lock_contention(hook: impl FnOnce() + 'static) {
    ON_LOCK_CONTENTION.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

/// A short native tree critical section shared by endpoint transitions, model
/// turn admission and recursive interrupt. Never retain it across RPC waits.
#[derive(Clone)]
pub(super) struct AttemptGate {
    root: PathBuf,
    member: String,
    revision: Value,
    epoch: Value,
}
impl AttemptGate {
    pub(super) fn new(run: &Path, manifest: &Value) -> Result<Self, String> {
        if !manifest["revision"].is_u64() || !manifest["subtree_epoch"].is_u64() {
            return Err("missing attempt revision/subtree epoch".into());
        }
        Ok(Self {
            root: tree_root(run, manifest)?,
            member: id(run).to_owned(),
            revision: manifest["revision"].clone(),
            epoch: manifest["subtree_epoch"].clone(),
        })
    }
    pub(super) fn lock(&self) -> Result<File, String> {
        lock(&self.root)
    }
    pub(super) fn with_open<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _lock = self.lock()?;
        let tree = read_tree(&self.root)?;
        let member = &tree["members"][&self.member];
        if tree["blocked"] != false
            || tree["epoch"] != self.epoch
            || member["active"] != true
            || member["blocked"] == true
            || member["revision"] != self.revision
            || CANCELLED.load(Ordering::SeqCst)
        {
            return Err("subtree attempt gate is closed or stale; no new turn admitted".into());
        }
        action()
    }
}

pub fn lock(root: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("tree.lock"))
        .map_err(|e| format!("open subtree lock: {e}"))?;
    let deadline = Instant::now() + LOCK_BUDGET;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                #[cfg(test)]
                {
                    let hook = ON_LOCK_CONTENTION.with(|slot| slot.borrow_mut().take());
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(format!("subtree lock unavailable: {e}")),
        }
    }
}

fn id(run: &Path) -> &str {
    run.file_name().unwrap().to_str().unwrap()
}

/// Admit before publishing running state. The same lock seals completion and interruption.
pub fn admit(run: &Path, manifest: &mut Value, resume: bool) -> Result<(), String> {
    let root = tree_root(run, manifest)?;
    let _lock = lock(&root)?;
    let path = root.join("tree.json");
    let mut tree = if path.exists() {
        read_tree(&root)?
    } else {
        if root != run {
            return Err("missing coordinator relationships".into());
        }
        json!({"version":1,"blocked":false,"epoch":0,"members":{}})
    };
    if root == run {
        // Only deliberate root resume clears the gate. A previous coordinator or child
        // cannot clear it; all previous subtree owners must have settled first.
        if resume
            && (tree["blocked"] == true
                || tree["members"]
                    .as_object()
                    .ok_or("invalid subtree members")?
                    .values()
                    .any(|member| member["blocked"] == true))
        {
            if tree["members"]
                .as_object()
                .ok_or("invalid subtree members")?
                .iter()
                .any(|(_, member)| member["active"] == true)
            {
                return Err("interrupted subtree still has active/unreconciled owners; repeat recursive interrupt before root resume".into());
            }
            tree["blocked"] = json!(false);
            for member in tree["members"].as_object_mut().unwrap().values_mut() {
                member["blocked"] = json!(false);
            }
            tree["epoch"] = json!(tree["epoch"].as_u64().ok_or("invalid interruption epoch")? + 1);
        }
    } else {
        let caller =
            caller_run()?.ok_or("only the immediate coordinator may launch or resume a child")?;
        if caller != root {
            return Err("child belongs to another coordinator".into());
        }
        let parent = current_caller(&root)?;
        if tree["members"][id(run)]["blocked"] == true {
            return Err("child interruption gate requires deliberate root resume".into());
        }
        if parent["allow_subteam"] != true
            || !parent["parent_run_id"].is_null()
            || tree["members"][id(&root)]["active"] != true
            || tree["blocked"] != false
            || tree["members"][id(&root)]["revision"] != parent["revision"]
        {
            return Err("coordinator subtree is closed to child launches/resumes".into());
        }
    }
    if tree["blocked"] != false {
        return Err("subtree interruption gate is closed".into());
    }
    manifest["subtree_epoch"] = tree["epoch"].clone();
    let launcher = crate::native::current()?;
    tree["members"][id(run)] = json!({"active":true,"revision":manifest["revision"],"blocked":false,
        "launcher_pid":launcher.pid,"launcher_birth":launcher.birth,"credential_claimed":false});
    save_json(&path, &tree)
}

pub struct Watcher {
    done: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watcher {
    pub fn start(run: &Path, manifest: &Value) -> Result<Self, String> {
        let root = tree_root(run, manifest)?;
        let epoch = manifest["subtree_epoch"].clone();
        let member = id(run).to_owned();
        let (done, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            loop {
                match read_tree(&root) {
                    Ok(tree)
                        if tree["blocked"] == false
                            && tree["epoch"] == epoch
                            && tree["members"][&member]["blocked"] != true => {}
                    _ => {
                        CANCELLED.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                if !matches!(
                    receiver.recv_timeout(Duration::from_millis(20)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    break;
                }
            }
        });
        Ok(Self {
            done: Some(done),
            thread: Some(thread),
        })
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.done.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn finish(run: &Path, manifest: &mut Value) -> Result<(), String> {
    let root = tree_root(run, manifest)?;
    let _lock = lock(&root)?;
    let mut tree = read_tree(&root)?;
    let children_active = root == run
        && tree["members"]
            .as_object()
            .ok_or("invalid subtree members")?
            .iter()
            .any(|(child, member)| child != id(run) && member["active"] == true);
    let rejected = manifest["status"] == "completed" && children_active;
    if rejected {
        manifest["status"] = json!("incomplete");
        manifest["error"] = json!(
            "coordinator cannot complete while children are active; resume to integrate and finish child work"
        );
    }
    let interrupted = (tree["blocked"] == true || tree["members"][id(run)]["blocked"] == true)
        && manifest["status"] == "completed";
    if interrupted {
        manifest["status"] = json!("interrupted");
        manifest["error"] = json!("subtree interrupted before completion publication");
    }
    manifest["children_active"] = json!(children_active);
    save_json(&run.join("manifest.json"), manifest)?;
    tree["members"][id(run)]["active"] = json!(false);
    save_json(&root.join("tree.json"), &tree)?;
    if rejected || interrupted {
        Err(manifest["error"].as_str().unwrap().to_owned())
    } else {
        Ok(())
    }
}

/// Summaries contain lifecycle/accounting only, never descendant content or paths.
pub fn summary(run: &Path) -> Result<Value, String> {
    let path = run.join("tree.json");
    if !path.exists() {
        return Ok(json!({"runs":[],"usage":{},"usage_complete":true}));
    }
    let _lock = lock(run)?;
    let tree = read_tree(run)?;
    let mut runs = Vec::new();
    let mut usage = serde_json::Map::new();
    let mut complete = true;
    for (child, member) in tree["members"]
        .as_object()
        .ok_or("invalid subtree members")?
    {
        if child == id(run) {
            continue;
        }
        crate::storage::validate_run_id(child)?;
        let manifest = read_json(&run.parent().unwrap().join(child).join("manifest.json"));
        let record = match manifest {
            Ok(m) => {
                complete &= m["usage_complete"] == true && member["active"] != true;
                if let Some(totals) = m["usage"].as_object() {
                    for (key, value) in totals {
                        if let Some(amount) = value.as_u64() {
                            let total = usage
                                .get(key)
                                .and_then(Value::as_u64)
                                .unwrap_or(0_u64)
                                .checked_add(amount)
                                .ok_or("descendant usage overflow")?;
                            usage.insert(key.clone(), json!(total));
                        } else {
                            complete = false;
                        }
                    }
                } else {
                    complete = false;
                }
                json!({"run_id":child,"status":m["status"],"active":member["active"],"usage":m["usage"],"usage_complete":m["usage_complete"]})
            }
            Err(_) => {
                complete = false;
                json!({"run_id":child,"active":member["active"],"status":"unavailable","usage_complete":false})
            }
        };
        runs.push(record);
    }
    Ok(
        json!({"runs":runs,"usage":usage,"usage_complete":complete,"interruption_gate":tree["blocked"],"epoch":tree["epoch"]}),
    )
}

fn bound_backend_alive(
    pid: u32,
    birth: u64,
    probe: impl FnOnce(u32) -> std::io::Result<crate::native::Identity>,
) -> Result<bool, String> {
    match probe(pid) {
        Ok(identity) => Ok(identity.alive && identity.birth == birth),
        Err(error) if crate::native::missing(&error) => Ok(false),
        Err(_) => Err("cannot verify orphan backend exit".into()),
    }
}
fn recorded_identity(runtime: &Value, role: &str) -> Result<(u32, u64), String> {
    let pid = runtime[format!("{role}_pid")]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| {
            format!("missing native {role} PID identity; refusing ownerless settlement")
        })?;
    let birth = runtime[format!("{role}_birth")]
        .as_u64()
        .filter(|birth| *birth > 0)
        .ok_or_else(|| {
            format!("missing native {role} birth identity; refusing ownerless settlement")
        })?;
    Ok((pid, birth))
}

fn scope_empty(runtime: &Value, backend: u32) -> Result<bool, String> {
    #[cfg(unix)]
    {
        let _ = runtime;
        // process::spawn establishes a separate session/group led by the backend.
        // A dead leader can leave live members. Probe only; never terminate a
        // persisted group (its numeric ID may have been reused).
        let group = i32::try_from(backend)
            .ok()
            .filter(|group| *group > 1)
            .ok_or("invalid retained backend process group")?;
        if unsafe { libc::kill(-group, 0) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(true)
        } else {
            Err(format!("backend scope exit unconfirmed: {error}"))
        }
    }
    #[cfg(windows)]
    {
        let _ = backend;
        let name = runtime["backend_job"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or("missing retained backend job identity")?;
        let session = runtime["backend_job_session"]
            .as_u64()
            .and_then(|session| u32::try_from(session).ok())
            .ok_or("missing retained backend job OS session")?;
        crate::native::windows::job_empty(name, session)
            .map_err(|error| format!("backend scope exit unconfirmed: {error}"))
    }
}

fn terminal(status: &Value) -> bool {
    matches!(
        status.as_str(),
        Some(
            "completed" | "needs_decision" | "incomplete" | "interrupted" | "timed_out" | "failed"
        )
    )
}

// Caller holds run.lock throughout. Match resume's run.lock -> tree.lock order;
// never wait for an owner while holding tree.lock (its finalizer needs that lock).
fn settle_stopped(run: &Path, revision: &Value, _owner: &File) -> Result<Value, String> {
    let mut current = read_json(&run.join("manifest.json"))?;
    let number = current["revision"]
        .as_u64()
        .ok_or("missing retained revision")?;
    if current["revision"] != *revision || current["run_id"] != id(run) {
        return Err("attempt changed during interruption; refusing stale settlement".into());
    }
    let root = tree_root(run, &current)?;
    let _tree_lock = lock(&root)?;
    let mut tree = read_tree(&root)?;
    let member = &tree["members"][id(run)];
    if member["revision"] != *revision {
        return Err("subtree revision does not match interrupted attempt".into());
    }
    if current["status"] == "running" {
        let runtime = read_json(&run.join("runtime.json"))?;
        if runtime["revision"] != *revision {
            return Err("runtime revision does not match interrupted attempt".into());
        }
        let attempts = current["attempts"]
            .as_array()
            .filter(|attempts| !attempts.is_empty())
            .ok_or("missing current attempt evidence")?;
        let attempt = attempts.last().unwrap();
        if attempt["status"] != "running"
            || attempt["logs"].as_str().is_none()
            || if runtime["attempt_logs"].is_null() {
                // Legacy bindings are unambiguous only before any repair/stage retry.
                attempts.len() != 1
            } else {
                runtime["attempt_logs"] != attempt["logs"]
            }
        {
            return Err(
                "runtime does not identify the current attempt; refusing ambiguous recovery".into(),
            );
        }
        let (launcher, birth) = recorded_identity(&runtime, "launcher")?;
        if (!member["launcher_pid"].is_null() || !member["launcher_birth"].is_null())
            && (member["launcher_pid"] != launcher || member["launcher_birth"] != birth)
        {
            return Err("subtree launcher identity does not match interrupted attempt".into());
        }
        if bound_backend_alive(launcher, birth, crate::native::identity)
            .map_err(|e| format!("launcher exit unconfirmed: {e}"))?
        {
            return Err("recorded launcher is still alive; refusing ownerless settlement".into());
        }
        let (backend, birth) = recorded_identity(&runtime, "backend")?;
        if bound_backend_alive(backend, birth, crate::native::identity)? {
            return Err("owner crashed with running state; backend stop unconfirmed; retry interrupt only after the original backend exits".into());
        }
        if !scope_empty(&runtime, backend)? {
            return Err("backend process scope is not empty; refusing ownerless settlement".into());
        }
        let snapshot = run.join(format!("revisions/{number:06}/manifest.json"));
        if snapshot
            .try_exists()
            .map_err(|e| format!("inspect revision snapshot: {e}"))?
        {
            let retained = read_json(&snapshot)?;
            if retained["revision"] != number
                || !terminal(&retained["status"])
                || retained["run_id"] != id(run)
                || retained["parent_run_id"] != current["parent_run_id"]
                || retained["creator_main_session_id"] != current["creator_main_session_id"]
                || retained["thread_id"] != current["thread_id"]
            {
                return Err("invalid crashed revision snapshot; retained for inspection".into());
            }
            current = retained;
        } else {
            let history = current["revisions"]
                .as_array_mut()
                .ok_or("invalid crashed revision history")?;
            if history.iter().any(|entry| entry["revision"] == number) {
                return Err(
                    "crashed revision history has no matching snapshot; retained for inspection"
                        .into(),
                );
            }
            let record = json!({"revision":number,"status":"interrupted","usage":current["revision_usage"],"manifest":format!("revisions/{number:06}/manifest.json"),"recovered":true});
            current["revisions"].as_array_mut().unwrap().push(record);
            current["status"] = json!("interrupted");
            current["active_turn"] = Value::Null;
            current["usage_complete"] = json!(false);
            current["error"] = json!(
                "crashed owner reconciled by explicit interruption after launcher and backend exit"
            );
            save_json(&snapshot, &current)?;
        }
        save_json(&run.join("manifest.json"), &current)?;
    } else if !terminal(&current["status"]) {
        return Err("attempt has no recognized terminal state".into());
    }
    tree["members"][id(run)]["active"] = json!(false);
    save_json(&root.join("tree.json"), &tree)?;
    Ok(json!({"run_id":id(run),"status":current["status"],"stopped":true}))
}

fn owner_file(run: &Path) -> Result<File, String> {
    let path = run.join("run.lock");
    crate::lifecycle::private_path(&path, false)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("owner lock unavailable: {e}"))
}

pub fn interrupt(run: &Path, recursive: bool) -> Result<Value, String> {
    interrupt_expected(run, recursive, None)
}

pub(super) fn interrupt_bound(
    run: &Path,
    recursive: bool,
    target: &Value,
) -> Result<Value, String> {
    interrupt_expected(run, recursive, Some(target))
}

fn interrupt_expected(
    run: &Path,
    recursive: bool,
    target: Option<&Value>,
) -> Result<Value, String> {
    let manifest = read_json(&run.join("manifest.json"))?;
    if !recursive {
        let attempt = match target {
            Some(expected) => {
                crate::app_server::control_bound(run, "interrupt", "", None, expected)
            }
            None => crate::app_server::control(run, "interrupt", "", None),
        };
        let mut receipt = match attempt {
            Ok(receipt) => receipt,
            Err(error) if manifest["status"] == "running" => {
                // A missing endpoint is not exit evidence. The exclusive lock and
                // both birth-bound native identities must independently prove it.
                let recovery = (|| {
                    let owner = owner_file(run)?;
                    owner
                        .try_lock()
                        .map_err(|e| format!("owner lock unavailable: {e}"))?;
                    if let Some(expected) = target {
                        crate::app_server::require_control_target(run, expected)?;
                    }
                    settle_stopped(run, &manifest["revision"], &owner)
                })();
                recovery
                    .map_err(|reason| format!("{error}; ownerless settlement refused: {reason}"))?
            }
            Err(error) => return Err(error),
        };
        receipt["active_children_left_running"] = json!(
            summary(run)?["runs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["active"] == true)
                .map(|r| r["run_id"].clone())
                .collect::<Vec<_>>()
        );
        return Ok(receipt);
    }
    let root = tree_root(run, &manifest)?;
    let targets = close_gate(run, &manifest, target)?;
    // Gate watchers cover startup, repairs and stale/missing endpoints. Wait on the
    // native owner lock, not an acceptance receipt; never signal a persisted PID.
    let mut results = Vec::new();
    for (target, revision) in targets {
        let child = root.parent().unwrap().join(&target);
        let deadline = Instant::now() + Duration::from_secs(12);
        let result = (|| -> Result<Value, String> {
            let file = owner_file(&child)?;
            if root != run {
                let _ = crate::app_server::control(&child, "interrupt", "", None);
            }
            loop {
                match file.try_lock() {
                    Ok(()) => break,
                    Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    Err(e) => return Err(format!("owner did not confirm stopped: {e}")),
                }
            }
            settle_stopped(&child, &revision, &file)
        })();
        results.push(match result {
            Ok(result) => result,
            Err(error) => json!({"run_id":target,"stopped":false,"error":error}),
        });
    }
    let stopped = results.iter().all(|r| r["stopped"] == true);
    let receipt = json!({"status":if stopped {"stopped"} else {"incomplete"},"operation":"interrupt","recursive":true,"run_id":id(run),"results":results});
    if stopped {
        Ok(receipt)
    } else {
        Err(receipt.to_string())
    }
}

// Publication/admission take the same tree lock. An A-target gate cannot be
// checked, then applied after B becomes visible in the same revision.
fn close_gate(
    run: &Path,
    manifest: &Value,
    target: Option<&Value>,
) -> Result<Vec<(String, Value)>, String> {
    let root = tree_root(run, manifest)?;
    let _lock = lock(&root)?;
    let path = root.join("tree.json");
    let mut tree = if path.exists() {
        read_tree(&root)?
    } else {
        json!({"version":1,"blocked":false,"epoch":0,"members":{}})
    };
    if let Some(expected) = target {
        crate::app_server::require_control_target(run, expected)?;
        if tree["epoch"] != expected["subtree_epoch"]
            || tree["members"][id(run)]["revision"] != expected["revision"]
        {
            return Err(
                "stale signed recursive control; subtree admission epoch/revision changed".into(),
            );
        }
    }
    #[cfg(test)]
    {
        let hook = AFTER_BOUND_CHECK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
    let targets: Vec<String> = if root == run {
        tree["blocked"] = json!(true);
        let mut ids: Vec<String> = tree["members"]
            .as_object()
            .ok_or("invalid subtree members")?
            .keys()
            .cloned()
            .collect();
        if !ids.iter().any(|v| v == id(run)) {
            ids.push(id(run).to_owned());
        }
        ids
    } else {
        tree["members"][id(run)]["blocked"] = json!(true);
        vec![id(run).to_owned()]
    };
    let attempts: Vec<_> = targets
        .into_iter()
        .map(|target| {
            let revision = if tree["members"][&target]["revision"].is_null() && target == id(run) {
                manifest["revision"].clone()
            } else {
                tree["members"][&target]["revision"].clone()
            };
            (target, revision)
        })
        .collect();
    save_json(&path, &tree)?;
    Ok(attempts)
}
#[cfg(test)]
pub(super) fn close_bound_gate_for_test(run: &Path, target: &Value) -> Result<(), String> {
    close_gate(run, &read_json(&run.join("manifest.json"))?, Some(target)).map(|_| ())
}

// Shared Team clauses are copied verbatim except coordinator-specific authority.
pub const POLICY: &str = include_str!("coordinator.md");

#[cfg(test)]
mod tests {
    use super::*;
    include!("native_boundary_tests.rs");
    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn coordinator_writing_flag_accepts_no_value() {
        assert!(
            invocation_options(
                &arguments(&["resume", "child", "--inject-write"]),
                Path::new("."),
            )
            .is_ok()
        );
    }

    #[test]
    fn coordinator_writing_flag_cannot_hide_subteam_grant() {
        let error = invocation_options(
            &arguments(&["run", "--inject-write", "--allow-subteam"]),
            Path::new("."),
        )
        .unwrap_err();
        assert_eq!(error, "coordinators cannot grant --allow-subteam");
    }

    #[test]
    fn authorization_distinguishes_task_data_from_options() {
        let store = std::env::current_dir().unwrap().canonicalize().unwrap();
        for args in [
            arguments(&["run", "--task", "--allow-subteam", "--completion-gate", "x"]),
            arguments(&["run", "--task=--allow-subteam", "--completion-gate", "x"]),
            arguments(&["run", "--completion-gate", "x", "--", "--allow-subteam"]),
            arguments(&[
                "resume",
                "child",
                "--task",
                "--store",
                "--completion-gate",
                "x",
            ]),
        ] {
            invocation_options(&args, &store).unwrap();
        }
        for args in [
            arguments(&["run", "--allow-subteam", "--task", "x"]),
            arguments(&["resume", "child", "--allow-subteam"]),
            arguments(&["read", "child", "--store", "/"]),
        ] {
            assert!(invocation_options(&args, &store).is_err());
        }
    }
    #[test]
    fn windows_argument_adapter_preserves_json_paths_and_rejects_task_hints() {
        let path = PathBuf::from(r"C:\Program Files\Team\run");
        let mut args = arguments(&[
            "codex.exe",
            "app-server",
            "--strict-config",
            "--stdio",
            "-c",
        ]);
        args.push(format!("{MARKER}{}", json!(path)));
        assert_eq!(marker(&args).unwrap(), Some(path));
        let forged = arguments(&[
            "pira_team.exe",
            "run",
            "--task",
            r#"app-server --strict-config --stdio shell_environment_policy.set.PIRA_TEAM_CONTEXT="C:\bad""#,
        ]);
        assert!(marker(&forged).unwrap().is_none());
    }
    #[test]
    fn windows_process_adapter_binds_birth_revision_and_parent_chain() {
        let run = std::env::temp_dir().join(format!(
            "team-win-adapter-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::private_dir(&run).unwrap();
        save_json(
            &run.join("runtime.json"),
            &json!({"backend_pid":12,"backend_birth":20,"revision":2}),
        )
        .unwrap();
        save_json(&run.join("manifest.json"), &json!({"revision":2})).unwrap();
        let probe = |pid| {
            Ok((
                crate::native::Identity {
                    pid,
                    parent: if pid == 32 { 12 } else { 1 },
                    birth: if pid == 32 { 30 } else { 20 },
                    executable: PathBuf::from(r"C:\Program Files\Codex.exe"),
                    alive: true,
                },
                (pid == 12).then_some(run.clone()),
            ))
        };
        assert_eq!(
            discover_using(32, probe).unwrap().unwrap().run,
            run.canonicalize().unwrap()
        );
        save_json(
            &run.join("runtime.json"),
            &json!({"backend_pid":12,"backend_birth":19,"revision":2}),
        )
        .unwrap();
        assert!(discover_using(32, probe).is_err());
        save_json(
            &run.join("runtime.json"),
            &json!({"backend_pid":12,"backend_birth":20,"revision":1}),
        )
        .unwrap();
        assert!(discover_using(32, probe).is_err());
        assert!(
            discover_using(32, |pid| Ok((
                crate::native::Identity {
                    pid,
                    parent: 12,
                    birth: if pid == 32 { 10 } else { 20 },
                    executable: PathBuf::new(),
                    alive: true
                },
                Some(run.clone())
            )))
            .unwrap()
            .is_none()
        );
        std::fs::remove_dir_all(run).unwrap();
    }

    struct DiscoveryFixture {
        run: PathBuf,
        processes: Vec<(crate::native::Identity, Option<PathBuf>)>,
    }
    impl DiscoveryFixture {
        fn new(owner: u32) -> Self {
            let run = std::env::temp_dir().join(format!(
                "team-discovery-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            crate::private_dir(&run).unwrap();
            save_json(
                &run.join("runtime.json"),
                &json!({"backend_pid":owner,"backend_birth":owner,"revision":2}),
            )
            .unwrap();
            save_json(&run.join("manifest.json"), &json!({"revision":2})).unwrap();
            let processes = [(32, 24), (24, 16), (16, 12), (12, 1)]
                .into_iter()
                .map(|(pid, parent)| {
                    (
                        crate::native::Identity {
                            pid,
                            parent,
                            birth: u64::from(pid),
                            executable: PathBuf::new(),
                            alive: true,
                        },
                        (pid == 24).then_some(run.clone()),
                    )
                })
                .collect();
            Self { run, processes }
        }
        fn inspect(&self, pid: u32) -> Result<(crate::native::Identity, Option<PathBuf>), String> {
            self.processes
                .iter()
                .find(|(identity, _)| identity.pid == pid)
                .cloned()
                .ok_or_else(|| "unexpected fixture PID".into())
        }
        fn discover(&self) -> Result<Option<Caller>, String> {
            discover_using(32, |pid| self.inspect(pid))
        }
    }
    impl Drop for DiscoveryFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.run).unwrap();
        }
    }

    #[test]
    fn discovery_accepts_direct_and_wrapped_retained_owners() {
        for owner in [24, 16, 12] {
            let mut fixture = DiscoveryFixture::new(owner);
            // Repeated matching markers on wrappers must not change the binding.
            fixture.processes[2].1 = Some(fixture.run.clone());
            let caller = fixture.discover().unwrap().unwrap();
            assert_eq!(caller.run, fixture.run.canonicalize().unwrap());
            assert_eq!(caller.runtime["backend_pid"], owner);
        }
    }

    #[test]
    fn discovery_rejects_malformed_birth() {
        for owner in [24, 12] {
            let fixture = DiscoveryFixture::new(owner);
            let path = fixture.run.join("runtime.json");
            let valid = read_json(&path).unwrap();
            for birth in [
                None,
                Some(Value::Null),
                Some(json!("24")),
                Some(json!(-1)),
                Some(json!(24.0)),
                Some(json!(true)),
                Some(json!({})),
                Some(json!([])),
            ] {
                let mut runtime = valid.clone();
                runtime.as_object_mut().unwrap().remove("backend_birth");
                if let Some(value) = &birth {
                    runtime["backend_birth"] = value.clone();
                }
                save_json(&path, &runtime).unwrap();
                assert!(
                    fixture.discover().is_err(),
                    "owner={owner}, birth={birth:?}"
                );
            }
        }
    }

    #[test]
    fn discovery_rejects_malformed_revisions() {
        let fixture = DiscoveryFixture::new(24);
        let runtime_path = fixture.run.join("runtime.json");
        let manifest_path = fixture.run.join("manifest.json");
        let valid = read_json(&runtime_path).unwrap();
        for revision in [
            None,
            Some(Value::Null),
            Some(json!("2")),
            Some(json!(-1)),
            Some(json!(2.0)),
            Some(json!(true)),
            Some(json!({})),
            Some(json!([])),
        ] {
            for targets in [
                vec!["runtime"],
                vec!["manifest"],
                vec!["runtime", "manifest"],
            ] {
                let mut runtime = valid.clone();
                let mut manifest = json!({"revision":2});
                for (name, value) in [("runtime", &mut runtime), ("manifest", &mut manifest)] {
                    if targets.contains(&name) {
                        value.as_object_mut().unwrap().remove("revision");
                        if let Some(revision) = &revision {
                            value["revision"] = revision.clone();
                        }
                    }
                }
                save_json(&runtime_path, &runtime).unwrap();
                save_json(&manifest_path, &manifest).unwrap();
                assert!(
                    fixture.discover().is_err(),
                    "targets={targets:?}, revision={revision:?}"
                );
            }
        }
    }

    #[test]
    fn discovery_rejects_unrelated_or_recycled_owner() {
        for (field, value) in [("backend_pid", 99), ("backend_birth", 13)] {
            let fixture = DiscoveryFixture::new(12);
            let path = fixture.run.join("runtime.json");
            let mut runtime = read_json(&path).unwrap();
            runtime[field] = json!(value);
            save_json(&path, &runtime).unwrap();
            assert!(fixture.discover().is_err(), "{field}");
        }
    }

    #[test]
    fn discovery_rejects_conflicting_context() {
        let mut fixture = DiscoveryFixture::new(12);
        let other = DiscoveryFixture::new(12);
        fixture.processes[2].1 = Some(other.run.clone());
        assert!(fixture.discover().is_err());
    }

    #[test]
    fn discovery_rejects_broken_wrapper_chain() {
        for broken in [
            "dead wrapper",
            "dead owner",
            "birth order",
            "missing owner",
            "cycle",
        ] {
            let mut fixture = DiscoveryFixture::new(12);
            match broken {
                "dead wrapper" => fixture.processes[2].0.alive = false,
                "dead owner" => fixture.processes[3].0.alive = false,
                "birth order" => fixture.processes[2].0.birth = 25,
                "missing owner" => fixture.processes[2].0.parent = 1,
                "cycle" => fixture.processes[2].0.parent = 16,
                _ => unreachable!(),
            }
            assert!(fixture.discover().is_err(), "{broken}");
        }
    }

    #[test]
    fn discovery_propagates_inspection_errors() {
        let fixture = DiscoveryFixture::new(12);
        let result = discover_using(32, |pid| {
            if pid == 16 {
                Err("cannot verify process identity".into())
            } else {
                fixture.inspect(pid)
            }
        });
        assert_eq!(
            result.err().as_deref(),
            Some("cannot verify process identity")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_native_ancestry_probe_is_available() {
        assert!(discover_from(std::process::id()).is_ok());
    }

    #[test]
    fn backend_exit_probe_never_confuses_reused_or_inaccessible_pids() {
        let identity = |birth, alive| crate::native::Identity {
            pid: 12,
            parent: 1,
            birth,
            executable: PathBuf::new(),
            alive,
        };
        assert!(bound_backend_alive(12, 20, |_| Ok(identity(20, true))).unwrap());
        assert!(!bound_backend_alive(12, 20, |_| Ok(identity(21, true))).unwrap());
        assert!(!bound_backend_alive(12, 20, |_| Ok(identity(20, false))).unwrap());
        assert!(
            !bound_backend_alive(12, 20, |_| Err(std::io::Error::from(
                std::io::ErrorKind::NotFound
            )))
            .unwrap()
        );
        assert!(
            bound_backend_alive(12, 20, |_| Err(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            )))
            .is_err()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unlinked_live_executable_never_becomes_missing_worker_or_exited_backend() {
        struct Probe {
            child: std::process::Child,
            dir: PathBuf,
        }
        impl Drop for Probe {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "team-unlinked-executable-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let executable = dir.join("codex-fixture");
        std::fs::copy("/bin/sleep", &executable).unwrap();
        let probe = Probe {
            child: std::process::Command::new(&executable)
                .arg("30")
                .spawn()
                .unwrap(),
            dir,
        };
        let pid = probe.child.id();
        let original = crate::native::identity(pid).unwrap();
        assert!(original.alive);
        std::fs::remove_file(executable).unwrap();
        let error = crate::native::identity(pid).unwrap_err();
        assert!(!crate::native::missing(&error), "{error}");
        assert!(inspect(pid).is_err());
        assert!(bound_backend_alive(pid, original.birth, crate::native::identity).is_err());
    }
}
