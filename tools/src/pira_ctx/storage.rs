use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt as _;
use sha2::{Digest, Sha256};

use crate::model::{
    BlockDescriptor, CaptureResult, ListedEntry, Metadata, StreamKind, StreamReaders,
};
use crate::{summarize, util};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const MAGIC_V1: &[u8; 8] = b"PIRACTX1";
const MAGIC_V2: &[u8; 8] = b"PIRACTX2";
const MAGIC_V3: &[u8; 8] = b"PIRACTX3";
const MAGIC_V4: &[u8; 8] = b"PIRACTX4";
const FORMAT_VERSION: u32 = 4;
const HEADER_V2_BYTES: u64 = 8 + 4 + 4 + 8 + 8 + 8 + 32 + 32 + 32;
const HEADER_V3_BYTES: u64 = 8 + 4 + 4 + 8 * 6 + 32 * 4;
const HEADER_V4_BYTES: u64 = HEADER_V3_BYTES;
const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LEGACY_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BLOCK_TABLE_BYTES: u64 = 64 * 1024 * 1024;
const BLOCK_BYTES: u64 = 256 * 1024;
const V3_BLOCK_DESCRIPTOR_BYTES: usize = 40;
const V4_BLOCK_DESCRIPTOR_BYTES: usize = 72;
const MAX_DECODED_LINES: usize = 2_000_000;
const FLAG_AUTHENTICATED_TABLES: u32 = 1;
const INDEX_COMPLETE: &str = ".complete-v2";
static RESULT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct StoredResult {
    pub metadata: Metadata,
    pub path: PathBuf,
    pub format_version: u32,
    stdout_offset: u64,
    stderr_offset: u64,
    stdout_hash: Option<[u8; 32]>,
    stderr_hash: Option<[u8; 32]>,
    stdout_blocks: Option<Vec<BlockDescriptor>>,
    stderr_blocks: Option<Vec<BlockDescriptor>>,
    live: Option<LiveState>,
}

