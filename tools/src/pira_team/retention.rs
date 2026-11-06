//! Store-local retention. Foreground callers only spawn; all inspection is here.
use crate::{lifecycle, native::FileLock, storage, subteam};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const AGE_MS: u64 = 10 * 24 * 60 * 60 * 1000;
const BUDGET: u64 = 50_000_000_000;
const PREFIX: &str = ".retiring-";

fn now_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock precedes Unix epoch".to_owned())?
        .as_millis()
        .try_into()
        .map_err(|_| "clock overflow".into())
}

pub(super) fn activity(manifest: &mut Value) -> Result<(), String> {
    manifest["last_execution_activity_ms"] = json!(now_ms()?);
    Ok(())
}

fn marker(root: &Path, id: &str) -> PathBuf {
    root.join(format!("{PREFIX}{id}.json"))
}

pub(super) fn available(run: &Path) -> Result<(), String> {
    if exists(&marker(
        run.parent().ok_or("missing store")?,
        run.file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid run path")?,
    ))? {
        return Err("run is being retired by retention".into());
    }
    Ok(())
}

// Windows junctions and other reparse points are also boundaries, never traversed.
fn link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn real(path: &Path, directory: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "metadata_unavailable")?;
    if link(&meta) {
        return Err("symlink_or_reparse_metadata".into());
    }
    lifecycle::private_path(path, directory).map_err(|_| "unsafe_metadata".into())
}

fn read(path: &Path) -> Result<Value, String> {
    real(path, false)?;
    lifecycle::read_json(path).map_err(|_| "invalid_metadata".into())
}

fn lock(path: &Path, create: bool) -> Result<FileLock, String> {
    if !create || path.try_exists().map_err(|_| "lock_unavailable")? {
        real(path, false)?;
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(create)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| "lock_unavailable")?;
    match file.try_lock() {
        Ok(()) => Ok(FileLock(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err("live_owner_or_busy_lock".into()),
        Err(_) => Err("lock_unavailable".into()),
    }
}

#[derive(Default, Clone, Copy)]
struct Size {
    bytes: u64,
    unknown: bool,
}
impl Size {
    fn add(&mut self, other: Self) {
        match self.bytes.checked_add(other.bytes) {
            Some(sum) => self.bytes = sum,
            None => {
                self.bytes = u64::MAX;
                self.unknown = true;
            }
        }
        self.unknown |= other.unknown;
    }
}

// Count logical file bytes, including link objects but never their targets.
// PIRA: this is an estimate, not filesystem allocation accounting (sparse files,
// compression and shared hard links can differ). No file payloads are opened.
fn size(path: &Path) -> Size {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Size {
            bytes: 0,
            unknown: true,
        };
    };
    if link(&meta) || meta.is_file() {
        return Size {
            bytes: meta.len(),
            unknown: false,
        };
    }
    if !meta.is_dir() {
        return Size {
            bytes: 0,
            unknown: true,
        };
    }
    // Pin each directory while enumerating. Unix uses its descriptor as the
    // recursion anchor, so a concurrently replaced scratch directory cannot
    // redirect traversal. Windows denies delete/rename while this handle lives.
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3).custom_flags(0x02000000 | 0x00200000);
    }
    let Ok(directory) = options.open(path) else {
        return Size {
            bytes: 0,
            unknown: true,
        };
    };
    if directory
        .metadata()
        .map_or(true, |m| link(&m) || !m.is_dir())
    {
        return Size {
            bytes: 0,
            unknown: true,
        };
    }
    #[cfg(unix)]
    {
        directory_size(directory)
    }
    #[cfg(windows)]
    {
        let Ok(entries) = fs::read_dir(path) else {
            return Size {
                bytes: 0,
                unknown: true,
            };
        };
        let mut total = Size::default();
        for entry in entries {
            total.add(match entry {
                Ok(entry) => size(&entry.path()),
                Err(_) => Size {
                    bytes: 0,
                    unknown: true,
                },
            });
        }
        total
    }
}

