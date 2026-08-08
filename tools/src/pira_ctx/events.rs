use std::collections::{BinaryHeap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::{model::Metadata, storage, util};

const EVENT_MAGIC: &[u8; 8] = b"PIRAEVT1";
const CATALOG_MAGIC: &[u8; 8] = b"PIRAEIX1";
const RETENTION_MAGIC: &[u8; 8] = b"PIRARTN1";
const MAX_EVENT_BYTES: u64 = 64 * 1024;
const MAX_CATALOG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RETENTION_BYTES: u64 = 8 * 1024 * 1024;
const RETENTION_COMPACT_BYTES: u64 = 6 * 1024 * 1024;
const MAX_EVENTS_PER_SCOPE: usize = 2_000;
const MAX_EVENTS_PER_WORKSPACE: usize = 8_000;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeChoice {
    Current,
    Workspace,
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub hash: String,
    pub detected: bool,
}

impl Scope {
    pub fn label(&self) -> &'static str {
        if self.detected {
            "current-thread"
        } else {
            "current-workspace-fallback"
        }
    }

    fn directory_name(&self) -> &str {
        if self.detected {
            &self.hash
        } else {
            ".unscoped"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub timestamp_ms: u64,
    pub workspace_hash: String,
    pub scope_hash: String,
    pub intent: String,
    pub command: String,
    pub category: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub observed: String,
    pub capture_id: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSummary {
    pub timestamp_ms: u64,
    pub scope_hash: String,
    pub intent: String,
    pub category: String,
    pub exit_code: i32,
    pub capture_id: Option<String>,
    record_size: u64,
    record_modified_ns: u64,
    record_name: String,
    record_path: PathBuf,
    details: Option<EventDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EventDetails {
    duration_ms: u64,
    command: String,
}

impl EventSummary {
    pub fn scope_label(&self) -> String {
        if self.scope_hash.is_empty() {
            "unscoped".into()
        } else {
            self.scope_hash.chars().take(12).collect()
        }
    }

    pub fn loaded_details(&self) -> Option<(u64, &str)> {
        self.details
            .as_ref()
            .map(|details| (details.duration_ms, details.command.as_str()))
    }
}

#[derive(Debug, Clone)]
pub struct ReadResult {
    pub events: Vec<EventSummary>,
    pub skipped: usize,
    pub scope_label: String,
}

#[derive(Debug, Clone, Copy)]
pub struct HistoryBounds {
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
    pub offset: usize,
    pub lookback: Option<usize>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct HistorySearchResult {
    pub events: Vec<EventSummary>,
    pub scanned: usize,
    pub skipped: usize,
    pub scope_label: String,
    pub stopped_by_limit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetentionRecord {
    timestamp_ms: u64,
    record_name: String,
    summary: Option<RetentionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetentionSummary {
    intent: String,
    category: String,
    exit_code: i32,
    capture_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RetentionState {
    scopes: HashMap<String, Vec<RetentionRecord>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetentionRemoval {
    scope_name: String,
    record: RetentionRecord,
}

struct RetentionLoad {
    state: RetentionState,
    appendable: bool,
    bytes: u64,
}

pub fn current_scope(workspace_hash: &str) -> Scope {
    scope_from_environment(workspace_hash, |name| std::env::var(name).ok())
}

fn scope_from_environment<F>(workspace_hash: &str, mut read: F) -> Scope
where
    F: FnMut(&str) -> Option<String>,
{
    for (provider, name) in [
        ("pira", "PIRA_CTX_THREAD_ID"),
        ("codex", "CODEX_THREAD_ID"),
        ("claude", "CLAUDE_CODE_SESSION_ID"),
    ] {
        let Some(value) = read(name).filter(|value| {
            !value.is_empty() && value.len() <= 4_096 && !value.chars().any(char::is_control)
        }) else {
            continue;
        };
        return Scope {
            hash: scope_hash(workspace_hash, provider, &value),
            detected: true,
        };
    }
    Scope {
        hash: String::new(),
        detected: false,
    }
}

fn scope_hash(workspace_hash: &str, provider: &str, raw_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pira_ctx-thread-v1\0");
    hasher.update(provider.as_bytes());
    hasher.update(b"\0");
    hasher.update(workspace_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(raw_id.as_bytes());
    util::hex(&hasher.finalize())
}

pub fn record(
    store: &Path,
    intent: &str,
    command: &[String],
    exit: i32,
    duration: u128,
    metadata: Option<&Metadata>,
) -> Result<(), String> {
    let workspace_hash = storage::current_workspace_hash()?;
    let scope = current_scope(&workspace_hash);
    let mut event = Event {
        timestamp_ms: util::millis(SystemTime::now()).min(u128::from(u64::MAX)) as u64,
        workspace_hash: workspace_hash.clone(),
        scope_hash: scope.hash.clone(),
        intent: intent.trim().to_string(),
        command: util::single_line_clip(&util::redacted_argv_display(command), 2_048),
        category: category(command),
        exit_code: exit,
        duration_ms: duration.min(u128::from(u64::MAX)) as u64,
        observed: util::single_line_clip(&observed(exit, metadata), 1_024),
        capture_id: metadata.map(|value| util::single_line_clip(&value.result_id, 128)),
        files: metadata.map_or_else(Vec::new, |value| {
            value
                .detected_paths
                .iter()
                .take(16)
                .map(|path| util::single_line_clip(path, 512))
                .collect()
        }),
    };
    let workspace_dir = workspace_dir(store, &workspace_hash);
    let scope_dir = workspace_dir.join(scope.directory_name());
    let records_dir = scope_dir.join("records");
    storage::ensure_private_dir(store)?;
    storage::ensure_private_dir(&store.join(".events"))?;
    storage::ensure_private_dir(&workspace_dir)?;
    storage::ensure_private_dir(&scope_dir)?;
    storage::ensure_private_dir(&records_dir)?;
    let _workspace_lock = lock_workspace(&workspace_dir)?;
    let state_path = retention_state_path(&workspace_dir);
    let loaded = load_or_rebuild_retention_state(&workspace_dir, &workspace_hash);
    let mut retention = loaded.state;
    event.timestamp_ms = next_retention_timestamp(&retention, event.timestamp_ms);
    let name = format!(
        "{:020}-{:010}-{:016x}.piraevt",
        event.timestamp_ms,
        std::process::id(),
        short_nonce()
    );
    let planned = RetentionRecord {
        timestamp_ms: event.timestamp_ms,
        record_name: name.clone(),
        summary: Some(RetentionSummary {
            intent: event.intent.clone(),
            category: event.category.clone(),
            exit_code: event.exit_code,
            capture_id: event.capture_id.clone(),
        }),
    };
    let removals = plan_retention(&mut retention, scope.directory_name(), planned.clone());
    for removal in &removals {
        remove_retention_record(&workspace_dir, removal)?;
    }
    let retention_result = if loaded.appendable && loaded.bytes < RETENTION_COMPACT_BYTES {
        append_retention_delta(
            &state_path,
            Some((scope.directory_name(), &planned)),
            &removals,
        )
    } else {
        write_retention_snapshot(&state_path, &workspace_hash, &retention)
    };
    if let Err(error) = retention_result {
        let _ = fs::remove_file(&state_path);
        return Err(error);
    }
    if let Err(error) = atomic_write(&records_dir.join(&name), &encode_event(&event)?) {
        remove_retention_entry(&mut retention, scope.directory_name(), &name);
        let rollback = RetentionRemoval {
            scope_name: scope.directory_name().to_string(),
            record: planned,
        };
        if append_retention_delta(&state_path, None, &[rollback]).is_err() {
            let _ = fs::remove_file(&state_path);
        }
        return Err(error);
    }
    for removal in removals {
        let _ = cleanup_empty_scope(&workspace_dir, &removal.scope_name);
    }
    Ok(())
}

pub fn read_current(store: &Path, limit: usize) -> Result<ReadResult, String> {
    let workspace_hash = storage::current_workspace_hash()?;
    let scope = current_scope(&workspace_hash);
    let event_root = store.join(".events");
    if !real_directory(&event_root, "event root")? {
        return Ok(empty_read(scope.label()));
    }
    let workspace_dir = workspace_dir(store, &workspace_hash);
    if !real_directory(&workspace_dir, "event workspace")? {
        return Ok(empty_read(scope.label()));
    }
    let scope_dir = workspace_dir.join(scope.directory_name());
    if !real_directory(&scope_dir, "event scope")? {
        return Ok(empty_read(scope.label()));
    }
    let mut result = read_scope(&scope_dir, &workspace_hash, &scope.hash, limit)?;
    result.scope_label = scope.label().into();
    Ok(result)
}

pub fn search_history<F>(
    store: &Path,
    choice: ScopeChoice,
    bounds: HistoryBounds,
    mut matches: F,
) -> Result<HistorySearchResult, String>
where
    F: FnMut(&EventSummary) -> bool,
{
    let workspace_hash = storage::current_workspace_hash()?;
    let scope = current_scope(&workspace_hash);
    let event_root = store.join(".events");
    let scope_label = match choice {
        ScopeChoice::Current => scope.label(),
        ScopeChoice::Workspace => "current-workspace",
    };
    if !real_directory(&event_root, "event root")? {
        return Ok(empty_history_search(scope_label));
    }
    let workspace_dir = workspace_dir(store, &workspace_hash);
    if !real_directory(&workspace_dir, "event workspace")? {
        return Ok(empty_history_search(scope_label));
    }
    let _workspace_lock = lock_workspace(&workspace_dir)?;
    let state_path = retention_state_path(&workspace_dir);
    let state = match read_retention_state(&state_path, &workspace_hash) {
        Ok(state) => state,
        Err(_) => {
            let state = rebuild_retention_state(&workspace_dir, &workspace_hash);
            write_retention_snapshot(&state_path, &workspace_hash, &state)?;
            state
        }
    };

    let mut offset = bounds.offset;
    let mut window_seen = 0_usize;
    let mut scanned = 0_usize;
    let mut skipped = 0_usize;
    let mut events = Vec::with_capacity(bounds.limit);
    let mut stopped_by_limit = false;
    let mut consider = |scope_name: &str, record: &RetentionRecord| {
        if bounds
            .until_ms
            .is_some_and(|until| record.timestamp_ms >= until)
        {
            return false;
        }
        if bounds
            .since_ms
            .is_some_and(|since| record.timestamp_ms < since)
        {
            return true;
        }
        if offset > 0 {
            offset -= 1;
            return false;
        }
        if bounds
            .lookback
            .is_some_and(|maximum| window_seen >= maximum)
        {
            return true;
        }
        window_seen += 1;
        scanned += 1;
        let Some(indexed_summary) = &record.summary else {
            skipped += 1;
            return false;
        };
        let scope_hash = if scope_name == ".unscoped" {
            String::new()
        } else {
            scope_name.to_string()
        };
        let indexed = EventSummary {
            timestamp_ms: record.timestamp_ms,
            scope_hash,
            intent: indexed_summary.intent.clone(),
            category: indexed_summary.category.clone(),
            exit_code: indexed_summary.exit_code,
            capture_id: indexed_summary.capture_id.clone(),
            record_size: 0,
            record_modified_ns: 0,
            record_name: record.record_name.clone(),
            record_path: workspace_dir
                .join(scope_name)
                .join("records")
                .join(&record.record_name),
            details: None,
        };
        if !matches(&indexed) {
            return false;
        }
        let authoritative = record_fingerprint(&indexed.record_path).and_then(
            |(record_size, record_modified_ns)| {
                read_event_path(&indexed.record_path).and_then(|event| {
                    if event.workspace_hash != workspace_hash
                        || event.scope_hash != indexed.scope_hash
                        || event.timestamp_ms != indexed.timestamp_ms
                        || event.intent != indexed.intent
                        || event.category != indexed.category
                        || event.exit_code != indexed.exit_code
                        || event.capture_id != indexed.capture_id
                    {
                        return Err("event history index mismatch".into());
                    }
                    Ok(summary(
                        &event,
                        indexed.record_name.clone(),
                        indexed.record_path.clone(),
                        record_size,
                        record_modified_ns,
                    ))
                })
            },
        );
        let Ok(authoritative) = authoritative else {
            skipped += 1;
            return false;
        };
        events.push(authoritative);
        if events.len() == bounds.limit {
            stopped_by_limit = true;
            return true;
        }
        false
    };

    match choice {
        ScopeChoice::Current => {
            if let Some(records) = state.scopes.get(scope.directory_name()) {
                for record in records.iter().rev() {
                    if consider(scope.directory_name(), record) {
                        break;
                    }
                }
            }
        }
        ScopeChoice::Workspace => {
            let scopes = state.scopes.iter().collect::<Vec<_>>();
            let mut newest = BinaryHeap::new();
            for (scope_index, (_, records)) in scopes.iter().enumerate() {
                if let Some(record_index) = records.len().checked_sub(1) {
                    let record = &records[record_index];
                    newest.push((
                        record.timestamp_ms,
                        record.record_name.clone(),
                        scope_index,
                        record_index,
                    ));
                }
            }
            while let Some((_, _, scope_index, record_index)) = newest.pop() {
                let (scope_name, records) = scopes[scope_index];
                let record = &records[record_index];
                if consider(scope_name, record) {
                    break;
                }
                if let Some(previous) = record_index.checked_sub(1) {
                    let record = &records[previous];
                    newest.push((
                        record.timestamp_ms,
                        record.record_name.clone(),
                        scope_index,
                        previous,
                    ));
                }
            }
        }
    }

    Ok(HistorySearchResult {
        events,
        scanned,
        skipped,
        scope_label: scope_label.into(),
        stopped_by_limit,
    })
}

fn empty_history_search(scope_label: &str) -> HistorySearchResult {
    HistorySearchResult {
        events: Vec::new(),
        scanned: 0,
        skipped: 0,
        scope_label: scope_label.into(),
        stopped_by_limit: false,
    }
}

fn read_scope(
    scope_dir: &Path,
    workspace_hash: &str,
    scope_hash: &str,
    limit: usize,
) -> Result<ReadResult, String> {
    let records_dir = scope_dir.join("records");
    if !real_directory(&records_dir, "event records")? {
        return Ok(empty_read(""));
    }
    let mut paths = record_paths(&records_dir)?;
    paths.sort();
    let cached = read_catalog(
        &scope_dir.join(".catalog.piraidx"),
        workspace_hash,
        scope_hash,
    )
    .unwrap_or_default();
    let mut by_name: HashMap<String, EventSummary> = cached
        .into_iter()
        .map(|event| (event.record_name.clone(), event))
        .collect();
    let mut events = Vec::with_capacity(paths.len());
    let mut skipped = 0;
    let mut changed = by_name.len() != paths.len();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "invalid event filename".to_string())?
            .to_string();
        let (record_size, record_modified_ns) = match record_fingerprint(&path) {
            Ok(value) => value,
            Err(_) => {
                changed = true;
                skipped += 1;
                continue;
            }
        };
        if let Some(mut cached) = by_name.remove(&name)
            && cached.record_size == record_size
            && cached.record_modified_ns == record_modified_ns
        {
            cached.record_path = path;
            events.push(cached);
            continue;
        }
        changed = true;
        match read_event_path(&path) {
            Ok(event)
                if event.workspace_hash == workspace_hash && event.scope_hash == scope_hash =>
            {
                events.push(summary(&event, name, path, record_size, record_modified_ns));
            }
            _ => skipped += 1,
        }
    }
    if !by_name.is_empty() {
        changed = true;
    }
    events.sort_by(|left, right| event_key(left).cmp(&event_key(right)));
    if changed {
        let _ = write_catalog(
            &scope_dir.join(".catalog.piraidx"),
            workspace_hash,
            scope_hash,
            &events,
        );
    }
    if events.len() > limit {
        events = events.split_off(events.len() - limit);
    }
    Ok(ReadResult {
        events,
        skipped,
        scope_label: String::new(),
    })
}

pub fn read_details(summary: &EventSummary) -> Result<Event, String> {
    read_event_path(&summary.record_path)
}

pub fn select_recap(events: &[EventSummary], maximum: usize) -> Vec<EventSummary> {
    events[events.len().saturating_sub(maximum)..].to_vec()
}

pub fn forget(store: &Path, choice: ScopeChoice) -> Result<usize, String> {
    let workspace_hash = storage::current_workspace_hash()?;
    let event_root = store.join(".events");
    if !real_directory(&event_root, "event root")? {
        return Ok(0);
    }
    let workspace_dir = workspace_dir(store, &workspace_hash);
    if !real_directory(&workspace_dir, "event workspace")? {
        return Ok(0);
    }
    let target = match choice {
        ScopeChoice::Current => {
            let scope = current_scope(&workspace_hash);
            workspace_dir.join(scope.directory_name())
        }
        ScopeChoice::Workspace => workspace_dir.clone(),
    };
    if !real_directory(&target, "event deletion target")? {
        return Ok(0);
    }
    let _workspace_lock = lock_workspace(&workspace_dir)?;
    let count = count_records(&target)?;
    fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
    if choice == ScopeChoice::Current {
        let _ = fs::remove_file(retention_state_path(&workspace_dir));
    }
    Ok(count)
}

pub fn prune(store: &Path, max_age_days: Option<u64>) -> Result<usize, String> {
    let root = store.join(".events");
    if !real_directory(&root, "event root")? {
        return Ok(0);
    }
    let cutoff = max_age_days.map(|days| {
        util::millis(SystemTime::now())
            .saturating_sub(u128::from(days) * 86_400_000)
            .min(u128::from(u64::MAX)) as u64
    });
    let mut removed = 0;
    for entry in fs::read_dir(&root)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let workspace_dir = entry.path();
        let _workspace_lock = lock_workspace(&workspace_dir)?;
        for path in recursive_record_paths(&workspace_dir)? {
            if cutoff
                .is_some_and(|value| timestamp_from_path(&path).is_some_and(|time| time < value))
            {
                fs::remove_file(&path).map_err(|error| error.to_string())?;
                removed += 1;
                if let Some(scope_dir) = path.parent().and_then(Path::parent) {
                    let _ = fs::remove_file(scope_dir.join(".catalog.piraidx"));
                }
            }
        }
        let _ = fs::remove_file(retention_state_path(&workspace_dir));
        cleanup_empty_scopes(&workspace_dir)?;
    }
    Ok(removed)
}

pub fn legacy_count(store: &Path) -> usize {
    let root = store.join("events");
    match real_directory(&root, "legacy event root") {
        Ok(true) => recursive_paths_with_extension(&root, "json").map_or(0, |paths| paths.len()),
        _ => 0,
    }
}

pub fn prune_legacy(store: &Path) -> Result<usize, String> {
    let root = store.join("events");
    if !real_directory(&root, "legacy event root")? {
        return Ok(0);
    }
    let count = legacy_count(store);
    fs::remove_dir_all(root).map_err(|error| error.to_string())?;
    Ok(count)
}

fn summary(
    event: &Event,
    record_name: String,
    record_path: PathBuf,
    record_size: u64,
    record_modified_ns: u64,
) -> EventSummary {
    EventSummary {
        timestamp_ms: event.timestamp_ms,
        scope_hash: event.scope_hash.clone(),
        intent: event.intent.clone(),
        category: event.category.clone(),
        exit_code: event.exit_code,
        capture_id: event.capture_id.clone(),
        record_size,
        record_modified_ns,
        record_name,
        record_path,
        details: Some(EventDetails {
            duration_ms: event.duration_ms,
            command: event.command.clone(),
        }),
    }
}

fn event_key(event: &EventSummary) -> (u64, &str) {
    (event.timestamp_ms, &event.record_name)
}

fn workspace_dir(store: &Path, workspace_hash: &str) -> PathBuf {
    store.join(".events").join(workspace_hash)
}

fn record_paths(records_dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !real_directory(records_dir, "event records")? {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(records_dir)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("piraevt"))
        .collect())
}

fn recursive_record_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    recursive_paths_with_extension(root, "piraevt")
}

fn recursive_paths_with_extension(root: &Path, extension: &str) -> Result<Vec<PathBuf>, String> {
    if !real_directory(root, "event traversal root")? {
        return Ok(Vec::new());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut paths = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
        {
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some(extension)
            {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn count_records(root: &Path) -> Result<usize, String> {
    Ok(recursive_record_paths(root)?.len())
}

fn empty_read(scope_label: &str) -> ReadResult {
    ReadResult {
        events: Vec::new(),
        skipped: 0,
        scope_label: scope_label.into(),
    }
}

fn real_directory(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("refusing symlinked {label}: {}", path.display()))
        }
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(format!("{label} is not a directory: {}", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn record_fingerprint(path: &Path) -> Result<(u64, u64), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "event record is not a regular file: {}",
            path.display()
        ));
    }
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos().min(u128::from(u64::MAX)) as u64);
    Ok((metadata.len(), modified_ns))
}

fn lock_workspace(workspace_dir: &Path) -> Result<File, String> {
    let path = workspace_dir.with_extension("lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path).map_err(|error| error.to_string())?;
    file.lock().map_err(|error| error.to_string())?;
    Ok(file)
}

fn retention_state_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(".retention.piraidx")
}

fn valid_scope_name(value: &str) -> bool {
    value == ".unscoped"
        || (value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

fn valid_record_name(value: &str) -> bool {
    let path = Path::new(value);
    path.components().count() == 1
        && path.extension().and_then(|extension| extension.to_str()) == Some("piraevt")
        && timestamp_from_path(path).is_some()
}

fn load_or_rebuild_retention_state(workspace_dir: &Path, workspace_hash: &str) -> RetentionLoad {
    let path = retention_state_path(workspace_dir);
    match read_retention_state(&path, workspace_hash) {
        Ok(state) => RetentionLoad {
            state,
            appendable: true,
            bytes: fs::metadata(path).map_or(RETENTION_COMPACT_BYTES, |value| value.len()),
        },
        Err(_) => RetentionLoad {
            state: rebuild_retention_state(workspace_dir, workspace_hash),
            appendable: false,
            bytes: 0,
        },
    }
}

fn rebuild_retention_state(workspace_dir: &Path, workspace_hash: &str) -> RetentionState {
    let mut state = RetentionState::default();
    let Ok(entries) = fs::read_dir(workspace_dir) else {
        return state;
    };
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let scope_dir = entry.path();
        let Some(scope_name) = scope_dir
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| valid_scope_name(value))
        else {
            continue;
        };
        let Ok(paths) = record_paths(&scope_dir.join("records")) else {
            continue;
        };
        let records = state.scopes.entry(scope_name.to_string()).or_default();
        for path in paths {
            let Some(record_name) = path
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| valid_record_name(value))
            else {
                continue;
            };
            let expected_scope = if scope_name == ".unscoped" {
                ""
            } else {
                scope_name
            };
            let summary = read_event_path(&path).ok().and_then(|event| {
                (event.workspace_hash == workspace_hash && event.scope_hash == expected_scope)
                    .then_some(RetentionSummary {
                        intent: event.intent,
                        category: event.category,
                        exit_code: event.exit_code,
                        capture_id: event.capture_id,
                    })
            });
            records.push(RetentionRecord {
                timestamp_ms: timestamp_from_path(&path).unwrap_or_default(),
                record_name: record_name.to_string(),
                summary,
            });
        }
        records.sort_by(retention_record_key);
    }
    state.scopes.retain(|_, records| !records.is_empty());
    state
}

fn retention_record_key(left: &RetentionRecord, right: &RetentionRecord) -> std::cmp::Ordering {
    left.timestamp_ms
        .cmp(&right.timestamp_ms)
        .then_with(|| left.record_name.cmp(&right.record_name))
}

fn insert_retention_record(records: &mut Vec<RetentionRecord>, record: RetentionRecord) {
    let position = records.partition_point(|candidate| {
        retention_record_key(candidate, &record) != std::cmp::Ordering::Greater
    });
    records.insert(position, record);
}

fn plan_retention(
    state: &mut RetentionState,
    scope_name: &str,
    record: RetentionRecord,
) -> Vec<RetentionRemoval> {
    let scope = state.scopes.entry(scope_name.to_string()).or_default();
    insert_retention_record(scope, record);
    let mut removals = Vec::new();
    if scope.len() > MAX_EVENTS_PER_SCOPE {
        for record in scope.drain(..scope.len() - MAX_EVENTS_PER_SCOPE) {
            removals.push(RetentionRemoval {
                scope_name: scope_name.to_string(),
                record,
            });
        }
    }
    while retention_record_count(state) > MAX_EVENTS_PER_WORKSPACE {
        let Some(oldest_scope) = state
            .scopes
            .iter()
            .filter_map(|(name, records)| records.first().map(|record| (name, record)))
            .min_by(|left, right| {
                retention_record_key(left.1, right.1).then_with(|| left.0.cmp(right.0))
            })
            .map(|(name, _)| name.clone())
        else {
            break;
        };
        let record = state
            .scopes
            .get_mut(&oldest_scope)
            .expect("selected retention scope exists")
            .remove(0);
        removals.push(RetentionRemoval {
            scope_name: oldest_scope,
            record,
        });
    }
    state.scopes.retain(|_, records| !records.is_empty());
    removals
}

fn retention_record_count(state: &RetentionState) -> usize {
    state.scopes.values().map(Vec::len).sum()
}

fn next_retention_timestamp(state: &RetentionState, observed: u64) -> u64 {
    state
        .scopes
        .values()
        .filter_map(|records| records.last())
        .map(|record| record.timestamp_ms)
        .max()
        .map_or(observed, |latest| observed.max(latest.saturating_add(1)))
}

fn remove_retention_entry(state: &mut RetentionState, scope_name: &str, record_name: &str) {
    if let Some(records) = state.scopes.get_mut(scope_name) {
        records.retain(|record| record.record_name != record_name);
        if records.is_empty() {
            state.scopes.remove(scope_name);
        }
    }
}

fn remove_retention_record(workspace_dir: &Path, removal: &RetentionRemoval) -> Result<(), String> {
    if !valid_scope_name(&removal.scope_name) || !valid_record_name(&removal.record.record_name) {
        return Err("invalid event retention entry".into());
    }
    let scope_dir = workspace_dir.join(&removal.scope_name);
    let path = scope_dir.join("records").join(&removal.record.record_name);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let _ = fs::remove_file(scope_dir.join(".catalog.piraidx"));
    Ok(())
}

fn cleanup_empty_scopes(workspace_dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(workspace_dir)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if valid_scope_name(name) {
            cleanup_empty_scope(workspace_dir, name)?;
        }
    }
    Ok(())
}

fn cleanup_empty_scope(workspace_dir: &Path, scope_name: &str) -> Result<(), String> {
    if !valid_scope_name(scope_name) {
        return Err("invalid event scope name".into());
    }
    let scope_dir = workspace_dir.join(scope_name);
    if !real_directory(&scope_dir, "event scope")? {
        return Ok(());
    }
    let records_dir = scope_dir.join("records");
    if real_directory(&records_dir, "event records")?
        && fs::read_dir(&records_dir)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Ok(());
    }
    let _ = fs::remove_file(scope_dir.join(".catalog.piraidx"));
    if records_dir.exists() {
        fs::remove_dir(&records_dir).map_err(|error| error.to_string())?;
    }
    fs::remove_dir(&scope_dir).map_err(|error| error.to_string())
}

fn timestamp_from_path(path: &Path) -> Option<u64> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .and_then(|value| value.split('-').next())
        .and_then(|value| value.parse().ok())
}

fn encode_event(event: &Event) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    put_tlv(&mut body, 1, &event.timestamp_ms.to_le_bytes())?;
    put_tlv(&mut body, 2, event.workspace_hash.as_bytes())?;
    put_tlv(&mut body, 3, event.scope_hash.as_bytes())?;
    put_tlv(&mut body, 4, event.intent.as_bytes())?;
    put_tlv(&mut body, 5, event.command.as_bytes())?;
    put_tlv(&mut body, 6, event.category.as_bytes())?;
    put_tlv(&mut body, 7, &event.exit_code.to_le_bytes())?;
    put_tlv(&mut body, 8, &event.duration_ms.to_le_bytes())?;
    put_tlv(&mut body, 9, event.observed.as_bytes())?;
    if let Some(capture_id) = &event.capture_id {
        put_tlv(&mut body, 10, capture_id.as_bytes())?;
    }
    for file in &event.files {
        put_tlv(&mut body, 11, file.as_bytes())?;
    }
    finish_checked(EVENT_MAGIC, body, MAX_EVENT_BYTES)
}