pub(crate) struct StreamGrowth {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_total: u64,
    pub stderr_total: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct LiveState {
    pub generation: u64,
    pub checkpoint_unix_ms: u128,
    owner_lock: bool,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl StoredResult {
    pub fn is_running(&self) -> bool {
        self.live.is_some()
    }

    pub fn live_generation(&self) -> Option<u64> {
        self.live.as_ref().map(|state| state.generation)
    }

    pub fn checkpoint_unix_ms(&self) -> Option<u128> {
        self.live.as_ref().map(|state| state.checkpoint_unix_ms)
    }

    pub fn reader(&self) -> Result<StreamReaders, String> {
        if let Some(live) = &self.live {
            return StreamReaders::from_paths(
                &live.stdout_path,
                0,
                self.metadata.stdout_bytes,
                &live.stderr_path,
                0,
                self.metadata.stderr_bytes,
            );
        }
        if let (Some(stdout), Some(stderr)) = (&self.stdout_blocks, &self.stderr_blocks) {
            return StreamReaders::from_blocks(
                &self.path,
                self.stdout_offset,
                self.metadata.stdout_bytes,
                stdout.clone(),
                self.stderr_offset,
                self.metadata.stderr_bytes,
                stderr.clone(),
            );
        }
        StreamReaders::from_paths(
            &self.path,
            self.stdout_offset,
            self.metadata.stdout_bytes,
            &self.path,
            self.stderr_offset,
            self.metadata.stderr_bytes,
        )
    }

    pub(crate) fn current_stream_lengths(&self) -> Result<(u64, u64), String> {
        if let Some(live) = &self.live {
            return Ok((
                fs::metadata(&live.stdout_path)
                    .map_err(|error| format!("read live stdout metadata: {error}"))?
                    .len(),
                fs::metadata(&live.stderr_path)
                    .map_err(|error| format!("read live stderr metadata: {error}"))?
                    .len(),
            ));
        }
        Ok((self.metadata.stdout_bytes, self.metadata.stderr_bytes))
    }

    pub(crate) fn read_stream_growth(
        &self,
        stdout_from: u64,
        stderr_from: u64,
        maximum_each: u64,
    ) -> Result<StreamGrowth, String> {
        let (stdout_total, stderr_total) = self.current_stream_lengths()?;
        if stdout_total < stdout_from || stderr_total < stderr_from {
            return Err("capture streams became shorter than the watch cursor".into());
        }
        let stdout_start = stdout_from.max(stdout_total.saturating_sub(maximum_each));
        let stderr_start = stderr_from.max(stderr_total.saturating_sub(maximum_each));
        let mut readers = if let Some(live) = &self.live {
            StreamReaders::from_paths(
                &live.stdout_path,
                0,
                stdout_total,
                &live.stderr_path,
                0,
                stderr_total,
            )?
        } else {
            self.reader()?
        };
        let stdout = readers.read_section_range(
            StreamKind::Stdout,
            stdout_start,
            stdout_total.saturating_sub(stdout_start),
        )?;
        let stderr = readers.read_section_range(
            StreamKind::Stderr,
            stderr_start,
            stderr_total.saturating_sub(stderr_start),
        )?;
        Ok(StreamGrowth {
            stdout,
            stderr,
            stdout_total,
            stderr_total,
            truncated: stdout_start > stdout_from || stderr_start > stderr_from,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.is_running() {
            return Err("cannot verify a running capture; retry after PROGRAM exits".into());
        }
        if self.stdout_blocks.is_some() {
            let mut readers = self.reader()?;
            let mut stdout = HashSink(Sha256::new());
            let mut stderr = HashSink(Sha256::new());
            readers.copy_section(StreamKind::Stdout, &mut stdout)?;
            readers.copy_section(StreamKind::Stderr, &mut stderr)?;
            let actual_stdout: [u8; 32] = stdout.0.finalize().into();
            let actual_stderr: [u8; 32] = stderr.0.finalize().into();
            if self.stdout_hash.is_some_and(|h| h != actual_stdout)
                || self.stderr_hash.is_some_and(|h| h != actual_stderr)
            {
                return Err("corrupt result: stream checksum mismatch".into());
            }
            return Ok(());
        }
        if let Some(expected) = self.stdout_hash {
            verify_section(
                &self.path,
                self.stdout_offset,
                self.metadata.stdout_bytes,
                &expected,
                "stdout",
            )?;
        }
        if let Some(expected) = self.stderr_hash {
            verify_section(
                &self.path,
                self.stderr_offset,
                self.metadata.stderr_bytes,
                &expected,
                "stderr",
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LiveManifest {
    schema: u32,
    generation: u64,
    checkpoint_unix_ms: u128,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    #[serde(default)]
    owner_lock: bool,
    metadata: Metadata,
}

#[derive(Debug)]
pub struct LiveOwnerLease {
    _file: File,
}

fn live_owner_path(store_dir: &Path, result_id: &str) -> PathBuf {
    store_dir
        .join("live")
        .join("owners")
        .join(format!("{result_id}.lock"))
}

fn acquire_live_owner(store_dir: &Path, result_id: &str) -> Result<LiveOwnerLease, String> {
    let path = live_owner_path(store_dir, result_id);
    ensure_private_dir(path.parent().expect("live owner path has parent"))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path).map_err(|error| error.to_string())?;
    file.try_lock_exclusive()
        .map_err(|_| "live result id already has an owner".to_string())?;
    Ok(LiveOwnerLease { _file: file })
}

fn live_owner_is_active(store_dir: &Path, result_id: &str) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(live_owner_path(store_dir, result_id))
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

#[derive(Debug)]
pub struct LiveCheckpoint<'a> {
    pub command: &'a [String],
    pub cwd: &'a str,
    pub start_ms: u128,
    pub duration_ms: u128,
    pub stdout_path: &'a Path,
    pub stderr_path: &'a Path,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_lines: usize,
    pub stderr_lines: usize,
    pub total_lines: usize,
    pub timeline: &'a [crate::model::LineMeta],
    pub timeline_truncated: bool,
}

pub fn write_live_checkpoint(
    store_dir: &Path,
    result_id: Option<&str>,
    generation: u64,
    owner_lock: bool,
    snapshot: &LiveCheckpoint<'_>,
) -> Result<String, String> {
    ensure_private_dir(store_dir)?;
    let live_dir = store_dir.join("live");
    ensure_private_dir(&live_dir)?;
    let result_id = match result_id {
        Some(value) => value.to_string(),
        None => new_live_result_id(snapshot),
    };
    if !result_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid live result id".into());
    }
    let workspace_id = workspace_id()?;
    let workspace_hash = short_hash(workspace_id.as_bytes(), 16);
    let scope_hash = crate::events::current_scope(&workspace_hash).hash;
    let total_bytes = snapshot.stdout_bytes.saturating_add(snapshot.stderr_bytes);
    let checkpoint_unix_ms = util::millis(SystemTime::now());
    let filename = format!("{result_id}.live.json");
    let path = live_dir.join(&filename);
    let metadata = Metadata {
        compat_version: FORMAT_VERSION,
        tool_version: format!("pira_ctx-{}", env!("CARGO_PKG_VERSION")),
        command_argv: crate::util::redacted_argv(snapshot.command),
        original_command_argv: snapshot.command.to_vec(),
        cwd: snapshot.cwd.to_string(),
        created_at: format_utc_timestamp(snapshot.start_ms / 1000),
        start_unix_ms: snapshot.start_ms,
        end_unix_ms: checkpoint_unix_ms,
        duration_ms: snapshot.duration_ms,
        exit_code: 0,
        stdout_bytes: snapshot.stdout_bytes,
        stderr_bytes: snapshot.stderr_bytes,
        total_bytes,
        observed_stdout_bytes: snapshot.stdout_bytes,
        observed_stderr_bytes: snapshot.stderr_bytes,
        observed_total_bytes: total_bytes,
        retention_truncated: false,
        cancelled: false,
        stdout_lines: snapshot.stdout_lines,
        stderr_lines: snapshot.stderr_lines,
        total_lines: snapshot.total_lines,
        detected_paths: Vec::new(),
        binary_stdout: false,
        binary_stderr: false,
        non_utf8_stdout: false,
        non_utf8_stderr: false,
        line_timeline: snapshot.timeline.to_vec(),
        suggested_keywords: Vec::new(),
        store_dir: store_dir.display().to_string(),
        store_path: path.display().to_string(),
        filename: filename.clone(),
        result_id: result_id.clone(),
        workspace_id,
        workspace_hash,
        scope_hash,
        stdout_sha256: String::new(),
        stderr_sha256: String::new(),
        timeline_truncated: snapshot.timeline_truncated,
    };
    let manifest = LiveManifest {
        schema: 1,
        generation,
        checkpoint_unix_ms,
        stdout_path: snapshot.stdout_path.to_path_buf(),
        stderr_path: snapshot.stderr_path.to_path_buf(),
        owner_lock,
        metadata,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("live checkpoint metadata is too large".into());
    }
    let temporary = live_dir.join(format!(".{result_id}.{}.tmp", std::process::id()));
    write_private_file_relaxed(&temporary, &bytes)?;
    atomic_replace(&temporary, &path)
        .map_err(|error| format!("publish live checkpoint: {error}"))?;
    Ok(result_id)
}

pub fn begin_live_capture(
    store_dir: &Path,
    snapshot: &LiveCheckpoint<'_>,
) -> Result<(String, LiveOwnerLease), String> {
    let result_id = new_live_result_id(snapshot);
    let lease = acquire_live_owner(store_dir, &result_id)?;
    write_live_checkpoint(store_dir, Some(&result_id), 1, true, snapshot)?;
    Ok((result_id, lease))
}

fn new_live_result_id(snapshot: &LiveCheckpoint<'_>) -> String {
    let timestamp = format_utc_timestamp(snapshot.start_ms / 1000);
    let mut seed = Vec::new();
    seed.extend_from_slice(snapshot.cwd.as_bytes());
    seed.extend_from_slice(&snapshot.start_ms.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&RESULT_COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    format!("{}-{}", timestamp, short_hash(&seed, 12))
}

/// Atomically publishes a fully written temporary file at `destination`.
/// Both paths must be on the same filesystem and in the same trust boundary.
pub(crate) fn atomic_replace(temporary: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        fs::rename(temporary, destination)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };

        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: both pointers refer to live, NUL-terminated UTF-16 buffers for this call.
        let ok = unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn live_manifest_path(store_dir: &Path, result_id: &str) -> PathBuf {
    store_dir
        .join("live")
        .join(format!("{result_id}.live.json"))
}

fn cancel_request_path(store_dir: &Path, result_id: &str) -> PathBuf {
    store_dir.join("live").join(format!("{result_id}.cancel"))
}

pub fn request_cancel(store_dir: &Path, target: &str) -> Result<String, String> {
    let path = resolve_result(store_dir, target)?;
    let stored = read_result_path(&path)?;
    if !stored.is_running() || !live_result_is_active(store_dir, &stored) {
        return Err(format!(
            "capture {} is not running",
            stored.metadata.result_id
        ));
    }
    let result_id = stored.metadata.result_id.clone();
    write_private_file_relaxed(&cancel_request_path(store_dir, &result_id), b"cancel\n")?;
    Ok(result_id)
}

pub fn cancellation_requested(store_dir: &Path, result_id: &str) -> bool {
    cancel_request_path(store_dir, result_id).is_file()
}

pub fn remove_live_checkpoint(store_dir: &Path, result_id: &str) {
    let _ = fs::remove_file(live_manifest_path(store_dir, result_id));
    let _ = fs::remove_file(live_owner_path(store_dir, result_id));
    let _ = fs::remove_file(cancel_request_path(store_dir, result_id));
}

struct HashSink(Sha256);
impl Write for HashSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct PruneResult {
    pub removed_files: usize,
    pub removed_bytes: u64,
    pub remaining_files: usize,
    pub remaining_bytes: u64,
}

pub fn effective_store_dir(option: Option<&PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = option {
        return Ok(path.clone());
    }
    if let Some(path) = std::env::var_os("PIRA_CTX_STORE_DIR") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("PIRA").join("ctx"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("PIRA")
            .join("ctx"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
            return Ok(PathBuf::from(path).join("pira").join("ctx"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home).join(".cache").join("pira").join("ctx"));
        }
    }
    Err(
        "cannot determine a per-user pira_ctx store; set PIRA_CTX_STORE_DIR or --store-dir"
            .to_string(),
    )
}

pub fn store_capture(
    store_dir: &Path,
    command: &[String],
    keywords: &[String],
    capture: &CaptureResult,
) -> Result<StoredResult, String> {
    ensure_private_dir(store_dir)?;
    ensure_private_dir(&store_dir.join("indexes"))?;
    let workspace_id = workspace_id()?;
    let workspace_hash = short_hash(workspace_id.as_bytes(), 16);
    let scope_hash = crate::events::current_scope(&workspace_hash).hash;
    let timestamp = format_utc_timestamp(capture.start_ms / 1000);
    let mut seed = Vec::new();
    seed.extend_from_slice(capture.cwd.as_bytes());
    seed.extend_from_slice(&capture.start_ms.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&RESULT_COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    seed.extend_from_slice(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    let base_id = format!("{}-{}", timestamp, short_hash(&seed, 12));
    let (result_id, filename, path) = if let Some(id) = &capture.live_id {
        (
            id.clone(),
            format!("{id}.piractx"),
            store_dir.join(format!("{id}.piractx")),
        )
    } else {
        available_result_path(store_dir, &base_id)
    };
    let detected_paths = summarize::detected_paths(capture)?;
    let suggested_keywords = summarize::suggested_keywords(capture, command, keywords)?;
    let metadata = Metadata {
        compat_version: FORMAT_VERSION,
        tool_version: format!("pira_ctx-{}", env!("CARGO_PKG_VERSION")),
        command_argv: crate::util::redacted_argv(command),
        original_command_argv: command.to_vec(),
        cwd: capture.cwd.clone(),
        created_at: timestamp,
        start_unix_ms: capture.start_ms,
        end_unix_ms: capture.end_ms,
        duration_ms: capture.duration_ms,
        exit_code: capture.exit_code,
        stdout_bytes: capture.stdout.length,
        stderr_bytes: capture.stderr.length,
        total_bytes: capture.total_bytes(),
        observed_stdout_bytes: capture.stdout.observed_length,
        observed_stderr_bytes: capture.stderr.observed_length,
        observed_total_bytes: capture.observed_bytes(),
        retention_truncated: capture.retention_truncated,
        cancelled: capture.cancelled,
        stdout_lines: capture.stdout_lines,
        stderr_lines: capture.stderr_lines,
        total_lines: capture.total_lines,
        detected_paths,
        binary_stdout: capture.stdout.binary,
        binary_stderr: capture.stderr.binary,
        non_utf8_stdout: capture.stdout.non_utf8,
        non_utf8_stderr: capture.stderr.non_utf8,
        line_timeline: capture.timeline.clone(),
        suggested_keywords,
        store_dir: store_dir.display().to_string(),
        store_path: path.display().to_string(),
        filename: filename.clone(),
        result_id: result_id.clone(),
        workspace_id,
        workspace_hash: workspace_hash.clone(),
        scope_hash,
        stdout_sha256: util::hex(&capture.stdout.sha256),
        stderr_sha256: util::hex(&capture.stderr.sha256),
        timeline_truncated: capture.timeline_truncated,
    };
    let dirty = store_dir
        .join("indexes")
        .join(format!(".dirty-{result_id}"));
    write_private_file_relaxed(&dirty, b"dirty\n")?;
    write_container(&path, &metadata, capture)?;
    let entry = ListedEntry::from_metadata(&metadata, path.clone());
    if let Err(error) = update_index(store_dir, &entry, &dirty) {
        let _ = fs::remove_file(store_dir.join("indexes").join(INDEX_COMPLETE));
        eprintln!("pira_ctx: warning: stored result but could not update index: {error}");
    }
    if capture.live_id.is_some() {
        remove_live_checkpoint(store_dir, &result_id);
    }
    read_result_path(&path)
}

fn available_result_path(store_dir: &Path, base_id: &str) -> (String, String, PathBuf) {
    for suffix in 0_u32.. {
        let id = if suffix == 0 {
            base_id.to_string()
        } else {
            format!("{base_id}-{suffix}")
        };
        let filename = format!("{id}.piractx");
        let path = store_dir.join(&filename);
        if !path.exists() {
            return (id, filename, path);
        }
    }
    unreachable!()
}

fn write_container(
    path: &Path,
    metadata: &Metadata,
    capture: &CaptureResult,
) -> Result<(), String> {
    let mut compact_metadata = metadata.clone();
    compact_metadata.line_timeline.clear();
    let metadata_bytes =
        serde_json::to_vec(&compact_metadata).map_err(|error| error.to_string())?;
    let line_index = encode_line_index_v4(&capture.timeline)?;
    if metadata_bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("capture metadata is too large to store safely".to_string());
    }
    if line_index.len() as u64 > MAX_INDEX_BYTES {
        return Err("capture line index is too large to store safely".to_string());
    }
    let metadata_hash: [u8; 32] = Sha256::digest(&metadata_bytes).into();
    let stdout_table_length = block_table_length(capture.stdout.length)?;
    let stderr_table_length = block_table_length(capture.stderr.length)?;
    let temporary = path.with_extension(format!("piractx.tmp-{}", std::process::id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        write_zeros(&mut output, HEADER_V4_BYTES)?;
        output
            .write_all(&metadata_bytes)
            .map_err(|error| error.to_string())?;
        output
            .write_all(&line_index)
            .map_err(|error| error.to_string())?;
        let stdout_table_offset = output.stream_position().map_err(|e| e.to_string())?;
        write_zeros(&mut output, stdout_table_length)?;
        let stderr_table_offset = output.stream_position().map_err(|e| e.to_string())?;
        write_zeros(&mut output, stderr_table_length)?;
        let (stdout_blocks, stdout_stored) = write_block_stream(&capture.stdout, &mut output)?;
        let (stderr_blocks, stderr_stored) = write_block_stream(&capture.stderr, &mut output)?;
        let stdout_table = encode_block_table_v4(&stdout_blocks);
        let stderr_table = encode_block_table_v4(&stderr_blocks);
        if stdout_table.len() as u64 != stdout_table_length
            || stderr_table.len() as u64 != stderr_table_length
        {
            return Err("block table length changed while storing capture".into());
        }
        output
            .seek(SeekFrom::Start(stdout_table_offset))
            .map_err(|e| e.to_string())?;
        output.write_all(&stdout_table).map_err(|e| e.to_string())?;
        output
            .seek(SeekFrom::Start(stderr_table_offset))
            .map_err(|e| e.to_string())?;
        output.write_all(&stderr_table).map_err(|e| e.to_string())?;
        let mut index_hasher = Sha256::new();
        index_hasher.update(&line_index);
        index_hasher.update(&stdout_table);
        index_hasher.update(&stderr_table);
        let line_index_hash: [u8; 32] = index_hasher.finalize().into();
        output.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        output.write_all(MAGIC_V4).map_err(|e| e.to_string())?;
        output
            .write_all(&FORMAT_VERSION.to_le_bytes())
            .map_err(|e| e.to_string())?;
        output
            .write_all(&FLAG_AUTHENTICATED_TABLES.to_le_bytes())
            .map_err(|e| e.to_string())?;
        for length in [
            metadata_bytes.len() as u64,
            line_index.len() as u64,
            stdout_table_length,
            stderr_table_length,
            stdout_stored,
            stderr_stored,
        ] {
            write_u64(&mut output, length)?;
        }
        for hash in [
            metadata_hash,
            line_index_hash,
            capture.stdout.sha256,
            capture.stderr.sha256,
        ] {
            output.write_all(&hash).map_err(|e| e.to_string())?;
        }
        output.sync_all().map_err(|error| error.to_string())?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => {
                let _ = fs::remove_file(&temporary);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(format!("result path already exists: {}", path.display()));
            }
            Err(_) if !path.exists() => {
                fs::rename(&temporary, path).map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(format!("publish {}: {error}", path.display())),
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_zeros(output: &mut File, length: u64) -> Result<(), String> {
    let zeros = [0_u8; 8192];
    let mut remaining = length;
    while remaining > 0 {
        let count = usize::try_from(remaining.min(zeros.len() as u64)).unwrap();
        output
            .write_all(&zeros[..count])
            .map_err(|e| e.to_string())?;
        remaining -= count as u64;
    }
    Ok(())
}

pub fn read_result_path(path: &Path) -> Result<StoredResult, String> {
    if path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.ends_with(".live.json"))
    {
        return read_live_result(path);
    }
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let file_length = file.metadata().map_err(|error| error.to_string())?.len();
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)
        .map_err(|error| format!("corrupt result magic: {error}"))?;
    match &magic {
        MAGIC_V1 => read_v1(path, file, file_length),
        MAGIC_V2 => read_v2(path, file, file_length),
        MAGIC_V3 => read_v3(path, file, file_length),
        MAGIC_V4 => read_v4(path, file, file_length),
        _ => Err("corrupt result: bad magic".to_string()),
    }
}

fn read_live_result(path: &Path) -> Result<StoredResult, String> {
    let bytes = crate::util::read_file_limited(path, MAX_METADATA_BYTES, "live checkpoint")?;
    let manifest: LiveManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid live checkpoint: {error}"))?;
    if manifest.schema != 1 {
        return Err("unsupported live checkpoint schema".into());
    }
    for stream_path in [&manifest.stdout_path, &manifest.stderr_path] {
        let valid_name = stream_path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with(".pira_ctx-spool-"));
        if !valid_name || stream_path.parent() != Some(std::env::temp_dir().as_path()) {
            return Err("invalid live checkpoint stream path".into());
        }
    }
    let stdout_length = fs::metadata(&manifest.stdout_path)
        .map_err(|error| format!("open live stdout: {error}"))?
        .len();
    let stderr_length = fs::metadata(&manifest.stderr_path)
        .map_err(|error| format!("open live stderr: {error}"))?
        .len();
    if manifest.metadata.stdout_bytes > stdout_length
        || manifest.metadata.stderr_bytes > stderr_length
    {
        return Err("live checkpoint exceeds committed stream length".into());
    }
    validate_metadata(
        &manifest.metadata,
        manifest.metadata.stdout_bytes,
        manifest.metadata.stderr_bytes,
    )?;
    Ok(StoredResult {
        metadata: manifest.metadata,
        path: path.to_path_buf(),
        format_version: 0,
        stdout_offset: 0,
        stderr_offset: 0,
        stdout_hash: None,
        stderr_hash: None,
        stdout_blocks: None,
        stderr_blocks: None,
        live: Some(LiveState {
            generation: manifest.generation,
            checkpoint_unix_ms: manifest.checkpoint_unix_ms,
            owner_lock: manifest.owner_lock,
            stdout_path: manifest.stdout_path,
            stderr_path: manifest.stderr_path,
        }),
    })
}

fn encode_line_index_v4(lines: &[crate::model::LineMeta]) -> Result<Vec<u8>, String> {
    if lines.len() > MAX_DECODED_LINES {
        return Err(format!("line index exceeds hard limit {MAX_DECODED_LINES}"));
    }
    let mut out = Vec::with_capacity(8 + lines.len().saturating_mul(17));
    out.extend_from_slice(&(lines.len() as u64).to_le_bytes());
    let mut previous = [0_u64; 2];
    for line in lines {
        let stream = match line.stream {
            StreamKind::Stdout => 0_u8,
            StreamKind::Stderr => 1_u8,
        };
        out.push(stream);
        let slot = stream as usize;
        let delta = line
            .offset
            .checked_sub(previous[slot])
            .ok_or("non-monotonic line offset")?;
        put_varint(&mut out, delta);
        put_varint(&mut out, line.length);
        put_varint(&mut out, zigzag_encode(line.score));
        previous[slot] = line.offset;
    }
    Ok(out)
}

fn decode_line_index_v3(
    bytes: &[u8],
    stdout: u64,
    stderr: u64,
) -> Result<Vec<crate::model::LineMeta>, String> {
    if bytes.len() < 8 {
        return Err("corrupt line index".into());
    }
    let count = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    let count = usize::try_from(count).map_err(|_| "line index too large")?;
    let maximum_count = bytes.len().saturating_sub(8) / 3;
    if count > maximum_count {
        return Err("corrupt line index: impossible line count".into());
    }
    if count > MAX_DECODED_LINES {
        return Err("corrupt line index: too many lines".into());
    }
    let mut position = 8_usize;
    let mut previous = [0_u64; 2];
    let mut lines = Vec::with_capacity(count.min(1_000_000));
    for index in 0..count {
        let stream_byte = *bytes.get(position).ok_or("truncated line index")?;
        position += 1;
        let stream = match stream_byte {
            0 => StreamKind::Stdout,
            1 => StreamKind::Stderr,
            _ => return Err("invalid line-index stream".into()),
        };
        let slot = stream_byte as usize;
        let delta = get_varint(bytes, &mut position)?;
        let length = get_varint(bytes, &mut position)?;
        let offset = previous[slot]
            .checked_add(delta)
            .ok_or("line-index offset overflow")?;
        let section = if slot == 0 { stdout } else { stderr };
        if offset.checked_add(length).is_none_or(|end| end > section) {
            return Err("line index exceeds stream".into());
        }
        previous[slot] = offset;
        lines.push(crate::model::LineMeta {
            line: index + 1,
            stream,
            offset,
            length,
            score: 0,
            flags: 0,
        });
    }
    if position != bytes.len() {
        return Err("trailing bytes in line index".into());
    }
    Ok(lines)
}

fn decode_line_index_v4(
    bytes: &[u8],
    stdout: u64,
    stderr: u64,
) -> Result<Vec<crate::model::LineMeta>, String> {
    if bytes.len() < 8 {
        return Err("corrupt line index".into());
    }
    let count = usize::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
        .map_err(|_| "line index too large")?;
    if count > MAX_DECODED_LINES || count > bytes.len().saturating_sub(8) / 4 {
        return Err("corrupt line index: impossible line count".into());
    }
    let mut position = 8_usize;
    let mut previous = [0_u64; 2];
    let mut lines = Vec::with_capacity(count);
    for index in 0..count {
        let stream_byte = *bytes.get(position).ok_or("truncated line index")?;
        position += 1;
        let stream = match stream_byte {
            0 => StreamKind::Stdout,
            1 => StreamKind::Stderr,
            _ => return Err("invalid line-index stream".into()),
        };
        let slot = stream_byte as usize;
        let delta = get_varint(bytes, &mut position)?;
        let length = get_varint(bytes, &mut position)?;
        let score = zigzag_decode(get_varint(bytes, &mut position)?);
        let offset = previous[slot]
            .checked_add(delta)
            .ok_or("line-index offset overflow")?;
        let section = if slot == 0 { stdout } else { stderr };
        if offset.checked_add(length).is_none_or(|end| end > section) {
            return Err("line index exceeds stream".into());
        }
        previous[slot] = offset;
        lines.push(crate::model::LineMeta {
            line: index + 1,
            stream,
            offset,
            length,
            score,
            flags: 0,
        });
    }
    if position != bytes.len() {
        return Err("trailing bytes in line index".into());
    }
    Ok(lines)
}

fn zigzag_encode(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn zigzag_decode(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
fn get_varint(bytes: &[u8], position: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let byte = *bytes.get(*position).ok_or("truncated varint")?;
        *position += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("oversized varint".into())
}

fn block_table_length(logical_length: u64) -> Result<u64, String> {
    let count = logical_length.div_ceil(BLOCK_BYTES);
    let length = 8_u64
        .checked_add(
            count
                .checked_mul(V4_BLOCK_DESCRIPTOR_BYTES as u64)
                .ok_or("block table overflow")?,
        )
        .ok_or("block table overflow")?;
    if length > MAX_BLOCK_TABLE_BYTES {
        return Err("capture block table is too large to store safely".into());
    }
    Ok(length)
}

fn write_block_stream(
    spool: &crate::model::CapturedStream,
    output: &mut File,
) -> Result<(Vec<BlockDescriptor>, u64), String> {
    const BLOCK: usize = BLOCK_BYTES as usize;
    let mut input = spool.open()?;
    let mut descriptors = Vec::new();
    let mut logical = 0_u64;
    let mut payload = 0_u64;
    let mut buffer = vec![0_u8; BLOCK];
    loop {
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        let compressed = lz4_flex::block::compress(&buffer[..count]);
        let (codec, bytes) = if compressed.len() * 100 <= count * 95 {
            (1_u8, compressed)
        } else {
            (0_u8, buffer[..count].to_vec())
        };
        output.write_all(&bytes).map_err(|e| e.to_string())?;
        descriptors.push(BlockDescriptor {
            codec,
            logical_offset: logical,
            uncompressed_length: count as u64,
            stored_length: bytes.len() as u64,
            payload_offset: payload,
            content_sha256: Some(Sha256::digest(&buffer[..count]).into()),
        });
        logical += count as u64;
        payload += bytes.len() as u64;
    }
    Ok((descriptors, payload))
}
fn encode_block_table_v4(blocks: &[BlockDescriptor]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + blocks.len() * V4_BLOCK_DESCRIPTOR_BYTES);
    out.extend_from_slice(&(blocks.len() as u64).to_le_bytes());
    for b in blocks {
        out.push(b.codec);
        out.extend_from_slice(&[0; 7]);
        for v in [
            b.logical_offset,
            b.uncompressed_length,
            b.stored_length,
            b.payload_offset,
        ] {
            out.extend_from_slice(&v.to_le_bytes())
        }
        out.extend_from_slice(&b.content_sha256.unwrap_or([0_u8; 32]));
    }
    out
}
fn decode_block_table_v3(
    bytes: &[u8],
    logical_length: u64,
    payload_length: u64,
) -> Result<Vec<BlockDescriptor>, String> {
    if bytes.len() < 8 {
        return Err("corrupt block table".into());
    }
    let raw_count = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    let count = usize::try_from(raw_count).map_err(|_| "block table too large")?;
    if bytes.len()
        != 8 + count
            .checked_mul(V3_BLOCK_DESCRIPTOR_BYTES)
            .ok_or("block table overflow")?
    {
        return Err("corrupt block table length".into());
    }
    let mut blocks = Vec::with_capacity(count);
    let mut p = 8;
    let mut logical = 0;
    let mut payload = 0;
    for _ in 0..count {
        let codec = bytes[p];
        if codec > 1 {
            return Err("unsupported block codec".into());
        }
        p += 8;
        let mut values = [0_u64; 4];
        for value in &mut values {
            *value = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
            p += 8
        }
        let b = BlockDescriptor {
            codec,
            logical_offset: values[0],
            uncompressed_length: values[1],
            stored_length: values[2],
            payload_offset: values[3],
            content_sha256: None,
        };
        if b.logical_offset != logical
            || b.payload_offset != payload
            || b.uncompressed_length == 0
            || b.uncompressed_length > BLOCK_BYTES
            || b.stored_length == 0
            || (b.codec == 0 && b.stored_length != b.uncompressed_length)
            || (b.codec == 1 && b.stored_length >= b.uncompressed_length)
        {
            return Err("non-contiguous block table".into());
        }
        logical = logical
            .checked_add(b.uncompressed_length)
            .ok_or("block overflow")?;
        payload = payload
            .checked_add(b.stored_length)
            .ok_or("block overflow")?;
        blocks.push(b)
    }
    if logical != logical_length || payload != payload_length {
        return Err("block table totals disagree".into());
    }
    Ok(blocks)
}

fn decode_block_table_v4(
    bytes: &[u8],
    logical_length: u64,
    payload_length: u64,
) -> Result<Vec<BlockDescriptor>, String> {
    if bytes.len() < 8 {
        return Err("corrupt block table".into());
    }
    let count = usize::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
        .map_err(|_| "block table too large")?;
    if bytes.len()
        != 8 + count
            .checked_mul(V4_BLOCK_DESCRIPTOR_BYTES)
            .ok_or("block table overflow")?
    {
        return Err("corrupt block table length".into());
    }
    let mut blocks = Vec::with_capacity(count);
    let mut p = 8;
    let mut logical = 0_u64;
    let mut payload = 0_u64;
    for _ in 0..count {
        let codec = bytes[p];
        if codec > 1 {
            return Err("unsupported block codec".into());
        }
        p += 8;
        let mut values = [0_u64; 4];
        for value in &mut values {
            *value = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
            p += 8;
        }
        let content_sha256 = Some(bytes[p..p + 32].try_into().unwrap());
        p += 32;
        let block = BlockDescriptor {
            codec,
            logical_offset: values[0],
            uncompressed_length: values[1],
            stored_length: values[2],
            payload_offset: values[3],
            content_sha256,
        };
        if block.logical_offset != logical
            || block.payload_offset != payload
            || block.uncompressed_length == 0
            || block.uncompressed_length > BLOCK_BYTES
            || block.stored_length == 0
            || (block.codec == 0 && block.stored_length != block.uncompressed_length)
            || (block.codec == 1 && block.stored_length >= block.uncompressed_length)
        {
            return Err("non-contiguous block table".into());
        }
        logical = logical
            .checked_add(block.uncompressed_length)
            .ok_or("block overflow")?;
        payload = payload
            .checked_add(block.stored_length)
            .ok_or("block overflow")?;
        blocks.push(block);
    }
    if logical != logical_length || payload != payload_length {
        return Err("block table totals disagree".into());
    }
    Ok(blocks)
}

fn read_v3(path: &Path, mut file: File, file_length: u64) -> Result<StoredResult, String> {
    let version = read_u32(&mut file)?;
    if version != 3 {
        return Err(format!("unsupported pira_ctx format version {version}"));
    }
    let flags = read_u32(&mut file)?;
    if flags & !FLAG_AUTHENTICATED_TABLES != 0 {
        return Err("unsupported PIRACTX3 flags".into());
    }
    let metadata_length = read_u64(&mut file)?;
    let index_length = read_u64(&mut file)?;
    let stdout_table_length = read_u64(&mut file)?;
    let stderr_table_length = read_u64(&mut file)?;
    let stdout_length = read_u64(&mut file)?;
    let stderr_length = read_u64(&mut file)?;
    let metadata_hash = read_hash(&mut file)?;
    let index_hash = read_hash(&mut file)?;
    let stdout_hash = read_hash(&mut file)?;
    let stderr_hash = read_hash(&mut file)?;
    let expected = HEADER_V3_BYTES
        .checked_add(metadata_length)
        .and_then(|v| v.checked_add(index_length))
        .and_then(|v| v.checked_add(stdout_table_length))
        .and_then(|v| v.checked_add(stderr_table_length))
        .and_then(|v| v.checked_add(stdout_length))
        .and_then(|v| v.checked_add(stderr_length))
        .ok_or("capture length overflow")?;
    if expected != file_length
        || metadata_length > MAX_METADATA_BYTES
        || index_length > MAX_INDEX_BYTES
        || stdout_table_length > MAX_BLOCK_TABLE_BYTES
        || stderr_table_length > MAX_BLOCK_TABLE_BYTES
    {
        return Err("corrupt result: inconsistent PIRACTX3 sections".into());
    }
    let metadata_bytes = read_bounded_metadata(&mut file, metadata_length)?;
    let index_bytes = read_bounded_metadata(&mut file, index_length)?;
    let stdout_table = read_bounded_metadata(&mut file, stdout_table_length)?;
    let stderr_table = read_bounded_metadata(&mut file, stderr_table_length)?;
    let actual_metadata_hash: [u8; 32] = Sha256::digest(&metadata_bytes).into();
    let actual_index_hash: [u8; 32] = if flags & FLAG_AUTHENTICATED_TABLES != 0 {
        let mut hasher = Sha256::new();
        hasher.update(&index_bytes);
        hasher.update(&stdout_table);
        hasher.update(&stderr_table);
        hasher.finalize().into()
    } else {
        Sha256::digest(&index_bytes).into()
    };
    if actual_metadata_hash != metadata_hash || actual_index_hash != index_hash {
        return Err("corrupt result: metadata/index checksum mismatch".into());
    }
    let mut metadata: Metadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|e| format!("invalid result metadata: {e}"))?;
    if metadata.compat_version != 3 {
        return Err("unsupported metadata compatibility version".into());
    }
    let stdout_blocks = decode_block_table_v3(&stdout_table, metadata.stdout_bytes, stdout_length)?;
    let stderr_blocks = decode_block_table_v3(&stderr_table, metadata.stderr_bytes, stderr_length)?;
    metadata.line_timeline =
        decode_line_index_v3(&index_bytes, metadata.stdout_bytes, metadata.stderr_bytes)?;
    validate_metadata(&metadata, metadata.stdout_bytes, metadata.stderr_bytes)?;
    if metadata.stdout_sha256 != util::hex(&stdout_hash)
        || metadata.stderr_sha256 != util::hex(&stderr_hash)
    {
        return Err("corrupt result: stream hashes disagree".into());
    }
    let stdout_offset = HEADER_V3_BYTES
        + metadata_length
        + index_length
        + stdout_table_length
        + stderr_table_length;
    Ok(StoredResult {
        metadata,
        path: path.to_path_buf(),
        format_version: 3,
        stdout_offset,
        stderr_offset: stdout_offset + stdout_length,
        stdout_hash: Some(stdout_hash),
        stderr_hash: Some(stderr_hash),
        stdout_blocks: Some(stdout_blocks),
        stderr_blocks: Some(stderr_blocks),
        live: None,
    })
}

fn read_v4(path: &Path, mut file: File, file_length: u64) -> Result<StoredResult, String> {
    let version = read_u32(&mut file)?;
    if version != 4 {
        return Err(format!("unsupported pira_ctx format version {version}"));
    }
    let flags = read_u32(&mut file)?;
    if flags != FLAG_AUTHENTICATED_TABLES {
        return Err("unsupported PIRACTX4 flags".into());
    }
    let metadata_length = read_u64(&mut file)?;
    let index_length = read_u64(&mut file)?;
    let stdout_table_length = read_u64(&mut file)?;
    let stderr_table_length = read_u64(&mut file)?;
    let stdout_length = read_u64(&mut file)?;
    let stderr_length = read_u64(&mut file)?;
    let metadata_hash = read_hash(&mut file)?;
    let index_hash = read_hash(&mut file)?;
    let stdout_hash = read_hash(&mut file)?;
    let stderr_hash = read_hash(&mut file)?;
    let expected = HEADER_V4_BYTES
        .checked_add(metadata_length)
        .and_then(|v| v.checked_add(index_length))
        .and_then(|v| v.checked_add(stdout_table_length))
        .and_then(|v| v.checked_add(stderr_table_length))
        .and_then(|v| v.checked_add(stdout_length))
        .and_then(|v| v.checked_add(stderr_length))
        .ok_or("capture length overflow")?;
    if expected != file_length
        || metadata_length > MAX_METADATA_BYTES
        || index_length > MAX_INDEX_BYTES
        || stdout_table_length > MAX_BLOCK_TABLE_BYTES
        || stderr_table_length > MAX_BLOCK_TABLE_BYTES
    {
        return Err("corrupt result: inconsistent PIRACTX4 sections".into());
    }
    let metadata_bytes = read_bounded_metadata(&mut file, metadata_length)?;
    let index_bytes = read_bounded_metadata(&mut file, index_length)?;
    let stdout_table = read_bounded_metadata(&mut file, stdout_table_length)?;
    let stderr_table = read_bounded_metadata(&mut file, stderr_table_length)?;
    let actual_metadata_hash: [u8; 32] = Sha256::digest(&metadata_bytes).into();
    let mut hasher = Sha256::new();
    hasher.update(&index_bytes);
    hasher.update(&stdout_table);
    hasher.update(&stderr_table);
    let actual_index_hash: [u8; 32] = hasher.finalize().into();
    if actual_metadata_hash != metadata_hash || actual_index_hash != index_hash {
        return Err("corrupt result: metadata/index checksum mismatch".into());
    }
    let mut metadata: Metadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|e| format!("invalid result metadata: {e}"))?;
    if metadata.compat_version != 4 {
        return Err("unsupported metadata compatibility version".into());
    }
    let stdout_blocks = decode_block_table_v4(&stdout_table, metadata.stdout_bytes, stdout_length)?;
    let stderr_blocks = decode_block_table_v4(&stderr_table, metadata.stderr_bytes, stderr_length)?;
    metadata.line_timeline =
        decode_line_index_v4(&index_bytes, metadata.stdout_bytes, metadata.stderr_bytes)?;
    validate_metadata(&metadata, metadata.stdout_bytes, metadata.stderr_bytes)?;
    if metadata.stdout_sha256 != util::hex(&stdout_hash)
        || metadata.stderr_sha256 != util::hex(&stderr_hash)
    {
        return Err("corrupt result: stream hashes disagree".into());
    }
    let stdout_offset = HEADER_V4_BYTES
        + metadata_length
        + index_length
        + stdout_table_length
        + stderr_table_length;
    Ok(StoredResult {
        metadata,
        path: path.to_path_buf(),
        format_version: 4,
        stdout_offset,
        stderr_offset: stdout_offset + stdout_length,
        stdout_hash: Some(stdout_hash),
        stderr_hash: Some(stderr_hash),
        stdout_blocks: Some(stdout_blocks),
        stderr_blocks: Some(stderr_blocks),
        live: None,
    })
}

fn read_v2(path: &Path, mut file: File, file_length: u64) -> Result<StoredResult, String> {
    let version = read_u32(&mut file)?;
    if version != 2 {
        return Err(format!("unsupported pira_ctx format version {version}"));
    }
    let _flags = read_u32(&mut file)?;
    let metadata_length = read_u64(&mut file)?;
    let stdout_length = read_u64(&mut file)?;
    let stderr_length = read_u64(&mut file)?;
    validate_layout(
        file_length,
        HEADER_V2_BYTES,
        metadata_length,
        stdout_length,
        stderr_length,
    )?;
    let metadata_hash = read_hash(&mut file)?;
    let stdout_hash = read_hash(&mut file)?;
    let stderr_hash = read_hash(&mut file)?;
    let metadata_bytes = read_bounded_metadata(&mut file, metadata_length)?;
    let actual_metadata_hash: [u8; 32] = Sha256::digest(&metadata_bytes).into();
    if actual_metadata_hash != metadata_hash {
        return Err("corrupt result: metadata checksum mismatch".to_string());
    }
    let metadata: Metadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|error| format!("invalid result metadata: {error}"))?;
    if metadata.compat_version != 2 {
        return Err(format!(
            "unsupported metadata compatibility version {}",
            metadata.compat_version
        ));
    }
    validate_metadata(&metadata, stdout_length, stderr_length)?;
    if metadata.stdout_sha256 != util::hex(&stdout_hash)
        || metadata.stderr_sha256 != util::hex(&stderr_hash)
    {
        return Err("corrupt result: metadata stream checksums disagree with header".to_string());
    }
    let stdout_offset = HEADER_V2_BYTES + metadata_length;
    Ok(StoredResult {
        metadata,
        path: path.to_path_buf(),
        format_version: version,
        stdout_offset,
        stderr_offset: stdout_offset + stdout_length,
        stdout_hash: Some(stdout_hash),
        stderr_hash: Some(stderr_hash),
        stdout_blocks: None,
        stderr_blocks: None,
        live: None,
    })
}

fn read_v1(path: &Path, mut file: File, file_length: u64) -> Result<StoredResult, String> {
    let metadata_length = read_u64(&mut file)?;
    if metadata_length > MAX_LEGACY_METADATA_BYTES {
        return Err("corrupt result: metadata is too large".to_string());
    }
    let metadata_bytes = read_bounded_metadata(&mut file, metadata_length)?;
    let metadata: Metadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|error| format!("invalid result metadata: {error}"))?;
    if metadata.compat_version != 1 {
        return Err(format!(
            "unsupported legacy metadata version {}",
            metadata.compat_version
        ));
    }
    let stdout_length = read_u64(&mut file)?;
    let stdout_offset = 8 + 8 + metadata_length + 8;
    let stderr_length_offset = stdout_offset
        .checked_add(stdout_length)
        .ok_or_else(|| "corrupt result: length overflow".to_string())?;
    if stderr_length_offset + 8 > file_length {
        return Err("corrupt result: stdout length exceeds file".to_string());
    }
    file.seek(SeekFrom::Start(stderr_length_offset))
        .map_err(|error| error.to_string())?;
    let stderr_length = read_u64(&mut file)?;
    let stderr_offset = stderr_length_offset + 8;
    let expected = stderr_offset
        .checked_add(stderr_length)
        .ok_or_else(|| "corrupt result: length overflow".to_string())?;
    if expected != file_length {
        return Err("corrupt result: inconsistent payload lengths".to_string());
    }
    validate_metadata(&metadata, stdout_length, stderr_length)?;
    Ok(StoredResult {
        metadata,
        path: path.to_path_buf(),
        format_version: 1,
        stdout_offset,
        stderr_offset,
        stdout_hash: None,
        stderr_hash: None,
        stdout_blocks: None,
        stderr_blocks: None,
        live: None,
    })
}

fn validate_layout(
    file_length: u64,
    header: u64,
    metadata: u64,
    stdout: u64,
    stderr: u64,
) -> Result<(), String> {
    if metadata > MAX_LEGACY_METADATA_BYTES {
        return Err("corrupt result: metadata is too large".to_string());
    }
    let expected = header
        .checked_add(metadata)
        .and_then(|value| value.checked_add(stdout))
        .and_then(|value| value.checked_add(stderr))
        .ok_or_else(|| "corrupt result: length overflow".to_string())?;
    if expected != file_length {
        return Err("corrupt result: inconsistent payload lengths".to_string());
    }
    Ok(())
}

fn validate_metadata(metadata: &Metadata, stdout: u64, stderr: u64) -> Result<(), String> {
    if metadata.stdout_bytes != stdout || metadata.stderr_bytes != stderr {
        return Err("corrupt result: metadata stream lengths disagree with container".to_string());
    }
    if metadata.total_bytes != stdout.saturating_add(stderr) {
        return Err("corrupt result: invalid total byte count".to_string());
    }
    let observed_sum = metadata
        .observed_stdout_bytes
        .saturating_add(metadata.observed_stderr_bytes);
    if metadata.retention_truncated {
        if metadata.observed_stdout_bytes < stdout
            || metadata.observed_stderr_bytes < stderr
            || metadata.observed_total_bytes != observed_sum
            || metadata.observed_total_bytes <= metadata.total_bytes
        {
            return Err("corrupt result: invalid truncated retention lengths".to_string());
        }
    } else if metadata.observed_total_bytes != 0
        && (metadata.observed_stdout_bytes != stdout
            || metadata.observed_stderr_bytes != stderr
            || metadata.observed_total_bytes != metadata.total_bytes)
    {
        return Err("corrupt result: invalid observed stream lengths".to_string());
    }
    if (!metadata.timeline_truncated && metadata.total_lines != metadata.line_timeline.len())
        || (metadata.timeline_truncated && metadata.total_lines < metadata.line_timeline.len())
    {
        return Err("corrupt result: invalid timeline line count".to_string());
    }
    let mut previous_line = 0;
    for line in &metadata.line_timeline {
        if line.line <= previous_line {
            return Err("corrupt result: non-increasing timeline".to_string());
        }
        previous_line = line.line;
        let section = match line.stream {
            StreamKind::Stdout => stdout,
            StreamKind::Stderr => stderr,
        };
        if line
            .offset
            .checked_add(line.length)
            .is_none_or(|end| end > section)
        {
            return Err(format!(
                "corrupt result: invalid timeline offset at L{}",
                line.line
            ));
        }
    }
    Ok(())
}

fn read_bounded_metadata(file: &mut File, length: u64) -> Result<Vec<u8>, String> {
    let size =
        usize::try_from(length).map_err(|_| "metadata does not fit this platform".to_string())?;
    let mut bytes = vec![0_u8; size];
    file.read_exact(&mut bytes)
        .map_err(|error| format!("corrupt result metadata: {error}"))?;
    Ok(bytes)
}

fn verify_section(
    path: &Path,
    offset: u64,
    length: u64,
    expected: &[u8; 32],
    name: &str,
) -> Result<(), String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut limited = file.take(length);
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    loop {
        let count = limited
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if &actual != expected {
        return Err(format!("corrupt result: {name} checksum mismatch"));
    }
    Ok(())
}

pub fn scan_store(
    store_dir: &Path,
    workspace_filter: Option<&str>,
) -> Result<Vec<ListedEntry>, String> {
    if !store_dir.exists() {
        return Ok(Vec::new());
    }
    let indexes = store_dir.join("indexes");
    let mut entries = if indexes.join(INDEX_COMPLETE).is_file() && !indexes_dirty(&indexes) {
        match read_indexes(&indexes, workspace_filter) {
            Ok(entries) => Ok(entries),
            Err(_) => {
                let _ = fs::remove_file(indexes.join(INDEX_COMPLETE));
                scan_result_headers(store_dir, workspace_filter)
            }
        }?
    } else {
        scan_result_headers(store_dir, workspace_filter)?
    };
    let completed_ids: std::collections::HashSet<_> =
        entries.iter().map(|entry| entry.id.clone()).collect();
    entries.extend(
        scan_live_headers(store_dir, workspace_filter)
            .into_iter()
            .filter(|entry| !completed_ids.contains(&entry.id)),
    );
    sort_entries(&mut entries);
    Ok(entries)
}

fn scan_live_headers(store_dir: &Path, workspace_filter: Option<&str>) -> Vec<ListedEntry> {
    let live_dir = store_dir.join("live");
    let Ok(items) = fs::read_dir(live_dir) else {
        return Vec::new();
    };
    items
        .filter_map(Result::ok)
        .filter_map(|item| {
            let path = item.path();
            let stored = read_result_path(&path).ok()?;
            if workspace_filter.is_some_and(|filter| stored.metadata.workspace_hash != filter) {
                return None;
            }
            let mut entry = ListedEntry::from_metadata(&stored.metadata, path);
            let active = live_result_is_active(store_dir, &stored);
            entry.running = active;
            entry.state = if active { "running" } else { "interrupted" }.into();
            entry.exit = 0;
            Some(entry)
        })
        .collect()
}

pub(crate) fn resolve_current_live_capture(store_dir: &Path) -> Result<String, String> {
    let workspace_hash = current_workspace_hash()?;
    let scope = crate::events::current_scope(&workspace_hash);
    if !scope.detected {
        return Err("--current requires an automatically detected agent thread".into());
    }
    let live_dir = store_dir.join("live");
    let mut matches = fs::read_dir(&live_dir)
        .map(|items| {
            items
                .filter_map(Result::ok)
                .filter_map(|item| read_result_path(&item.path()).ok())
                .filter(|stored| {
                    stored.metadata.workspace_hash == workspace_hash
                        && stored.metadata.scope_hash == scope.hash
                        && live_result_is_active(store_dir, stored)
                        && !store_dir
                            .join(stored.metadata.filename.replace(".live.json", ".piractx"))
                            .exists()
                })
                .map(|stored| stored.metadata.result_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    matches.sort();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => Err("--current found no live capture in the current agent thread".into()),
        _ => Err(format!(
            "--current found multiple live captures in the current agent thread: {}; use --capture ID",
            matches.join(", ")
        )),
    }
}

fn live_result_is_active(store_dir: &Path, stored: &StoredResult) -> bool {
    let Some(live) = stored.live.as_ref() else {
        return false;
    };
    if live.owner_lock {
        return live_owner_is_active(store_dir, &stored.metadata.result_id);
    }
    // Compatibility for checkpoints written before owner leases existed. They are
    // considered live only while checkpoints are still reasonably fresh.
    util::millis(SystemTime::now()).saturating_sub(live.checkpoint_unix_ms) < 120_000
}

fn scan_result_headers(
    store_dir: &Path,
    workspace_filter: Option<&str>,
) -> Result<Vec<ListedEntry>, String> {
    let mut entries = Vec::new();
    for item in
        fs::read_dir(store_dir).map_err(|error| format!("read {}: {error}", store_dir.display()))?
    {
        let path = item.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("piractx") {
            continue;
        }
        if let Ok(stored) = read_result_path(&path)
            && workspace_filter.is_none_or(|filter| stored.metadata.workspace_hash == filter)
        {
            entries.push(ListedEntry::from_metadata(&stored.metadata, path));
        }
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn read_indexes(
    indexes: &Path,
    workspace_filter: Option<&str>,
) -> Result<Vec<ListedEntry>, String> {
    let mut entries = Vec::new();
    let paths: Vec<PathBuf> = if let Some(workspace) = workspace_filter {
        vec![indexes.join(format!("{workspace}.jsonl"))]
    } else {
        fs::read_dir(indexes)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
            })
            .collect()
    };
    let mut seen = HashSet::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let reader = BufReader::new(File::open(&path).map_err(|error| error.to_string())?);
        for line in reader.lines() {
            let line = line.map_err(|error| error.to_string())?;
            if line.len() > 64 * 1024 {
                return Err("corrupt capture index: oversized row".into());
            }
            let entry: ListedEntry = serde_json::from_str(&line)
                .map_err(|_| "corrupt capture index: invalid row".to_string())?;
            let mut entry = entry;
            if !valid_capture_filename(&entry.filename) {
                return Err("corrupt capture index: invalid filename".into());
            }
            let store_dir = indexes.parent().ok_or("corrupt capture index location")?;
            entry.path = store_dir.join(&entry.filename);
            if entry.path.is_file() && seen.insert(entry.path.clone()) {
                entries.push(entry);
            }
        }
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn valid_capture_filename(value: &str) -> bool {
    let path = Path::new(value);
    path.components().count() == 1
        && path.extension().and_then(|extension| extension.to_str()) == Some("piractx")
}

fn sort_entries(entries: &mut [ListedEntry]) {
    entries.sort_by(|a, b| b.start_ms.cmp(&a.start_ms).then_with(|| b.id.cmp(&a.id)));
}

fn update_index(store_dir: &Path, entry: &ListedEntry, current_dirty: &Path) -> Result<(), String> {
    let indexes = store_dir.join("indexes");
    ensure_private_dir(&indexes)?;
    let _lock = StoreLock::acquire(&indexes.join(".index.lock"))?;
    if !indexes.join(INDEX_COMPLETE).is_file() || indexes_dirty_except(&indexes, current_dirty) {
        rebuild_indexes_locked(store_dir, &indexes)?;
        clear_dirty_markers(&indexes);
        return Ok(());
    }
    let path = indexes.join(format!("{}.jsonl", entry.workspace_hash));
    append_index(&path, entry)?;
    match fs::remove_file(current_dirty) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn indexes_dirty(indexes: &Path) -> bool {
    fs::read_dir(indexes).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".dirty-"))
        })
    })
}

fn indexes_dirty_except(indexes: &Path, current: &Path) -> bool {
    fs::read_dir(indexes).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry.path() != current
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(".dirty-"))
        })
    })
}

