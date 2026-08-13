use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::render::TerminalView;
use crate::storage;

const MAX_STATE_BYTES: u64 = 512 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Probe,
    Capture,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MonitorStatus {
    Active,
    Paused,
    Stopped,
    Complete,
    Deadline,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Unknown,
    Pending,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Idle,
    Probing,
    Analyzing,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionPolicy {
    Return,
    Cache,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnalyzerSpec {
    pub revision: u64,
    #[serde(default)]
    pub code_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchState {
    pub schema: u32,
    pub id: String,
    pub workspace_hash: String,
    pub created_ms: u128,
    pub updated_ms: u128,
    pub deadline_ms: u128,
    pub source_kind: SourceKind,
    pub source: Vec<String>,
    #[serde(default)]
    pub source_cwd: String,
    #[serde(default)]
    pub capture_path: Option<PathBuf>,
    pub intent: Option<String>,
    pub sample_every_ms: u64,
    pub attempt_timeout_ms: u64,
    pub pending_exit: i32,
    pub attention_policy: AttentionPolicy,
    #[serde(default)]
    pub configuration_revision: u64,
    pub inactive_after_ms: Option<u64>,
    pub unchanged_after_ms: Option<u64>,
    pub no_progress_after_ms: Option<u64>,
    pub analyzer: Option<AnalyzerSpec>,
    pub monitor: MonitorStatus,
    pub job: JobStatus,
    pub attempt: AttemptStatus,
    pub attempts: u64,
    pub sample_ms: Option<u128>,
    pub next_sample_ms: u128,
    pub stdout_offset: u64,
    pub stderr_offset: u64,
    pub raw_stdout: Vec<u8>,
    pub raw_stderr: Vec<u8>,
    pub visible_stdout: String,
    pub visible_stderr: String,
    pub stdout_view: TerminalView,
    pub stderr_view: TerminalView,
    pub raw_hash: String,
    pub visible_hash: String,
    pub progress_hash: String,
    pub progress: String,
    #[serde(default)]
    pub analyzer_summary: String,
    pub analyzer_error: Option<String>,
    pub last_activity_ms: Option<u128>,
    pub last_visible_change_ms: Option<u128>,
    pub last_progress_ms: Option<u128>,
    pub attention_reason: Option<String>,
    pub attention_sequence: u64,
    pub detail: String,
    pub rendered_reliable: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ControlState {
    pub stop_requested: bool,
    pub analyzer_revision: u64,
    pub analyzer: Option<AnalyzerSpec>,
    pub clear_analyzer: bool,
    #[serde(default)]
    pub configuration_revision: u64,
    #[serde(default)]
    pub configuration: Option<WatchConfiguration>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfiguration {
    pub revision: u64,
    pub sample_every_ms: u64,
    pub inactive_after_ms: Option<u64>,
    pub unchanged_after_ms: Option<u64>,
    pub no_progress_after_ms: Option<u64>,
    pub attention_policy: AttentionPolicy,
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn root(store: &Path) -> PathBuf {
    store.join("watch")
}
pub fn state_dir(store: &Path) -> PathBuf {
    root(store).join("state")
}
pub fn control_dir(store: &Path) -> PathBuf {
    root(store).join("control")
}
pub fn state_path(store: &Path, id: &str) -> PathBuf {
    state_dir(store).join(format!("{id}.json"))
}
pub fn control_path(store: &Path, id: &str) -> PathBuf {
    control_dir(store).join(format!("{id}.json"))
}
fn control_lock_path(store: &Path, id: &str) -> PathBuf {
    control_dir(store).join(format!("{id}.control-lock"))
}
pub fn owner_path(store: &Path, id: &str) -> PathBuf {
    root(store).join("owners").join(format!("{id}.lock"))
}

pub fn owner_is_alive(store: &Path, id: &str) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(owner_path(store, id))
    else {
        return false;
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(_) => true,
    }
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if id.starts_with("watch-") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        Ok(())
    } else {
        Err("invalid watch ID".into())
    }
}

pub fn read<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T, String> {
    let bytes = crate::util::read_file_limited(path, MAX_STATE_BYTES, label)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid {label}: {e}"))
}

pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("watch state exceeds 512 KiB".into());
    }
    let parent = path.parent().ok_or("watch path has no parent")?;
    storage::ensure_private_dir(parent)?;
    let temporary = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    if let Err(e) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("write watch state: {e}"));
    }
    drop(file);
    if let Err(e) = storage::atomic_replace(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("publish watch state: {e}"));
    }
    Ok(())
}

pub struct OwnerLock {
    _file: File,
}
pub fn acquire_owner(store: &Path, id: &str) -> Result<OwnerLock, String> {
    validate_id(id)?;
    let path = owner_path(store, id);
    storage::ensure_private_dir(path.parent().unwrap())?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|error| error.to_string())?;
    file.try_lock_exclusive()
        .map_err(|_| format!("watch {id} already has an active owner"))?;
    file.set_len(0).map_err(|e| e.to_string())?;
    writeln!(file, "{}", std::process::id()).map_err(|e| e.to_string())?;
    Ok(OwnerLock { _file: file })
}

pub fn update_control(
    store: &Path,
    id: &str,
    update: impl FnOnce(&mut ControlState),
) -> Result<(), String> {
    validate_id(id)?;
    let lock_path = control_lock_path(store, id);
    storage::ensure_private_dir(lock_path.parent().expect("control lock has parent"))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(&lock_path)
        .map_err(|error| format!("open watch control lock: {error}"))?;
    lock.lock_exclusive()
        .map_err(|error| format!("lock watch control: {error}"))?;
    let path = control_path(store, id);
    let mut control = read(&path, "watch control")?;
    update(&mut control);
    let result = write(&path, &control);
    let _ = lock.unlock();
    result
}

pub fn resolve(store: &Path, target: &str) -> Result<String, String> {
    if validate_id(target).is_ok() {
        let exact = state_path(store, target);
        if exact.is_file() {
            return Ok(target.into());
        }
    } else if !target.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid watch ID or prefix".into());
    }
    let mut found = Vec::new();
    if let Ok(items) = fs::read_dir(state_dir(store)) {
        for item in items.flatten() {
            if let Some(name) = item
                .file_name()
                .to_str()
                .and_then(|v| v.strip_suffix(".json"))
                && name.starts_with(target)
            {
                found.push(name.to_string());
            }
        }
    }
    match found.as_slice() {
        [id] => Ok(id.clone()),
        [] => Err(format!("no watch matches {target}")),
        _ => Err(format!("ambiguous watch ID {target}")),
    }
}