#[cfg(unix)]
fn directory_size(directory: File) -> Size {
    use std::os::fd::{FromRawFd, IntoRawFd};
    struct Entries(*mut libc::DIR);
    impl Drop for Entries {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let fd = directory.into_raw_fd();
    let entries = unsafe { libc::fdopendir(fd) };
    if entries.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Size {
            bytes: 0,
            unknown: true,
        };
    }
    let entries = Entries(entries);
    let mut total = Size::default();
    loop {
        // readdir uses errno to distinguish end-of-directory from failure.
        #[cfg(target_os = "macos")]
        let errno = unsafe { libc::__error() };
        #[cfg(target_os = "linux")]
        let errno = unsafe { libc::__errno_location() };
        unsafe {
            *errno = 0;
        }
        let entry = unsafe { libc::readdir(entries.0) };
        if entry.is_null() {
            total.unknown |= unsafe { *errno != 0 };
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                fd,
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            total.unknown = true;
            continue;
        }
        let stat = unsafe { stat.assume_init() };
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFDIR => {
                let child = unsafe {
                    libc::openat(
                        fd,
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
                    )
                };
                if child < 0 {
                    total.unknown = true;
                } else {
                    total.add(directory_size(unsafe { File::from_raw_fd(child) }));
                }
            }
            libc::S_IFREG | libc::S_IFLNK => {
                total.add(match u64::try_from(stat.st_size) {
                    Ok(bytes) => Size {
                        bytes,
                        unknown: false,
                    },
                    Err(_) => Size {
                        bytes: 0,
                        unknown: true,
                    },
                });
            }
            _ => total.unknown = true,
        }
    }
    total
}

fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("metadata_unavailable".into()),
    }
}

fn explicit_activity(manifest: &Value, now: u64) -> Result<Option<u64>, String> {
    let mut latest = None;
    // Existing schemas have no wall-clock fields. Recognize additive execution
    // fields, never creation/access times. Invalid/future evidence is ambiguous.
    for key in [
        "last_execution_activity_ms",
        "execution_started_at_ms",
        "resumed_at_ms",
        "completed_at_ms",
    ] {
        if let Some(value) = manifest.get(key) {
            let time = value
                .as_u64()
                .filter(|n| *n > 0 && *n <= now)
                .ok_or("invalid_activity")?;
            latest = Some(latest.unwrap_or(0).max(time));
        }
    }
    Ok(latest)
}

fn log_activity(dir: &Path, now: u64) -> Result<Option<u64>, String> {
    real(dir, true)?;
    let mut latest = None;
    for name in [
        "events.jsonl",
        "requests.jsonl",
        "stderr.log",
        "controls.jsonl",
        "backend-check.log",
    ] {
        let path = dir.join(name);
        if !exists(&path)? {
            continue;
        }
        real(&path, false)?;
        let time: u64 = fs::symlink_metadata(&path)
            .and_then(|m| m.modified())
            .map_err(|_| "unknown_activity")?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "invalid_activity")?
            .as_millis()
            .try_into()
            .map_err(|_| "invalid_activity")?;
        if time == 0 || time > now {
            return Err("invalid_activity".into());
        }
        latest = Some(latest.unwrap_or(0).max(time));
    }
    for child in ["implementation", "repair"] {
        let path = dir.join(child);
        if exists(&path)?
            && let Some(time) = log_activity(&path, now)?
        {
            latest = Some(latest.unwrap_or(0).max(time));
        }
    }
    Ok(latest)
}

fn last_activity(run: &Path, manifest: &Value, now: u64) -> Result<u64, String> {
    let mut latest = explicit_activity(manifest, now)?;
    let number = manifest["revision"]
        .as_u64()
        .filter(|n| (1..=999999).contains(n))
        .ok_or("unknown_activity")?;
    let current = if number == 1 {
        run.to_owned()
    } else {
        if exists(&run.join("revisions"))? {
            real(&run.join("revisions"), true)?;
        }
        run.join(format!("revisions/{number:06}"))
    };
    // Require timing in the latest revision: older logs cannot date a failed
    // resume/preflight that produced no execution log of its own.
    if exists(&current)? {
        real(&current, true)?;
        if exists(&current.join("manifest.json"))? {
            let snapshot = read(&current.join("manifest.json"))?;
            if let Some(time) = explicit_activity(&snapshot, now)? {
                latest = Some(latest.unwrap_or(0).max(time));
            }
        }
        if let Some(time) = log_activity(&current, now)? {
            latest = Some(latest.unwrap_or(0).max(time));
        }
    }
    latest.ok_or_else(|| "unknown_activity".into())
}