fn decode_event(bytes: &[u8]) -> Result<Event, String> {
    let body = checked_body(bytes, EVENT_MAGIC, MAX_EVENT_BYTES)?;
    let fields = parse_tlvs(body)?;
    let timestamp_ms = required_u64(&fields, 1)?;
    let workspace_hash = required_string(&fields, 2, 128)?;
    let scope_hash = required_string(&fields, 3, 128)?;
    let intent = required_string(&fields, 4, 256)?;
    let command = required_string(&fields, 5, 2_048)?;
    let category = required_string(&fields, 6, 64)?;
    let exit_code = required_i32(&fields, 7)?;
    let duration_ms = required_u64(&fields, 8)?;
    let observed = required_string(&fields, 9, 1_024)?;
    let capture_id = optional_string(&fields, 10, 128)?;
    let files = fields
        .iter()
        .filter(|(tag, _)| *tag == 11)
        .take(16)
        .map(|(_, value)| bounded_string(value, 512))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Event {
        timestamp_ms,
        workspace_hash,
        scope_hash,
        intent,
        command,
        category,
        exit_code,
        duration_ms,
        observed,
        capture_id,
        files,
    })
}

fn read_event_path(path: &Path) -> Result<Event, String> {
    let bytes = util::read_file_limited(path, MAX_EVENT_BYTES, "event")?;
    decode_event(&bytes)
}