fn clear_dirty_markers(indexes: &Path) {
    if let Ok(entries) = fs::read_dir(indexes) {
        for entry in entries.filter_map(Result::ok) {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".dirty-"))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn rebuild_indexes_locked(store_dir: &Path, indexes: &Path) -> Result<(), String> {
    for item in fs::read_dir(indexes).map_err(|error| error.to_string())? {
        let path = item.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    let entries = scan_result_headers(store_dir, None)?;
    let mut grouped: HashMap<String, Vec<ListedEntry>> = HashMap::new();
    for entry in entries {
        grouped
            .entry(entry.workspace_hash.clone())
            .or_default()
            .push(entry);
    }
    for (workspace, entries) in grouped {
        let path = indexes.join(format!("{workspace}.jsonl"));
        for entry in entries {
            append_index(&path, &entry)?;
        }
    }
    write_private_file(&indexes.join(INDEX_COMPLETE), b"2\n")?;
    Ok(())
}

fn append_index(path: &Path, entry: &ListedEntry) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut file, entry).map_err(|error| error.to_string())?;
    // The index is derived from durable captures and can be rebuilt. Avoid an
    // additional disk barrier on every command.
    file.write_all(b"\n").map_err(|error| error.to_string())
}

pub fn resolve_result(store_dir: &Path, target: &str) -> Result<PathBuf, String> {
    if target == "--last" {
        let workspace = current_workspace_hash()?;
        return scan_store(store_dir, Some(&workspace))?
            .into_iter()
            .find(|entry| !entry.running)
            .map(|entry| entry.path)
            .ok_or_else(|| "no stored pira_ctx result for current workspace".to_string());
    }
    let path = PathBuf::from(target);
    if path.is_absolute()
        || path.components().count() > 1
        || (target.ends_with(".piractx") && path.exists())
    {
        return Ok(path);
    }
    if target.ends_with(".piractx") {
        let candidate = store_dir.join(target);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    if target
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && let Ok(items) = fs::read_dir(store_dir)
    {
        for item in items.flatten() {
            let path = item.path();
            if path.extension().and_then(|value| value.to_str()) != Some("piractx") {
                continue;
            }
            if let Ok(stored) = read_result_path(&path)
                && stored.metadata.result_id == target
            {
                return Ok(path);
            }
        }
    }
    let workspace = current_workspace_hash()?;
    let entries = scan_store(store_dir, Some(&workspace))?;
    let matches: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry.id == target
                || entry.id.starts_with(target)
                || entry.filename == target
                || entry.filename.starts_with(target)
        })
        .collect();
    match matches.as_slice() {
        [] => Err(format!("no result matches {target}")),
        [entry] => Ok(entry.path.clone()),
        _ => Err(format!("ambiguous result id/name {target}")),
    }
}