fn inactive(run: &Path, manifest: &Value, member: Option<&Value>) -> Result<(), String> {
    // Native identities screen candidates. Deletion additionally holds every
    // owner lock, including for legacy terminal records without runtime bindings.
    if let Some(member) = member {
        if member["revision"] != manifest["revision"] {
            return Err("unit_revision_mismatch".into());
        }
        if member["active"] == true {
            let (pid, birth) =
                subteam::recorded_identity(member, "launcher").map_err(|_| "unknown_launcher")?;
            if subteam::bound_backend_alive(pid, birth, crate::native::identity)
                .map_err(|_| "unknown_launcher")?
            {
                return Err("live_launcher".into());
            }
        }
    }
    let path = run.join("runtime.json");
    if exists(&path)? {
        let runtime = read(&path)?;
        inactive_runtime(&runtime, &manifest["revision"])?;
    } else if !subteam::terminal(&manifest["status"]) {
        return Err("unknown_native_state".into());
    }
    Ok(())
}

fn inactive_runtime(runtime: &Value, revision: &Value) -> Result<(), String> {
    if runtime["revision"] != *revision {
        return Err("runtime_revision_mismatch".into());
    }
    for role in ["launcher", "backend"] {
        let (pid, birth) =
            subteam::recorded_identity(&runtime, role).map_err(|_| "unknown_native_identity")?;
        if subteam::bound_backend_alive(pid, birth, crate::native::identity)
            .map_err(|_| "unknown_native_identity")?
        {
            return Err(format!("live_{role}"));
        }
    }
    let (pid, _) =
        subteam::recorded_identity(&runtime, "backend").map_err(|_| "unknown_native_identity")?;
    if !subteam::scope_empty(&runtime, pid).map_err(|_| "unknown_backend_scope")? {
        return Err("live_backend_scope".into());
    }
    Ok(())
}

fn runtime_proof(runtime: &Value) -> Value {
    let mut proof = json!({});
    for key in [
        "revision",
        "launcher_pid",
        "launcher_birth",
        "backend_pid",
        "backend_birth",
        "backend_job",
        "backend_job_session",
    ] {
        if let Some(value) = runtime.get(key) {
            proof[key] = value.clone();
        }
    }
    proof
}

fn proofs(root: &Path, unit: &Unit) -> Result<Value, String> {
    let mut proofs = json!({});
    for id in &unit.members {
        let run = root.join(id);
        let manifest = read(&run.join("manifest.json"))?;
        let runtime = if exists(&run.join("runtime.json"))? {
            runtime_proof(&read(&run.join("runtime.json"))?)
        } else {
            Value::Null
        };
        proofs[id] =
            json!({"revision":manifest["revision"],"status":manifest["status"],"runtime":runtime});
    }
    Ok(proofs)
}

#[derive(Clone)]
struct Unit {
    id: String,
    members: Vec<String>,
    last: Option<u64>,
    size: Size,
    skip: Option<String>,
    reasons: Vec<&'static str>,
    retiring: bool,
    proofs: Option<Value>,
}
impl Unit {
    fn value(&self) -> Value {
        json!({"root_id":self.id,"member_count":self.members.len(),"members":self.members,
            "last_activity_ms":self.last,"estimated_bytes":self.size.bytes,
            "size_unknown":self.size.unknown,"skip":self.skip,"reasons":self.reasons,"pending_recovery":self.retiring})
    }
}

// Inspection of ordinary young units never takes execution locks. Only selected
// candidates enter this ownership critical section, and deletion revalidates again.
fn inspect(root: &Path, unit: &Unit, now: u64) -> Result<(u64, Vec<FileLock>), String> {
    let mut locks = Vec::new();
    for id in &unit.members {
        let run = root.join(id);
        real(&run, true)?;
        locks.push(lock(&run.join("run.lock"), false)?);
        real(&run, true)?;
    }
    let run = root.join(&unit.id);
    real(&run, true)?;
    if exists(&run.join("tree.json"))? {
        locks.push(lock(&run.join("tree.lock"), false)?);
    }
    Ok((snapshot(root, unit, now)?, locks))
}