fn write_catalog(
    path: &Path,
    workspace_hash: &str,
    scope_hash: &str,
    events: &[EventSummary],
) -> Result<(), String> {
    let mut body = Vec::new();
    put_string(&mut body, workspace_hash)?;
    put_string(&mut body, scope_hash)?;
    put_u32(&mut body, events.len())?;
    for event in events {
        put_string(&mut body, &event.record_name)?;
        body.extend_from_slice(&event.record_size.to_le_bytes());
        body.extend_from_slice(&event.record_modified_ns.to_le_bytes());
        body.extend_from_slice(&event.timestamp_ms.to_le_bytes());
        body.extend_from_slice(&event.exit_code.to_le_bytes());
        put_string(&mut body, &event.scope_hash)?;
        put_string(&mut body, &event.intent)?;
        put_string(&mut body, &event.category)?;
        put_string(&mut body, event.capture_id.as_deref().unwrap_or(""))?;
    }
    atomic_write_relaxed(
        path,
        &finish_checked(CATALOG_MAGIC, body, MAX_CATALOG_BYTES)?,
    )
}

fn read_catalog(
    path: &Path,
    expected_workspace: &str,
    expected_scope: &str,
) -> Result<Vec<EventSummary>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = util::read_file_limited(path, MAX_CATALOG_BYTES, "event catalog")?;
    let body = checked_body(&bytes, CATALOG_MAGIC, MAX_CATALOG_BYTES)?;
    let mut position = 0;
    let workspace = get_string(body, &mut position, 128)?;
    let scope = get_string(body, &mut position, 128)?;
    if workspace != expected_workspace || scope != expected_scope {
        return Err("event catalog scope mismatch".into());
    }
    let count = get_u32(body, &mut position)?;
    if count > MAX_EVENTS_PER_SCOPE {
        return Err("event catalog count exceeds limit".into());
    }
    let mut events = Vec::with_capacity(count);
    for _ in 0..count {
        let record_name = get_string(body, &mut position, 128)?;
        let record_size = take_array::<8>(body, &mut position).map(u64::from_le_bytes)?;
        let record_modified_ns = take_array::<8>(body, &mut position).map(u64::from_le_bytes)?;
        let timestamp_ms = take_array::<8>(body, &mut position).map(u64::from_le_bytes)?;
        let exit_code = take_array::<4>(body, &mut position).map(i32::from_le_bytes)?;
        let scope_hash = get_string(body, &mut position, 128)?;
        let intent = get_string(body, &mut position, 256)?;
        let category = get_string(body, &mut position, 64)?;
        let capture = get_string(body, &mut position, 128)?;
        events.push(EventSummary {
            timestamp_ms,
            scope_hash,
            intent,
            category,
            exit_code,
            capture_id: (!capture.is_empty()).then_some(capture),
            record_size,
            record_modified_ns,
            record_name,
            record_path: PathBuf::new(),
            details: None,
        });
    }
    if position != body.len() {
        return Err("event catalog has trailing bytes".into());
    }
    Ok(events)
}