pub fn prune_store(
    store_dir: &Path,
    max_age_days: Option<u64>,
    max_store_bytes: Option<u64>,
) -> Result<PruneResult, String> {
    ensure_private_dir(store_dir)?;
    let mut entries = scan_store(store_dir, None)?;
    entries.retain(|entry| !entry.running);
    entries.sort_by_key(|entry| entry.start_ms);
    let now = util::millis(SystemTime::now());
    let cutoff = max_age_days.map(|days| now.saturating_sub(days as u128 * 86_400_000));
    let mut remove = HashSet::new();
    for entry in &entries {
        if cutoff.is_some_and(|cutoff| entry.start_ms < cutoff) {
            remove.insert(entry.path.clone());
        }
    }
    let mut remaining_bytes: u64 = entries
        .iter()
        .filter(|entry| !remove.contains(&entry.path))
        .map(entry_disk_size)
        .sum();
    if let Some(maximum) = max_store_bytes {
        for entry in &entries {
            if remaining_bytes <= maximum {
                break;
            }
            if remove.insert(entry.path.clone()) {
                remaining_bytes = remaining_bytes.saturating_sub(entry_disk_size(entry));
            }
        }
    }
    let mut result = PruneResult::default();
    for entry in &entries {
        if remove.contains(&entry.path) {
            let disk_size = entry_disk_size(entry);
            fs::remove_file(&entry.path)
                .map_err(|error| format!("remove {}: {error}", entry.path.display()))?;
            result.removed_files += 1;
            result.removed_bytes = result.removed_bytes.saturating_add(disk_size);
        } else {
            result.remaining_files += 1;
            result.remaining_bytes = result
                .remaining_bytes
                .saturating_add(entry_disk_size(entry));
        }
    }
    let indexes = store_dir.join("indexes");
    if indexes.exists() {
        let _lock = StoreLock::acquire(&indexes.join(".index.lock"))?;
        let _ = fs::remove_file(indexes.join(INDEX_COMPLETE));
        rebuild_indexes_locked(store_dir, &indexes)?;
    }
    Ok(result)
}