fn snapshot(root: &Path, unit: &Unit, now: u64) -> Result<u64, String> {
    for id in &unit.members {
        real(&root.join(id), true)?;
    }
    let run = root.join(&unit.id);
    real(&run, true)?;
    available(&run).map_err(|_| "retirement_pending")?;
    let root_manifest = read(&run.join("manifest.json"))?;
    if !root_manifest["parent_run_id"].is_null() {
        return Err("unit_relationships".into());
    }
    let tree = if exists(&run.join("tree.json"))? {
        let tree = subteam::read_tree(&run).map_err(|_| "unit_relationships")?;
        let expected: BTreeSet<_> = unit.members.iter().map(String::as_str).collect();
        let actual: BTreeSet<_> = tree["members"]
            .as_object()
            .ok_or("unit_relationships")?
            .keys()
            .map(String::as_str)
            .collect();
        if expected != actual {
            return Err("unit_relationships".into());
        }
        Some(tree)
    } else {
        if unit.members.len() != 1 || root_manifest["allow_subteam"] == true {
            return Err("unit_relationships".into());
        }
        None
    };
    let mut latest = 0;
    for id in &unit.members {
        let run = root.join(id);
        let manifest = read(&run.join("manifest.json"))?;
        if manifest["run_id"] != id.as_str()
            || (id != &unit.id && manifest["parent_run_id"] != unit.id)
        {
            return Err("unit_relationships".into());
        }
        inactive(&run, &manifest, tree.as_ref().map(|t| &t["members"][id]))?;
        latest = latest.max(last_activity(&run, &manifest, now)?);
    }
    Ok(latest)
}

fn select(units: &mut [Unit], total: u64, now: u64, budget: u64) -> u64 {
    units.sort_by(|a, b| a.last.cmp(&b.last).then(a.id.cmp(&b.id)));
    let mut remaining = total.saturating_sub(
        units
            .iter()
            .filter(|u| u.retiring && u.skip.is_none())
            .map(|u| u.size.bytes)
            .sum::<u64>(),
    );
    for unit in units {
        if unit.skip.is_some() || unit.retiring {
            continue;
        }
        if unit
            .last
            .is_some_and(|time| now.saturating_sub(time) > AGE_MS)
        {
            unit.reasons.push("age");
        } else if remaining >= budget {
            unit.reasons.push("storage");
        }
        if !unit.reasons.is_empty() {
            remaining = remaining.saturating_sub(unit.size.bytes);
        }
    }
    total.saturating_sub(remaining)
}

struct Plan {
    units: Vec<Unit>,
    total: Size,
    overhead: Size,
    reclaim: u64,
    recovery_issues: Vec<Value>,
}
impl Plan {
    fn value(&self, now: u64) -> Value {
        let skipped: Vec<_> = self.units.iter().filter(|u| u.skip.is_some()).collect();
        let mut counts = BTreeMap::<&str, usize>::new();
        for unit in &skipped {
            *counts.entry(unit.skip.as_deref().unwrap()).or_default() += 1;
        }
        json!({"as_of_ms":now,"max_age_ms":AGE_MS,"target_bytes":BUDGET,"size_basis":"logical file bytes; no symlink targets",
            "summary":{"total_estimated_bytes":self.total.bytes,"total_size_unknown":self.total.unknown,
                "reclaim_estimated_bytes":self.reclaim,"remaining_estimated_bytes":self.total.bytes.saturating_sub(self.reclaim),
                "selected_units":self.units.iter().filter(|u| !u.reasons.is_empty()).count(),
                "selected_members":self.units.iter().filter(|u| !u.reasons.is_empty()).map(|u|u.members.len()).sum::<usize>(),
                "skipped_units":skipped.len(),"protected_estimated_bytes":skipped.iter().fold(0_u64,|sum,u|sum.saturating_add(u.size.bytes)),
                "target_reached":!self.total.unknown && self.total.bytes.saturating_sub(self.reclaim)<BUDGET,
                "unknown_size_units":self.units.iter().filter(|u|u.size.unknown).count(),
                "unmanaged_estimated_bytes":self.overhead.bytes,"unmanaged_size_unknown":self.overhead.unknown,"skip_reasons":counts},
            "recovery_issues":self.recovery_issues,"units":self.units.iter().map(Unit::value).collect::<Vec<_>>()})
    }
}