fn write_retention_snapshot(
    path: &Path,
    workspace_hash: &str,
    state: &RetentionState,
) -> Result<(), String> {
    let mut bytes = retention_header(workspace_hash)?;
    let mut body = vec![1];
    encode_retention_state(&mut body, state)?;
    bytes.extend_from_slice(&retention_frame(&body)?);
    if bytes.len() as u64 > MAX_RETENTION_BYTES {
        return Err("event retention state exceeds size limit".into());
    }
    atomic_write_relaxed(path, &bytes)
}

fn append_retention_delta(
    path: &Path,
    addition: Option<(&str, &RetentionRecord)>,
    removals: &[RetentionRemoval],
) -> Result<(), String> {
    let mut body = vec![2, u8::from(addition.is_some())];
    if let Some((scope_name, record)) = addition {
        put_string(&mut body, scope_name)?;
        put_retention_record(&mut body, record)?;
    }
    put_u32(&mut body, removals.len())?;
    for removal in removals {
        put_string(&mut body, &removal.scope_name)?;
        put_retention_record(&mut body, &removal.record)?;
    }
    let frame = retention_frame(&body)?;
    let mut options = OpenOptions::new();
    options.write(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(&frame).map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())
}

fn retention_header(workspace_hash: &str) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    put_string(&mut body, workspace_hash)?;
    let length = u32::try_from(body.len()).map_err(|_| "event retention header is too large")?;
    let digest = Sha256::digest(&body);
    let mut bytes = Vec::with_capacity(12 + body.len() + digest.len());
    bytes.extend_from_slice(RETENTION_MAGIC);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&digest);
    Ok(bytes)
}