fn entry_disk_size(entry: &ListedEntry) -> u64 {
    entry
        .path
        .metadata()
        .map_or(entry.bytes, |metadata| metadata.len())
}

pub fn current_workspace_hash() -> Result<String, String> {
    Ok(short_hash(workspace_id()?.as_bytes(), 16))
}

fn workspace_id() -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let root = nearest_git_root(&cwd).unwrap_or(cwd);
    Ok(root.canonicalize().unwrap_or(root).display().to_string())
}

fn nearest_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

pub(crate) fn ensure_private_dir(path: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing symlinked store directory {}",
                path.display()
            ));
        }
        if !metadata.is_dir() {
            return Err(format!("store path is not a directory: {}", path.display()));
        }
    } else {
        fs::create_dir_all(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn write_private_file_relaxed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())
}

struct StoreLock {
    path: PathBuf,
}

impl StoreLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        for attempt in 0..100 {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id()).map_err(|error| error.to_string())?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(path) {
                        let _ = fs::remove_file(path);
                    } else {
                        thread::sleep(Duration::from_millis(20 + attempt));
                    }
                }
                Err(error) => return Err(format!("create index lock: {error}")),
            }
        }
        Err("timed out waiting for pira_ctx index lock".to_string())
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> bool {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .and_then(|modified| modified.elapsed().map_err(io::Error::other))
        .is_ok_and(|age| age > Duration::from_secs(300))
}