fn plan(root: &Path, now: u64) -> Result<Plan, String> {
    let entries: Vec<_> = fs::read_dir(root)
        .map_err(|_| "cannot enumerate Team store")?
        .collect::<Result<_, _>>()
        .map_err(|_| "cannot enumerate Team store")?;
    let mut units = BTreeMap::<String, Unit>::new();
    let mut recovery_issues = Vec::new();
    let mut retired_members = BTreeMap::<String, String>::new();
    for entry in &entries {
        let name = entry.file_name();
        if !name
            .to_str()
            .is_some_and(|s| s.starts_with(PREFIX) && s.ends_with(".json"))
        {
            continue;
        }
        match journal(root, &entry.path()) {
            Ok(unit) => {
                let id = unit.id.clone();
                let members = unit.members.clone();
                if members.iter().any(|m| retired_members.contains_key(m)) {
                    return Err(
                        "overlapping retirement journals; restore metadata before cleanup".into(),
                    );
                }
                for member in &members {
                    retired_members.insert(member.clone(), id.clone());
                }
                units.insert(id, unit);
            }
            Err(error) => recovery_issues.push(json!({"error":error})),
        }
    }
    let mut overhead = Size::default();
    for entry in entries {
        let path = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let bytes = size(&path);
        if let Some(root_id) = retired_members.get(&id) {
            units.get_mut(root_id).unwrap().size.add(bytes);
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|_| "cannot inspect store entry")?;
        if storage::validate_run_id(&id).is_err() || (!kind.is_dir() && !kind.is_symlink()) {
            overhead.add(bytes);
            continue;
        }
        let manifest = real(&path, true).and_then(|()| read(&path.join("manifest.json")));
        let parent = manifest
            .as_ref()
            .ok()
            .and_then(|m| m["parent_run_id"].as_str());
        let root_id = parent
            .filter(|p| storage::validate_run_id(p).is_ok())
            .unwrap_or(&id)
            .to_owned();
        if units.get(&root_id).is_some_and(|u| u.retiring) {
            // A child not named by the committed journal is contradictory evidence.
            units.get_mut(&root_id).unwrap().skip = Some("retirement_relationships".into());
            overhead.add(bytes);
            continue;
        }
        let unit = units.entry(root_id.clone()).or_insert_with(|| Unit {
            id: root_id,
            members: Vec::new(),
            last: None,
            size: Size::default(),
            skip: None,
            reasons: Vec::new(),
            retiring: false,
            proofs: None,
        });
        unit.members.push(id);
        unit.size.add(bytes);
        if manifest.is_err() {
            unit.skip = Some("invalid_metadata".into());
        }
    }
    let mut total = overhead;
    let mut units: Vec<_> = units.into_values().collect();
    for unit in &mut units {
        unit.members.sort();
        total.add(unit.size);
        if unit.skip.is_none() {
            if unit.retiring {
                if let Err(error) = retirement_guard(root, unit) {
                    unit.skip = Some(error);
                }
            } else {
                match snapshot(root, unit, now) {
                    Ok(last) => unit.last = Some(last),
                    Err(error) => unit.skip = Some(error),
                }
            }
        }
        if unit.size.unknown && unit.skip.is_none() {
            unit.skip = Some("unknown_size".into());
        }
        if unit.skip.is_some() {
            unit.reasons.clear();
        }
    }
    let mut checked = BTreeSet::new();
    let reclaim = loop {
        for unit in &mut units {
            if !unit.retiring {
                unit.reasons.clear();
            }
        }
        let reclaim = select(&mut units, total.bytes, now, BUDGET);
        let mut rejected = false;
        for unit in units
            .iter_mut()
            .filter(|u| !u.reasons.is_empty() && !u.retiring)
        {
            if !checked.insert(unit.id.clone()) {
                continue;
            }
            let error = match inspect(root, unit, now) {
                Ok((last, _locks)) if Some(last) == unit.last => None,
                Ok(_) => Some("activity_changed_after_plan".into()),
                Err(error) => Some(error),
            };
            if let Some(error) = error {
                unit.skip = Some(error);
                unit.reasons.clear();
                rejected = true;
            }
        }
        if !rejected {
            break reclaim;
        }
    };
    Ok(Plan {
        units,
        total,
        overhead,
        reclaim,
        recovery_issues,
    })
}

fn sync_dir(root: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|_| "retirement_directory_sync_failed")?;
    #[cfg(windows)]
    let _ = root;
    Ok(())
}