fn retention_frame(body: &[u8]) -> Result<Vec<u8>, String> {
    let length = u32::try_from(body.len()).map_err(|_| "event retention frame is too large")?;
    let digest = Sha256::digest(body);
    let mut bytes = Vec::with_capacity(4 + body.len() + digest.len());
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(&digest);
    Ok(bytes)
}

fn encode_retention_state(out: &mut Vec<u8>, state: &RetentionState) -> Result<(), String> {
    let mut scopes = state.scopes.iter().collect::<Vec<_>>();
    scopes.sort_by_key(|(name, _)| *name);
    put_u32(out, scopes.len())?;
    for (scope_name, records) in scopes {
        put_string(out, scope_name)?;
        put_u32(out, records.len())?;
        for record in records {
            put_retention_record(out, record)?;
        }
    }
    Ok(())
}

fn put_retention_record(out: &mut Vec<u8>, record: &RetentionRecord) -> Result<(), String> {
    out.extend_from_slice(&record.timestamp_ms.to_le_bytes());
    put_string(out, &record.record_name)?;
    out.push(u8::from(record.summary.is_some()));
    if let Some(summary) = &record.summary {
        put_string(out, &summary.intent)?;
        put_string(out, &summary.category)?;
        out.extend_from_slice(&summary.exit_code.to_le_bytes());
        put_string(out, summary.capture_id.as_deref().unwrap_or(""))?;
    }
    Ok(())
}

