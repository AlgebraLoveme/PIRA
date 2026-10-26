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

#[cfg(any(windows, test))]
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
    let initial = pid;
    let mut child_birth = u64::MAX;
    for _ in 0..64 {
        let (identity, candidate) = inspect(pid)?;
        if !identity.alive || identity.birth > child_birth {
            return Ok(None);
        }
        if pid != initial
            && let Some(path) = candidate
        {
            let run = path
                .canonicalize()
                .map_err(|_| "Team context unavailable")?;
            let runtime = read_json(&run.join("runtime.json"))?;
            let manifest = read_json(&run.join("manifest.json"))?;
            if runtime["backend_pid"] != pid
                || runtime["revision"] != manifest["revision"]
                || runtime["backend_birth"]
                    .as_u64()
                    .is_some_and(|born| born != identity.birth)
            {
                return Err("Team runtime does not match actual worker ancestry".into());
            }
            return Ok(Some(Caller { run, runtime }));
        }
        child_birth = identity.birth;
        pid = identity.parent;
        if pid <= if cfg!(windows) { 4 } else { 1 } {
            return Ok(None);
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

pub fn bind(run: &Path, revision: &Value, scope: &crate::process::Scope) -> Result<(), String> {
    let backend =
        crate::native::identity(scope.pid).map_err(|_| "cannot bind native backend identity")?;
    let launcher = crate::native::current()?;
    save_json(
        &run.join("runtime.json"),
        &json!({"backend_pid":scope.pid,"backend_birth":backend.birth,
        "launcher_pid":launcher.pid,"launcher_birth":launcher.birth,"backend_job":scope.job_name(),"revision":revision}),
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
fn backend_alive(run: &Path) -> Result<bool, String> {
    let runtime = read_json(&run.join("runtime.json"))?;
    let pid = runtime["backend_pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or("missing native backend identity")?;
    if let Some(birth) = runtime["backend_birth"].as_u64() {
        return bound_backend_alive(pid, birth, crate::native::identity);
    }
    #[cfg(windows)]
    {
        Err("legacy backend exit cannot be verified; inspect the original invocation".into())
    }
    #[cfg(unix)]
    {
        let output = std::process::Command::new("/bin/ps")
            .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
            .output()
            .map_err(|e| format!("verify orphan backend exit: {e}"))?;
        if !output.status.success() {
            if output.status.code() == Some(1)
                && output.stdout.is_empty()
                && output.stderr.is_empty()
            {
                return Ok(false);
            }
            return Err("could not verify orphan backend exit".into());
        }
        let command =
            String::from_utf8(output.stdout).map_err(|_| "invalid backend process evidence")?;
        if let Some((_, value)) = command.split_once(MARKER) {
            let path = serde_json::Deserializer::from_str(value)
                .into_iter::<String>()
                .next()
                .ok_or("missing orphan context")?
                .map_err(|_| "invalid orphan context")?;
            return Ok(Path::new(&path) == run);
        }
        Ok(false)
    }
}

pub fn interrupt(run: &Path, recursive: bool) -> Result<Value, String> {
    let manifest = read_json(&run.join("manifest.json"))?;
    if !recursive {
        let mut receipt = crate::app_server::control(run, "interrupt", "", None)?;
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
    let targets = {
        let _lock = lock(&root)?;
        let path = root.join("tree.json");
        let mut tree = if path.exists() {
            read_tree(&root)?
        } else {
            json!({"version":1,"blocked":false,"epoch":0,"members":{}})
        };
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
        save_json(&path, &tree)?;
        targets
    };
    // Gate watchers cover startup, repairs and stale/missing endpoints. Wait on the
    // native owner lock, not an acceptance receipt; never signal a persisted PID.
    let mut results = Vec::new();
    for target in targets {
        let child = root.parent().unwrap().join(&target);
        let deadline = Instant::now() + Duration::from_secs(12);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(child.join("run.lock"));
        let result = (|| -> Result<Value, String> {
            let file = file.map_err(|e| format!("owner lock unavailable: {e}"))?;
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
            let mut current = read_json(&child.join("manifest.json"))?;
            if current["status"] == "running" {
                // A crashed launcher cannot prove its backend stopped. Preserve the
                // ambiguity instead of claiming completion or killing by stale PID.
                if backend_alive(&child)? {
                    return Err("owner crashed with running state; backend stop unconfirmed; descendants gated and interrupted; retry recursive interrupt after the original backend exits".into());
                }
                let number = current["revision"]
                    .as_u64()
                    .ok_or("missing crashed revision")?;
                let snapshot = child.join(format!("revisions/{number:06}/manifest.json"));
                if snapshot.exists() {
                    let terminal = read_json(&snapshot)?;
                    if terminal["revision"] != number
                        || terminal["status"] == "running"
                        || terminal["run_id"] != target
                    {
                        return Err(
                            "invalid crashed revision snapshot; retained for inspection".into()
                        );
                    }
                    current = terminal;
                } else {
                    current["status"] = json!("interrupted");
                    current["active_turn"] = Value::Null;
                    current["usage_complete"] = json!(false);
                    current["error"] = json!(
                        "crashed owner reconciled by recursive interruption after backend exit"
                    );
                    let record = json!({"revision":number,"status":"interrupted","usage":current["revision_usage"],"manifest":format!("revisions/{number:06}/manifest.json"),"recovered":true});
                    current["revisions"]
                        .as_array_mut()
                        .ok_or("invalid crashed revision history")?
                        .push(record);
                    save_json(&snapshot, &current)?;
                }
                save_json(&child.join("manifest.json"), &current)?;
            }
            let _lock = lock(&root)?;
            let mut tree = read_tree(&root)?;
            tree["members"][&target]["active"] = json!(false);
            tree["members"][&target]["revision"] = current["revision"].clone();
            save_json(&root.join("tree.json"), &tree)?;
            current["run_id"] = json!(target);
            Ok(json!({"run_id":target,"status":current["status"],"stopped":true}))
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

// Shared Team clauses are copied verbatim except coordinator-specific authority.
pub const POLICY: &str = include_str!("coordinator.md");

#[cfg(test)]
mod tests {
    use super::*;
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
}