fn retirement_guard(root: &Path, unit: &Unit) -> Result<Vec<FileLock>, String> {
    let evidence = unit.proofs.as_ref().ok_or("invalid_retirement_journal")?;
    let mut locks = Vec::new();
    for id in &unit.members {
        let run = root.join(id);
        if exists(&run)? {
            real(&run, true)?;
            if exists(&run.join("run.lock"))? {
                locks.push(lock(&run.join("run.lock"), false)?);
            } else if exists(&run.join("manifest.json"))? {
                return Err("retirement_owner_lock_missing".into());
            }
        }
    }
    let root_run = root.join(&unit.id);
    if exists(&root_run.join("tree.lock"))? {
        locks.push(lock(&root_run.join("tree.lock"), false)?);
    }
    for id in &unit.members {
        let run = root.join(id);
        let proof = &evidence[id];
        if !proof["runtime"].is_null() {
            inactive_runtime(&proof["runtime"], &proof["revision"])?;
        }
        if exists(&run.join("manifest.json"))? {
            let manifest = read(&run.join("manifest.json"))?;
            if manifest["run_id"] != id.as_str()
                || manifest["revision"] != proof["revision"]
                || (id == &unit.id && !manifest["parent_run_id"].is_null())
                || (id != &unit.id && manifest["parent_run_id"] != unit.id)
            {
                return Err("retirement_relationships_or_revision_changed".into());
            }
            inactive(&run, &manifest, None)?;
            if last_activity(&run, &manifest, now_ms()?)? > unit.last.unwrap() {
                return Err("activity_changed_after_retirement".into());
            }
        } else if exists(&run.join("runtime.json"))? {
            if runtime_proof(&read(&run.join("runtime.json"))?) != proof["runtime"] {
                return Err("retirement_runtime_changed".into());
            }
        }
    }
    Ok(locks)
}

fn remove_payload(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "retirement_remove_failed")?;
    if meta.is_dir() && !link(&meta) {
        fs::remove_dir_all(path).map_err(|_| "retirement_remove_failed".into())
    } else {
        #[cfg(windows)]
        if link(&meta) && meta.is_dir() {
            return fs::remove_dir(path).map_err(|_| "retirement_remove_failed".into());
        }
        fs::remove_file(path).map_err(|_| "retirement_remove_failed".into())
    }
}

fn finish_retirement_locked(root: &Path, unit: &Unit, locks: Vec<FileLock>) -> Result<(), String> {
    crate::bookmarks::retire(root, &unit.members, || {
        // Remove the coordinator manifest first, then every child manifest under
        // the bookmark lock. Subsequent alias assignments and legacy resumes can
        // no longer bind any member, even if the cleaner dies after this fence.
        for id in std::iter::once(unit.id.as_str()).chain(
            unit.members
                .iter()
                .map(String::as_str)
                .filter(|id| *id != unit.id),
        ) {
            let path = root.join(id).join("manifest.json");
            if exists(&path)? {
                fs::remove_file(&path).map_err(|_| "retirement_manifest_remove_failed")?;
            }
        }
        sync_dir(root)
    })?;
    // Follow the existing Team migration lease contract: Windows cannot rename
    // a directory containing an open leased descendant, even with SHARE_DELETE.
    // Retain both native lock paths while removing only payload children.
    for id in &unit.members {
        let run = root.join(id);
        if !exists(&run)? {
            continue;
        }
        for entry in fs::read_dir(&run).map_err(|_| "retirement_remove_failed")? {
            let entry = entry.map_err(|_| "retirement_remove_failed")?;
            if matches!(entry.file_name().to_str(), Some("run.lock" | "tree.lock")) {
                continue;
            }
            remove_payload(&entry.path())?;
        }
    }
    // Every original manifest is absent before ownership is released. Removal
    // here is nonrecursive so unexpected new payload is preserved, never erased.
    drop(locks);
    for id in &unit.members {
        let run = root.join(id);
        if !exists(&run)? {
            continue;
        }
        for anchor in ["run.lock", "tree.lock"] {
            let path = run.join(anchor);
            if exists(&path)? {
                real(&path, false)?;
                fs::remove_file(path).map_err(|_| "retirement_anchor_remove_failed")?;
            }
        }
        fs::remove_dir(&run).map_err(|_| "retirement_empty_directory_remove_failed")?;
    }
    fs::remove_file(marker(root, &unit.id)).map_err(|_| "retirement_journal_remove_failed")?;
    sync_dir(root)
}

fn finish_retirement(root: &Path, unit: &Unit) -> Result<(), String> {
    let locks = retirement_guard(root, unit)?;
    finish_retirement_locked(root, unit, locks)
}