fn read_retention_state(path: &Path, expected_workspace: &str) -> Result<RetentionState, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("event retention state is not a regular file".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err("event retention state is missing".into());
        }
        Err(error) => return Err(error.to_string()),
    }
    let bytes = util::read_file_limited(path, MAX_RETENTION_BYTES, "event retention state")?;
    if bytes.len() < 44 || bytes.get(..8) != Some(RETENTION_MAGIC) {
        return Err("invalid event retention header".into());
    }
    let mut position = 8;
    let header_length = get_u32(&bytes, &mut position)?;
    let header_end = position
        .checked_add(header_length)
        .filter(|end| {
            end.checked_add(32)
                .is_some_and(|digest_end| digest_end <= bytes.len())
        })
        .ok_or("invalid event retention header length")?;
    let header = &bytes[position..header_end];
    let digest_end = header_end + 32;
    if Sha256::digest(header).as_slice() != &bytes[header_end..digest_end] {
        return Err("event retention header checksum mismatch".into());
    }
    let mut header_position = 0;
    let workspace = get_string(header, &mut header_position, 128)?;
    if header_position != header.len() {
        return Err("event retention header has trailing bytes".into());
    }
    if workspace != expected_workspace {
        return Err("event retention workspace mismatch".into());
    }
    position = digest_end;
    let mut state = RetentionState::default();
    let mut first = true;
    while position < bytes.len() {
        let frame_length = get_u32(&bytes, &mut position)?;
        let frame_end = position
            .checked_add(frame_length)
            .filter(|end| {
                end.checked_add(32)
                    .is_some_and(|digest_end| digest_end <= bytes.len())
            })
            .ok_or("invalid event retention frame length")?;
        let body = &bytes[position..frame_end];
        let digest_end = frame_end + 32;
        if Sha256::digest(body).as_slice() != &bytes[frame_end..digest_end] {
            return Err("event retention frame checksum mismatch".into());
        }
        apply_retention_frame(body, &mut state, first)?;
        first = false;
        position = digest_end;
    }
    if first {
        return Err("event retention state has no snapshot".into());
    }
    Ok(state)
}

fn apply_retention_frame(
    body: &[u8],
    state: &mut RetentionState,
    first: bool,
) -> Result<(), String> {
    let mut position = 0;
    let kind = *body.get(position).ok_or("empty event retention frame")?;
    position += 1;
    match kind {
        1 if first => decode_retention_snapshot(body, &mut position, state)?,
        2 if !first => decode_retention_delta(body, &mut position, state)?,
        _ => return Err("invalid event retention frame sequence".into()),
    }
    if position != body.len() {
        return Err("event retention frame has trailing bytes".into());
    }
    if retention_record_count(state) > MAX_EVENTS_PER_WORKSPACE
        || state
            .scopes
            .values()
            .any(|records| records.len() > MAX_EVENTS_PER_SCOPE)
    {
        return Err("event retention state exceeds limits".into());
    }
    Ok(())
}

fn decode_retention_snapshot(
    body: &[u8],
    position: &mut usize,
    state: &mut RetentionState,
) -> Result<(), String> {
    let scope_count = get_u32(body, position)?;
    if scope_count > MAX_EVENTS_PER_WORKSPACE {
        return Err("event retention scope count exceeds limit".into());
    }
    let mut total = 0_usize;
    for _ in 0..scope_count {
        let scope_name = get_string(body, position, 128)?;
        if !valid_scope_name(&scope_name) || state.scopes.contains_key(&scope_name) {
            return Err("invalid event retention scope".into());
        }
        let record_count = get_u32(body, position)?;
        if record_count > MAX_EVENTS_PER_SCOPE {
            return Err("event retention record count exceeds limit".into());
        }
        total = total
            .checked_add(record_count)
            .ok_or("event retention count overflow")?;
        if total > MAX_EVENTS_PER_WORKSPACE {
            return Err("event retention workspace count exceeds limit".into());
        }
        let mut records = Vec::with_capacity(record_count);
        for _ in 0..record_count {
            records.push(get_retention_record(body, position)?);
        }
        records.sort_by(retention_record_key);
        if records
            .windows(2)
            .any(|pair| pair[0].record_name == pair[1].record_name)
        {
            return Err("duplicate event retention record".into());
        }
        state.scopes.insert(scope_name, records);
    }
    Ok(())
}

fn decode_retention_delta(
    body: &[u8],
    position: &mut usize,
    state: &mut RetentionState,
) -> Result<(), String> {
    let has_addition = match body.get(*position) {
        Some(0) => false,
        Some(1) => true,
        _ => return Err("invalid event retention delta flag".into()),
    };
    *position += 1;
    let addition = if has_addition {
        let scope_name = get_string(body, position, 128)?;
        if !valid_scope_name(&scope_name) {
            return Err("invalid event retention scope".into());
        }
        Some((scope_name, get_retention_record(body, position)?))
    } else {
        None
    };
    let removal_count = get_u32(body, position)?;
    if removal_count > MAX_EVENTS_PER_WORKSPACE {
        return Err("event retention removal count exceeds limit".into());
    }
    if let Some((scope_name, record)) = addition {
        let records = state.scopes.entry(scope_name).or_default();
        if records
            .iter()
            .any(|candidate| candidate.record_name == record.record_name)
        {
            return Err("duplicate event retention record".into());
        }
        insert_retention_record(records, record);
    }
    for _ in 0..removal_count {
        let scope_name = get_string(body, position, 128)?;
        if !valid_scope_name(&scope_name) {
            return Err("invalid event retention scope".into());
        }
        let record = get_retention_record(body, position)?;
        remove_retention_entry(state, &scope_name, &record.record_name);
    }
    Ok(())
}

fn get_retention_record(body: &[u8], position: &mut usize) -> Result<RetentionRecord, String> {
    let timestamp_ms = take_array::<8>(body, position).map(u64::from_le_bytes)?;
    let record_name = get_string(body, position, 128)?;
    if !valid_record_name(&record_name)
        || timestamp_from_path(Path::new(&record_name)) != Some(timestamp_ms)
    {
        return Err("invalid event retention record".into());
    }
    let has_summary = match body.get(*position) {
        Some(0) => false,
        Some(1) => true,
        _ => return Err("invalid event retention summary flag".into()),
    };
    *position += 1;
    let summary = if has_summary {
        let intent = get_string(body, position, 256)?;
        let category = get_string(body, position, 64)?;
        let exit_code = take_array::<4>(body, position).map(i32::from_le_bytes)?;
        let capture_id = get_string(body, position, 128)?;
        Some(RetentionSummary {
            intent,
            category,
            exit_code,
            capture_id: (!capture_id.is_empty()).then_some(capture_id),
        })
    } else {
        None
    };
    Ok(RetentionRecord {
        timestamp_ms,
        record_name,
        summary,
    })
}