fn read_u32(reader: &mut File) -> Result<u32, String> {
    let mut bytes = [0_u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| format!("corrupt header: {error}"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut File) -> Result<u64, String> {
    let mut bytes = [0_u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| format!("corrupt length: {error}"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn write_u64(writer: &mut File, value: u64) -> Result<(), String> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(|error| error.to_string())
}

fn read_hash(reader: &mut File) -> Result<[u8; 32], String> {
    let mut bytes = [0_u8; 32];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| format!("corrupt checksum: {error}"))?;
    Ok(bytes)
}

fn short_hash(bytes: &[u8], characters: usize) -> String {
    let digest = Sha256::digest(bytes);
    util::hex(&digest)[..characters].to_string()
}

pub(crate) fn format_utc_timestamp(seconds: u128) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = (seconds % 86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (
        (year + i64::from(month <= 2)) as i32,
        month as u32,
        day as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_rejects_impossible_count_and_oversized_varint() {
        let mut impossible = 2_u64.to_le_bytes().to_vec();
        impossible.extend_from_slice(&[0, 0, 1]);
        assert!(decode_line_index_v4(&impossible, 1, 0).is_err());

        let mut oversized = 1_u64.to_le_bytes().to_vec();
        oversized.push(0);
        oversized.extend_from_slice(&[0x80; 10]);
        oversized.push(0);
        assert!(decode_line_index_v4(&oversized, 1, 0).is_err());
    }

    #[test]
    fn block_table_rejects_oversized_and_inconsistent_blocks() {
        let oversized = encode_block_table_v4(&[BlockDescriptor {
            codec: 0,
            logical_offset: 0,
            uncompressed_length: BLOCK_BYTES + 1,
            stored_length: BLOCK_BYTES + 1,
            payload_offset: 0,
            content_sha256: Some([0; 32]),
        }]);
        assert!(decode_block_table_v4(&oversized, BLOCK_BYTES + 1, BLOCK_BYTES + 1).is_err());

        let bad_raw = encode_block_table_v4(&[BlockDescriptor {
            codec: 0,
            logical_offset: 0,
            uncompressed_length: 8,
            stored_length: 4,
            payload_offset: 0,
            content_sha256: Some([0; 32]),
        }]);
        assert!(decode_block_table_v4(&bad_raw, 8, 4).is_err());
    }
}