fn journal(root: &Path, path: &Path) -> Result<Unit, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid_retirement_journal")?;
    let id = name
        .strip_prefix(PREFIX)
        .and_then(|s| s.strip_suffix(".json"))
        .ok_or("invalid_retirement_journal")?;
    storage::validate_run_id(id).map_err(|_| "invalid_retirement_journal")?;
    let record = read(path)?;
    if record["version"] != 2 || record["root_id"] != id {
        return Err("invalid_retirement_journal".into());
    }
    let mut members = Vec::new();
    for value in record["members"]
        .as_array()
        .ok_or("invalid_retirement_journal")?
    {
        let member = value.as_str().ok_or("invalid_retirement_journal")?;
        storage::validate_run_id(member).map_err(|_| "invalid_retirement_journal")?;
        if members.iter().any(|s| s == member) {
            return Err("invalid_retirement_journal".into());
        }
        members.push(member.to_owned());
    }
    if !members.iter().any(|s| s == id) || marker(root, id) != path {
        return Err("invalid_retirement_journal".into());
    }
    let last = record["last_activity_ms"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("invalid_retirement_journal")?;
    let mut reasons = Vec::new();
    for reason in record["reasons"]
        .as_array()
        .ok_or("invalid_retirement_journal")?
    {
        let reason = match reason.as_str() {
            Some("age") => "age",
            Some("storage") => "storage",
            _ => return Err("invalid_retirement_journal".into()),
        };
        if reasons.contains(&reason) {
            return Err("invalid_retirement_journal".into());
        }
        reasons.push(reason);
    }
    if reasons.is_empty() {
        return Err("invalid_retirement_journal".into());
    }
    let evidence = record["proofs"]
        .as_object()
        .ok_or("invalid_retirement_journal")?;
    if evidence.len() != members.len() {
        return Err("invalid_retirement_journal".into());
    }
    for member in &members {
        let proof = &record["proofs"][member];
        if !proof["revision"]
            .as_u64()
            .is_some_and(|n| (1..=999999).contains(&n))
            || (!subteam::terminal(&proof["status"])
                && (proof["status"] != "running" || proof["runtime"].is_null()))
        {
            return Err("invalid_retirement_journal".into());
        }
        if !proof["runtime"].is_null() {
            for role in ["launcher", "backend"] {
                subteam::recorded_identity(&proof["runtime"], role)
                    .map_err(|_| "invalid_retirement_journal")?;
            }
            if proof["runtime"]["revision"] != proof["revision"] {
                return Err("invalid_retirement_journal".into());
            }
        }
    }
    Ok(Unit {
        id: id.to_owned(),
        members,
        last: Some(last),
        size: Size::default(),
        skip: None,
        reasons,
        retiring: true,
        proofs: Some(record["proofs"].clone()),
    })
}

fn retire(root: &Path, unit: &Unit, now: u64) -> Result<(), String> {
    let (last, locks) = inspect(root, unit, now)?;
    if unit.last != Some(last) {
        return Err("activity_changed_after_plan".into());
    }
    let mut pending = unit.clone();
    pending.retiring = true;
    pending.proofs = Some(proofs(root, unit)?);
    lifecycle::save_json(
        &marker(root, &unit.id),
        &json!({"version":2,"root_id":unit.id,"members":unit.members,
        "last_activity_ms":last,"reasons":unit.reasons,"proofs":pending.proofs}),
    )
    .map_err(|_| "retirement_journal_failed")?;
    sync_dir(root)?;
    finish_retirement_locked(root, &pending, locks)
}

fn execute(root: &Path, dry: bool) -> Result<Value, String> {
    if exists(root)? {
        real(root, true)?;
    }
    let Some(root) = crate::profile::defaults::root_if_present(root)? else {
        return Ok(
            json!({"dry_run":dry,"summary":{"total_estimated_bytes":0,"selected_units":0,"selected_members":0,"reclaim_estimated_bytes":0},"units":[]}),
        );
    };
    real(&root, true)?;
    let _cleaner = if dry {
        None
    } else {
        match lock(&root.join(".cleanup.lock"), true) {
            Ok(lock) => Some(lock),
            Err(error) if error == "live_owner_or_busy_lock" => {
                return Ok(json!({"skipped":"cleaner_already_active"}));
            }
            Err(error) => return Err(error),
        }
    };
    crate::bookmarks::validate_cleanup(&root)?;
    let now = now_ms()?;
    let plan = plan(&root, now)?;
    let mut result = plan.value(now);
    result["dry_run"] = json!(dry);
    if !dry {
        let outcomes: Vec<_> = plan
            .units
            .iter()
            .filter(|u| !u.reasons.is_empty())
            .map(|unit| {
                let outcome = if unit.retiring {
                    finish_retirement(&root, unit)
                } else {
                    retire(&root, unit, now_ms()?)
                };
                Ok(json!({"root_id":unit.id,"deleted":outcome.is_ok(),"error":outcome.err()}))
            })
            .collect::<Result<_, String>>()?;
        result["execution_summary"] = json!({
            "deleted_units":outcomes.iter().filter(|o|o["deleted"]==true).count(),
            "failed_units":outcomes.iter().filter(|o|o["deleted"]!=true).count(),
            "completed_reclaim_estimated_bytes":plan.units.iter().filter(|u|outcomes.iter().any(|o|o["root_id"]==u.id && o["deleted"]==true)).fold(0_u64,|sum,u|sum.saturating_add(u.size.bytes))
        });
        result["outcomes"] = json!(outcomes);
    }
    if !dry {
        let mut report = result.clone();
        report["cleaner_pid"] = json!(std::process::id());
        report.as_object_mut().unwrap().remove("units");
        lifecycle::save_json(&root.join(".cleanup-report.json"), &report)
            .map_err(|_| "cleanup finished but report could not be saved")?;
    }
    Ok(result)
}