fn finish_checked(magic: &[u8; 8], body: Vec<u8>, maximum: u64) -> Result<Vec<u8>, String> {
    let body_length = u32::try_from(body.len()).map_err(|_| "event body is too large")?;
    let total = 8_u64 + 4 + u64::from(body_length) + 32;
    if total > maximum {
        return Err("event record exceeds size limit".into());
    }
    let digest = Sha256::digest(&body);
    let mut bytes = Vec::with_capacity(total as usize);
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&body_length.to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&digest);
    Ok(bytes)
}

fn checked_body<'a>(bytes: &'a [u8], magic: &[u8; 8], maximum: u64) -> Result<&'a [u8], String> {
    if bytes.len() as u64 > maximum || bytes.len() < 44 || &bytes[..8] != magic {
        return Err("invalid event format".into());
    }
    let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let end = 12_usize
        .checked_add(length)
        .ok_or("event length overflow")?;
    if end.checked_add(32) != Some(bytes.len()) {
        return Err("invalid event length".into());
    }
    let body = &bytes[12..end];
    if Sha256::digest(body).as_slice() != &bytes[end..] {
        return Err("event checksum mismatch".into());
    }
    Ok(body)
}

fn put_tlv(out: &mut Vec<u8>, tag: u8, value: &[u8]) -> Result<(), String> {
    out.push(tag);
    let length = u32::try_from(value.len()).map_err(|_| "event field is too large")?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(value);
    Ok(())
}

fn parse_tlvs(body: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut position = 0;
    let mut fields = Vec::new();
    while position < body.len() {
        let tag = *body.get(position).ok_or("truncated event field")?;
        position += 1;
        let length = get_u32(body, &mut position)?;
        let end = position
            .checked_add(length)
            .filter(|end| *end <= body.len())
            .ok_or("invalid event field length")?;
        fields.push((tag, &body[position..end]));
        position = end;
    }
    Ok(fields)
}

fn required_field<'a>(fields: &'a [(u8, &'a [u8])], tag: u8) -> Result<&'a [u8], String> {
    fields
        .iter()
        .find(|(candidate, _)| *candidate == tag)
        .map(|(_, value)| *value)
        .ok_or_else(|| format!("missing event field {tag}"))
}

fn required_string(fields: &[(u8, &[u8])], tag: u8, maximum: usize) -> Result<String, String> {
    bounded_string(required_field(fields, tag)?, maximum)
}

fn optional_string(
    fields: &[(u8, &[u8])],
    tag: u8,
    maximum: usize,
) -> Result<Option<String>, String> {
    fields
        .iter()
        .find(|(candidate, _)| *candidate == tag)
        .map(|(_, value)| bounded_string(value, maximum))
        .transpose()
}

fn bounded_string(bytes: &[u8], maximum: usize) -> Result<String, String> {
    if bytes.len() > maximum {
        return Err("event string exceeds limit".into());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "event string is not UTF-8".into())
}

fn required_u64(fields: &[(u8, &[u8])], tag: u8) -> Result<u64, String> {
    required_field(fields, tag)?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| format!("invalid event integer field {tag}"))
}

fn required_i32(fields: &[(u8, &[u8])], tag: u8) -> Result<i32, String> {
    required_field(fields, tag)?
        .try_into()
        .map(i32::from_le_bytes)
        .map_err(|_| format!("invalid event integer field {tag}"))
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    put_u32(out, value.len())?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn get_string(bytes: &[u8], position: &mut usize, maximum: usize) -> Result<String, String> {
    let length = get_u32(bytes, position)?;
    if length > maximum {
        return Err("event catalog string exceeds limit".into());
    }
    let end = position
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or("invalid event catalog string length")?;
    let value = bounded_string(&bytes[*position..end], maximum)?;
    *position = end;
    Ok(value)
}

fn put_u32(out: &mut Vec<u8>, value: usize) -> Result<(), String> {
    out.extend_from_slice(
        &u32::try_from(value)
            .map_err(|_| "event catalog value is too large")?
            .to_le_bytes(),
    );
    Ok(())
}

fn get_u32(bytes: &[u8], position: &mut usize) -> Result<usize, String> {
    take_array::<4>(bytes, position).map(|value| u32::from_le_bytes(value) as usize)
}

fn take_array<const N: usize>(bytes: &[u8], position: &mut usize) -> Result<[u8; N], String> {
    let end = position.checked_add(N).ok_or("event offset overflow")?;
    let value = bytes
        .get(*position..end)
        .ok_or("truncated event data")?
        .try_into()
        .unwrap();
    *position = end;
    Ok(value)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    atomic_write_with_durability(path, bytes, true)
}

fn atomic_write_relaxed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    atomic_write_with_durability(path, bytes, false)
}

fn atomic_write_with_durability(path: &Path, bytes: &[u8], durable: bool) -> Result<(), String> {
    let temporary =
        path.with_extension(format!("tmp-{}-{:016x}", std::process::id(), short_nonce()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if durable && let Err(error) = file.sync_all() {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    drop(file);
    if let Err(error) = replace_file(&temporary, path, durable) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    #[cfg(unix)]
    if durable && let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path, _durable: bool) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path, durable: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let flags = MOVEFILE_REPLACE_EXISTING | if durable { MOVEFILE_WRITE_THROUGH } else { 0 };
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers that remain
    // alive for the duration of the call.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn category(command: &[String]) -> String {
    let first = command
        .first()
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if matches!(
        first,
        "cargo"
            | "rustc"
            | "make"
            | "cmake"
            | "ninja"
            | "pytest"
            | "tox"
            | "npm"
            | "pnpm"
            | "yarn"
            | "go"
            | "bazel"
            | "gradle"
            | "mvn"
            | "dotnet"
    ) {
        "validation"
    } else if first == "git" {
        "git"
    } else if matches!(first, "cat" | "sed" | "grep" | "find" | "rg") {
        "inspection"
    } else {
        "command"
    }
    .into()
}

fn observed(exit: i32, metadata: Option<&Metadata>) -> String {
    if let Some(metadata) = metadata {
        if metadata.retention_truncated {
            format!(
                "command exited {exit}; {} lines and {} of {} observed bytes retained",
                metadata.total_lines, metadata.total_bytes, metadata.observed_total_bytes
            )
        } else {
            format!(
                "command exited {exit}; {} lines and {} bytes captured",
                metadata.total_lines, metadata.total_bytes
            )
        }
    } else {
        format!("command exited {exit}; output was not captured")
    }
}

static EVENT_COUNTER: AtomicU64 = AtomicU64::new(0);
fn short_nonce() -> u64 {
    (util::millis(SystemTime::now()) as u64)
        ^ u64::from(std::process::id())
        ^ EVENT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> Event {
        Event {
            timestamp_ms: 123,
            workspace_hash: "workspace".into(),
            scope_hash: "scope".into(),
            intent: "verify history".into(),
            command: "cargo test".into(),
            category: "validation".into(),
            exit_code: 0,
            duration_ms: 456,
            observed: "command exited 0".into(),
            capture_id: Some("capture".into()),
            files: vec!["src/lib.rs".into()],
        }
    }

    #[test]
    fn event_binary_round_trip_and_checksum() {
        let event = sample_event();
        let mut bytes = encode_event(&event).unwrap();
        assert_eq!(decode_event(&bytes).unwrap(), event);
        bytes[20] ^= 1;
        assert!(decode_event(&bytes).is_err());
    }

    #[test]
    fn atomic_write_replaces_an_existing_file() {
        let root = std::env::temp_dir().join(format!(
            "pira-event-atomic-write-{}-{:016x}",
            std::process::id(),
            short_nonce()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("state.piraidx");
        fs::write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scope_hash_is_workspace_and_provider_bound() {
        let first = scope_hash("a", "codex", "thread");
        assert_eq!(first.len(), 64);
        assert_ne!(first, scope_hash("b", "codex", "thread"));
        assert_ne!(first, scope_hash("a", "pira", "thread"));
        assert!(!first.contains("thread"));
    }

    #[test]
    fn scope_resolution_supports_claude_and_preserves_priority() {
        let claude = scope_from_environment("workspace", |name| {
            (name == "CLAUDE_CODE_SESSION_ID").then(|| "claude-session".into())
        });
        assert!(claude.detected);
        assert_eq!(
            claude.hash,
            scope_hash("workspace", "claude", "claude-session")
        );

        let preferred = scope_from_environment("workspace", |name| match name {
            "PIRA_CTX_THREAD_ID" => Some("pira-session".into()),
            "CODEX_THREAD_ID" => Some("codex-session".into()),
            "CLAUDE_CODE_SESSION_ID" => Some("claude-session".into()),
            _ => None,
        });
        assert_eq!(
            preferred.hash,
            scope_hash("workspace", "pira", "pira-session")
        );

        let fallback = scope_from_environment("workspace", |name| match name {
            "PIRA_CTX_THREAD_ID" => Some("invalid\nvalue".into()),
            "CODEX_THREAD_ID" => Some(String::new()),
            "CLAUDE_CODE_SESSION_ID" => Some("claude-session".into()),
            _ => None,
        });
        assert!(fallback.detected);
        assert_eq!(fallback.hash, claude.hash);
    }

    #[test]
    fn command_redacts_common_secret_arguments() {
        let command = vec![
            "curl".into(),
            "--token".into(),
            "secret-value".into(),
            "MY_API_KEY=also-secret".into(),
            "safe".into(),
        ];
        let rendered = util::redacted_argv_display(&command);
        assert!(!rendered.contains("secret-value"));
        assert!(!rendered.contains("also-secret"));
        assert!(rendered.contains("safe"));
    }

    #[test]
    fn recap_is_the_newest_bounded_slice() {
        let make = |timestamp_ms| EventSummary {
            timestamp_ms,
            scope_hash: "scope".into(),
            intent: format!("event {timestamp_ms}"),
            category: "command".into(),
            exit_code: 0,
            capture_id: None,
            record_size: 0,
            record_modified_ns: 0,
            record_name: timestamp_ms.to_string(),
            record_path: PathBuf::new(),
            details: None,
        };
        let events = vec![make(1), make(2), make(3)];
        assert_eq!(
            select_recap(&events, 2)
                .iter()
                .map(|event| event.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn scope_and_workspace_retention_are_independently_bounded() {
        let record = |timestamp_ms| RetentionRecord {
            timestamp_ms,
            record_name: format!("{timestamp_ms:020}-0000000001-{timestamp_ms:016x}.piraevt"),
            summary: None,
        };
        let scope_a = "a".repeat(64);
        let mut scope_state = RetentionState::default();
        scope_state.scopes.insert(
            scope_a.clone(),
            (0..MAX_EVENTS_PER_SCOPE as u64).map(record).collect(),
        );
        let removed = plan_retention(&mut scope_state, &scope_a, record(10_000));
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].record.timestamp_ms, 0);
        assert_eq!(retention_record_count(&scope_state), MAX_EVENTS_PER_SCOPE);

        let mut workspace_state = RetentionState::default();
        for (scope_index, prefix) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
            let base = scope_index as u64 * MAX_EVENTS_PER_SCOPE as u64;
            workspace_state.scopes.insert(
                prefix.to_string().repeat(64),
                (base..base + MAX_EVENTS_PER_SCOPE as u64)
                    .map(record)
                    .collect(),
            );
        }
        let newest_scope = "e".repeat(64);
        let removed = plan_retention(&mut workspace_state, &newest_scope, record(20_000));
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].scope_name, scope_a);
        assert_eq!(removed[0].record.timestamp_ms, 0);
        assert_eq!(
            retention_record_count(&workspace_state),
            MAX_EVENTS_PER_WORKSPACE
        );
    }

    #[test]
    fn retention_timestamp_stays_monotonic_when_the_clock_moves_back() {
        let mut state = RetentionState::default();
        state.scopes.insert(
            "a".repeat(64),
            vec![RetentionRecord {
                timestamp_ms: 50,
                record_name: "00000000000000000050-0000000001-0000000000000001.piraevt".into(),
                summary: None,
            }],
        );
        assert_eq!(next_retention_timestamp(&state, 40), 51);
        assert_eq!(next_retention_timestamp(&state, 60), 60);
    }

    #[test]
    fn retention_delta_replays_addition_before_removal() {
        let root = std::env::temp_dir().join(format!(
            "pira-event-retention-delta-{}-{:016x}",
            std::process::id(),
            short_nonce()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("state.piraidx");
        let scope = "a".repeat(64);
        let record = RetentionRecord {
            timestamp_ms: 42,
            record_name: "00000000000000000042-0000000001-0000000000000001.piraevt".into(),
            summary: None,
        };
        write_retention_snapshot(&path, "workspace", &RetentionState::default()).unwrap();
        append_retention_delta(
            &path,
            Some((&scope, &record)),
            &[RetentionRemoval {
                scope_name: scope.clone(),
                record: record.clone(),
            }],
        )
        .unwrap();
        assert_eq!(
            read_retention_state(&path, "workspace").unwrap(),
            RetentionState::default()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retention_state_round_trip_and_checksum() {
        let root = std::env::temp_dir().join(format!(
            "pira-event-retention-state-{}-{:016x}",
            std::process::id(),
            short_nonce()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("state.piraidx");
        let scope = "a".repeat(64);
        let mut state = RetentionState::default();
        state.scopes.insert(
            scope,
            vec![RetentionRecord {
                timestamp_ms: 42,
                record_name: "00000000000000000042-0000000001-0000000000000001.piraevt".into(),
                summary: Some(RetentionSummary {
                    intent: "searchable intent".into(),
                    category: "validation".into(),
                    exit_code: 7,
                    capture_id: Some("capture".into()),
                }),
            }],
        );
        write_retention_snapshot(&path, "workspace", &state).unwrap();
        assert_eq!(read_retention_state(&path, "workspace").unwrap(), state);
        #[cfg(unix)]
        {
            let link = root.join("linked-state.piraidx");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read_retention_state(&link, "workspace").is_err());
        }
        let mut bytes = fs::read(&path).unwrap();
        bytes[20] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(read_retention_state(&path, "workspace").is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