fn render(result: &Value) {
    if let Some(skip) = result["skipped"].as_str() {
        println!("cleanup skipped: {skip}");
        return;
    }
    println!(
        "Team cleanup {} (50 GB = {BUDGET} bytes; age > 10 × 24 h)",
        if result["dry_run"] == true {
            "dry-run"
        } else {
            "plan/outcomes"
        }
    );
    if let Some(units) = result["units"].as_array() {
        let mut shown = 0;
        for unit in units
            .iter()
            .filter(|u| u["reasons"].as_array().is_some_and(|a| !a.is_empty()))
        {
            if shown == 50 {
                break;
            }
            println!(
                "{} members={} last_activity_ms={} estimated_bytes={} reasons={}",
                unit["root_id"].as_str().unwrap(),
                unit["member_count"],
                unit["last_activity_ms"],
                unit["estimated_bytes"],
                unit["reasons"]
            );
            shown += 1;
        }
        if result["summary"]["selected_units"].as_u64().unwrap_or(0) > shown {
            println!("Showing first {shown} selected units; --json includes the full plan.");
        }
    }
    println!("summary: {}", result["summary"]);
    if !result["execution_summary"].is_null() {
        println!("execution: {}", result["execution_summary"]);
    }
    for key in ["recovery_issues", "outcomes"] {
        if let Some(rows) = result[key].as_array() {
            let failures: Vec<_> = rows
                .iter()
                .filter(|r| !r["error"].is_null())
                .take(10)
                .collect();
            if !failures.is_empty() {
                println!("{key} errors (first 10): {}", json!(failures));
            }
        }
    }
}

pub(super) fn command(args: &[String]) -> Result<(), String> {
    let (mut root, mut dry, mut machine) = (None, false, false);
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--dry-run" if !dry => dry = true,
            "--json" if !machine => machine = true,
            "--store" if root.is_none() => {
                root = Some(PathBuf::from(
                    rest.next().ok_or("missing --store directory")?,
                ))
            }
            _ => return Err(format!("unknown/duplicate cleanup option {arg}")),
        }
    }
    let result = execute(&root.map(Ok).unwrap_or_else(storage::default_root)?, dry)?;
    if machine {
        println!("{result}");
    } else {
        render(&result);
    }
    Ok(())
}

/// Spawn only: no metadata reads, locks, store enumeration, or cleaner wait here.
pub(super) fn trigger(root: &Path) {
    let outcome = (|| {
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command
            .arg("__cleanup")
            .arg("--store")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detached(&mut command)
    })();
    if let Err(error) = outcome {
        crate::notice(format_args!(
            "pira_team: background retention not started: {error}"
        ));
    }
}

fn detached(command: &mut Command) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Match the native child-launcher breakaway contract. No inherited console
        // or backend job owns this finite-lived cleaner after its origin exits.
        command.creation_flags(
            crate::native::windows::CREATE_BREAKAWAY_FROM_JOB
                | crate::native::windows::CREATE_NEW_PROCESS_GROUP
                | 0x00000008,
        ); // DETACHED_PROCESS
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot spawn detached cleaner: {error}"))?;
    // Reap while this launcher exists; dropping the JoinHandle never joins it.
    if let Err(error) = std::thread::Builder::new()
        .name("team-cleaner-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })
    {
        // The finite-lived child is already running. Parent exit lets the OS
        // reap it if a local waiting thread cannot be created.
        crate::notice(format_args!(
            "pira_team: cleaner started; local reaper unavailable: {error}"
        ));
    }
    Ok(())
}

pub(super) fn background(args: &[String]) -> i32 {
    if args.len() != 3 || args[1] != "--store" {
        return 1;
    }
    let root = Path::new(&args[2]);
    match execute(root, false) {
        Ok(_) => 0,
        Err(_) => 1,
    }
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;
