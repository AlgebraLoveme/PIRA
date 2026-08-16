use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::model::{CaptureResult, CapturedStream, LineMeta, StreamKind};
use crate::util;
use crate::watch::process::ProcessTree;

const DEFAULT_MAX_RETAINED_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MAX_INDEXED_LINES: usize = 1_000_000;
const HARD_MAX_INDEXED_LINES: usize = 2_000_000;
const DEFAULT_LIVE_CHECKPOINT_MS: u64 = 30_000;
const LIVE_ANNOUNCEMENT_DELAY_MS: u128 = 250;
const CAPTURE_CONTROL_POLL_MS: u64 = 25;
const MEMORY_SPOOL_BYTES: usize = 64 * 1024;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Debug)]
struct StreamLine {
    stream: StreamKind,
    offset: u64,
    length: u64,
}

#[derive(Debug)]
struct StreamAnalysis {
    length: u64,
    observed_length: u64,
    sha256: [u8; 32],
    binary: bool,
    non_utf8: bool,
}

#[derive(Clone)]
struct CollectedLines {
    timeline: Vec<LineMeta>,
    total: usize,
    stdout: usize,
    stderr: usize,
    truncated: bool,
    stdout_bytes: u64,
    stderr_bytes: u64,
    stdout_line_start: u64,
    stderr_line_start: u64,
}

struct RetentionBudget {
    maximum: u64,
    used: AtomicU64,
}

impl RetentionBudget {
    fn reserve(&self, requested: usize) -> usize {
        let requested = requested as u64;
        let mut used = self.used.load(Ordering::Relaxed);
        loop {
            if used >= self.maximum {
                return 0;
            }
            let granted = requested.min(self.maximum - used);
            match self.used.compare_exchange_weak(
                used,
                used + granted,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return granted as usize,
                Err(actual) => used = actual,
            }
        }
    }
}

pub fn capture_command(
    cmd: &[String],
    live_store_dir: Option<&Path>,
    announce_live: bool,
) -> Result<Result<CaptureResult, i32>, String> {
    if cmd.is_empty() {
        return Err(crate::cli::USAGE.to_string());
    }
    let cwd_path = std::env::current_dir().map_err(|error| error.to_string())?;
    let cwd = cwd_path
        .canonicalize()
        .unwrap_or(cwd_path)
        .display()
        .to_string();
    let start = SystemTime::now();
    let elapsed = Instant::now();
    let start_ms = util::millis(start);
    let retained_limit = configured_u64(
        "PIRA_CTX_MAX_RETAINED_BYTES",
        DEFAULT_MAX_RETAINED_BYTES,
        4 * 1024,
    );
    let indexed_line_limit = configured_usize(
        "PIRA_CTX_MAX_INDEXED_LINES",
        DEFAULT_MAX_INDEXED_LINES,
        1_000,
        HARD_MAX_INDEXED_LINES,
    );
    let budget = Arc::new(RetentionBudget {
        maximum: retained_limit,
        used: AtomicU64::new(0),
    });
    let stdout_spool = Arc::new(Mutex::new(AdaptiveSpool::new("stdout", start_ms)));
    let stderr_spool = Arc::new(Mutex::new(AdaptiveSpool::new("stderr", start_ms)));
    let (initial_live_id, live_owner) = if announce_live {
        let store_dir = live_store_dir.ok_or("live announcement requires capture storage")?;
        let (stdout_path, stderr_path) = live_spool_paths(&stdout_spool, &stderr_spool)?;
        let checkpoint = crate::storage::LiveCheckpoint {
            command: cmd,
            cwd: &cwd,
            start_ms,
            duration_ms: 0,
            stdout_path: &stdout_path,
            stderr_path: &stderr_path,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_lines: 0,
            stderr_lines: 0,
            total_lines: 0,
            timeline: &[],
            timeline_truncated: false,
        };
        let (id, owner) = crate::storage::begin_live_capture(store_dir, &checkpoint)?;
        (Some(id), Some(owner))
    } else {
        (None, None)
    };
    let mut tree = match ProcessTree::spawn_capture(cmd) {
        Ok(tree) => tree,
        Err(error) if error.starts_with("__EXIT127__ ") => {
            remove_initial_checkpoint(live_store_dir, initial_live_id.as_deref());
            eprintln!("pira_ctx: {}", error.trim_start_matches("__EXIT127__ "));
            return Ok(Err(127));
        }
        Err(error) if error.starts_with("__EXIT126__ ") => {
            remove_initial_checkpoint(live_store_dir, initial_live_id.as_deref());
            eprintln!("pira_ctx: {}", error.trim_start_matches("__EXIT126__ "));
            return Ok(Err(126));
        }
        Err(error) => {
            remove_initial_checkpoint(live_store_dir, initial_live_id.as_deref());
            return Err(error);
        }
    };
    let child_stdout = tree
        .child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let child_stderr = tree
        .child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;
    let collected = Arc::new(Mutex::new(CollectedLines {
        timeline: Vec::new(),
        total: 0,
        stdout: 0,
        stderr: 0,
        truncated: false,
        stdout_bytes: 0,
        stderr_bytes: 0,
        stdout_line_start: 0,
        stderr_line_start: 0,
    }));
    let stdout_collected = Arc::clone(&collected);
    let stdout_budget = Arc::clone(&budget);
    let stdout_writer = Arc::clone(&stdout_spool);
    let stdout_handle = thread::spawn(move || {
        read_stream(
            child_stdout,
            &stdout_writer,
            StreamKind::Stdout,
            &stdout_collected,
            indexed_line_limit,
            &stdout_budget,
        )
    });
    let stderr_collected = Arc::clone(&collected);
    let stderr_budget = Arc::clone(&budget);
    let stderr_writer = Arc::clone(&stderr_spool);
    let stderr_handle = thread::spawn(move || {
        read_stream(
            child_stderr,
            &stderr_writer,
            StreamKind::Stderr,
            &stderr_collected,
            indexed_line_limit,
            &stderr_budget,
        )
    });
    let checkpoint_interval = Duration::from_millis(configured_u64(
        "PIRA_CTX_LIVE_CHECKPOINT_MS",
        DEFAULT_LIVE_CHECKPOINT_MS,
        100,
    ));
    let (checkpoint_stop, checkpoint_receiver) = mpsc::channel();
    let shared_live_id = Arc::new(Mutex::new(initial_live_id.clone()));
    let checkpoint_handle = live_store_dir.map(|store_dir| {
        let store_dir = store_dir.to_path_buf();
        let command = cmd.to_vec();
        let cwd = cwd.clone();
        let stdout_spool = Arc::clone(&stdout_spool);
        let stderr_spool = Arc::clone(&stderr_spool);
        let collected = Arc::clone(&collected);
        let shared_live_id = Arc::clone(&shared_live_id);
        thread::spawn(move || {
            let mut live_id = shared_live_id.lock().ok().and_then(|id| id.clone());
            let mut generation = u64::from(live_id.is_some());
            let mut last_progress = None;
            loop {
                match checkpoint_receiver.recv_timeout(checkpoint_interval) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let snapshot = {
                    let Ok(state) = collected.lock() else {
                        break;
                    };
                    let progress = (
                        state.stdout_bytes,
                        state.stderr_bytes,
                        state.total,
                        state.truncated,
                    );
                    if last_progress == Some(progress) {
                        continue;
                    }
                    last_progress = Some(progress);
                    state.clone()
                };
                let paths = live_spool_paths(&stdout_spool, &stderr_spool);
                let Ok((stdout_path, stderr_path)) = paths else {
                    break;
                };
                generation = generation.saturating_add(1);
                let mut timeline = snapshot.timeline.clone();
                let mut total_lines = snapshot.total;
                let mut stdout_lines = snapshot.stdout;
                let mut stderr_lines = snapshot.stderr;
                if !snapshot.truncated {
                    for (stream, start, end) in [
                        (
                            StreamKind::Stdout,
                            snapshot.stdout_line_start,
                            snapshot.stdout_bytes,
                        ),
                        (
                            StreamKind::Stderr,
                            snapshot.stderr_line_start,
                            snapshot.stderr_bytes,
                        ),
                    ] {
                        if start < end {
                            total_lines += 1;
                            match stream {
                                StreamKind::Stdout => stdout_lines += 1,
                                StreamKind::Stderr => stderr_lines += 1,
                            }
                            timeline.push(LineMeta {
                                line: total_lines,
                                stream,
                                offset: start,
                                length: end - start,
                                score: 0,
                                flags: 0,
                            });
                        }
                    }
                }
                let checkpoint = crate::storage::LiveCheckpoint {
                    command: &command,
                    cwd: &cwd,
                    start_ms,
                    duration_ms: elapsed.elapsed().as_millis(),
                    stdout_path: &stdout_path,
                    stderr_path: &stderr_path,
                    stdout_bytes: snapshot.stdout_bytes,
                    stderr_bytes: snapshot.stderr_bytes,
                    stdout_lines,
                    stderr_lines,
                    total_lines,
                    timeline: &timeline,
                    timeline_truncated: snapshot.truncated,
                };
                match crate::storage::write_live_checkpoint(
                    &store_dir,
                    live_id.as_deref(),
                    generation,
                    announce_live,
                    &checkpoint,
                ) {
                    Ok(id) => {
                        if let Ok(mut shared) = shared_live_id.lock() {
                            *shared = Some(id.clone());
                        }
                        live_id = Some(id);
                    }
                    Err(_) => break,
                }
            }
            live_id
        })
    });
    let mut live_announced = false;
    let mut cancelled = false;
    let status = loop {
        if announce_live
            && !live_announced
            && elapsed.elapsed().as_millis() >= LIVE_ANNOUNCEMENT_DELAY_MS
            && let Some(result_id) = initial_live_id.as_deref()
        {
            eprintln!("LIVE | result={result_id}");
            live_announced = true;
        }
        let active_live_id = shared_live_id.lock().ok().and_then(|id| id.clone());
        if let (Some(store_dir), Some(result_id)) = (live_store_dir, active_live_id.as_deref())
            && crate::storage::cancellation_requested(store_dir, result_id)
        {
            cancelled = true;
            tree.terminate_tree();
            break tree.child.wait().map_err(|error| error.to_string())?;
        }
        if let Some(status) = tree.child.try_wait().map_err(|error| error.to_string())? {
            tree.terminate_tree();
            break status;
        }
        thread::sleep(Duration::from_millis(CAPTURE_CONTROL_POLL_MS));
    };
    let _ = checkpoint_stop.send(());
    let live_id = checkpoint_handle
        .and_then(|handle| handle.join().ok())
        .flatten();
    let end_ms = util::millis(SystemTime::now());
    let stdout_analysis = join_reader(stdout_handle, "stdout")?;
    let stderr_analysis = join_reader(stderr_handle, "stderr")?;
    let collected = collected
        .lock()
        .map_err(|_| "capture state lock poisoned".to_string())?
        .clone();
    let retention_truncated = stdout_analysis.observed_length > stdout_analysis.length
        || stderr_analysis.observed_length > stderr_analysis.length;
    let stdout = finish_spool(stdout_spool, stdout_analysis, "stdout")?;
    let stderr = finish_spool(stderr_spool, stderr_analysis, "stderr")?;
    let exit_code = util::status_code(status);
    let capture = CaptureResult {
        stdout,
        stderr,
        timeline: collected.timeline,
        total_lines: collected.total,
        stdout_lines: collected.stdout,
        stderr_lines: collected.stderr,
        timeline_truncated: collected.truncated || retention_truncated,
        retention_truncated,
        cancelled,
        exit_code,
        start_ms,
        end_ms,
        duration_ms: elapsed.elapsed().as_millis(),
        cwd,
        live_id,
        live_store_dir: live_store_dir.map(Path::to_path_buf),
        _live_owner: live_owner,
    };
    Ok(Ok(capture))
}

fn remove_initial_checkpoint(store_dir: Option<&Path>, result_id: Option<&str>) {
    if let (Some(store_dir), Some(result_id)) = (store_dir, result_id) {
        crate::storage::remove_live_checkpoint(store_dir, result_id);
    }
}

fn join_reader(
    handle: thread::JoinHandle<io::Result<StreamAnalysis>>,
    name: &str,
) -> Result<StreamAnalysis, String> {
    handle
        .join()
        .map_err(|_| format!("{name} reader panicked"))?
        .map_err(|error| format!("{name} reader failed: {error}"))
}

fn live_spool_paths(
    stdout: &Mutex<AdaptiveSpool>,
    stderr: &Mutex<AdaptiveSpool>,
) -> Result<(PathBuf, PathBuf), String> {
    let stdout = stdout
        .lock()
        .map_err(|_| "stdout spool lock poisoned".to_string())?
        .ensure_file()?
        .to_path_buf();
    let stderr = stderr
        .lock()
        .map_err(|_| "stderr spool lock poisoned".to_string())?
        .ensure_file()?
        .to_path_buf();
    Ok((stdout, stderr))
}

fn finish_spool(
    spool: Arc<Mutex<AdaptiveSpool>>,
    analysis: StreamAnalysis,
    name: &str,
) -> Result<CapturedStream, String> {
    let spool = Arc::try_unwrap(spool)
        .map_err(|_| format!("{name} spool still shared after capture"))?
        .into_inner()
        .map_err(|_| format!("{name} spool lock poisoned"))?;
    spool.finish(analysis)
}

struct AdaptiveSpool {
    stream: &'static str,
    start_ms: u128,
    memory: Vec<u8>,
    file: Option<File>,
    guard: Option<SpoolGuard>,
}

impl AdaptiveSpool {
    fn new(stream: &'static str, start_ms: u128) -> Self {
        Self {
            stream,
            start_ms,
            memory: Vec::new(),
            file: None,
            guard: None,
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.file.is_none()
            && self.memory.len().saturating_add(bytes.len()) <= MEMORY_SPOOL_BYTES
        {
            self.memory.extend_from_slice(bytes);
            return Ok(());
        }
        self.ensure_file()?;
        self.file
            .as_mut()
            .ok_or_else(|| "temporary capture writer was not initialized".to_string())?
            .write_all(bytes)
            .map_err(|error| error.to_string())
    }

    fn ensure_file(&mut self) -> Result<&Path, String> {
        if self.file.is_none() {
            let (guard, mut file) = create_spool(self.stream, self.start_ms)?;
            file.write_all(&self.memory)
                .map_err(|error| format!("initialize temporary capture: {error}"))?;
            self.memory.clear();
            self.memory.shrink_to_fit();
            self.guard = Some(guard);
            self.file = Some(file);
        }
        self.guard
            .as_ref()
            .and_then(SpoolGuard::path)
            .ok_or_else(|| "temporary capture path was not initialized".to_string())
    }

    fn finish(mut self, analysis: StreamAnalysis) -> Result<CapturedStream, String> {
        if self.file.take().is_some() {
            let path = self
                .guard
                .as_mut()
                .and_then(SpoolGuard::disarm)
                .ok_or_else(|| "file-backed capture is missing its path".to_string())?;
            Ok(CapturedStream::file(
                path,
                analysis.length,
                analysis.observed_length,
                analysis.sha256,
                analysis.binary,
                analysis.non_utf8,
            ))
        } else {
            Ok(CapturedStream::memory(
                self.memory,
                analysis.observed_length,
                analysis.sha256,
                analysis.binary,
                analysis.non_utf8,
            ))
        }
    }
}

fn create_spool(stream: &str, start_ms: u128) -> Result<(SpoolGuard, File), String> {
    let directory = std::env::temp_dir();
    for nonce in 0..100_u32 {
        let filename = format!(
            ".pira_ctx-spool-{}-{start_ms}-{stream}-{nonce}",
            std::process::id()
        );
        let path = directory.join(filename);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path) {
            Ok(file) => return Ok((SpoolGuard { path: Some(path) }, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create temporary capture: {error}")),
        }
    }
    Err("could not create unique temporary capture file".to_string())
}

struct SpoolGuard {
    path: Option<PathBuf>,
}

impl SpoolGuard {
    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn disarm(&mut self) -> Option<PathBuf> {
        self.path.take()
    }
}

impl Drop for SpoolGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn read_stream<R: Read>(
    mut input: R,
    output: &Mutex<AdaptiveSpool>,
    stream: StreamKind,
    collected: &Mutex<CollectedLines>,
    indexed_line_limit: usize,
    budget: &RetentionBudget,
) -> io::Result<StreamAnalysis> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    let mut offset = 0_u64;
    let mut observed = 0_u64;
    let mut line_start = 0_u64;
    let mut null_seen = false;
    let mut controls = 0_u64;
    let mut validator = Utf8Validator::default();
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        observed = observed.saturating_add(count as u64);
        let retained = budget.reserve(count);
        if retained == 0 {
            continue;
        }
        let chunk = &buffer[..retained];
        output
            .lock()
            .map_err(|_| io::Error::other("capture spool lock poisoned"))?
            .write_all(chunk)
            .map_err(io::Error::other)?;
        hasher.update(chunk);
        validator.feed(chunk);
        null_seen |= chunk.contains(&0);
        controls += chunk
            .iter()
            .filter(|&&byte| byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t' | 0x1b))
            .count() as u64;
        let mut lines = Vec::new();
        for (index, byte) in chunk.iter().enumerate() {
            if *byte == b'\n' {
                let end = offset + index as u64 + 1;
                lines.push(StreamLine {
                    stream,
                    offset: line_start,
                    length: end - line_start,
                });
                line_start = end;
            }
        }
        offset += retained as u64;
        commit_stream_progress(
            collected,
            stream,
            offset,
            line_start,
            lines,
            indexed_line_limit,
        )?;
    }
    if line_start < offset {
        commit_stream_progress(
            collected,
            stream,
            offset,
            offset,
            vec![StreamLine {
                stream,
                offset: line_start,
                length: offset - line_start,
            }],
            indexed_line_limit,
        )?;
    }
    // This is an ephemeral spool. Closing the writer makes its bytes visible to
    // the later reader; durable synchronization belongs to the final capture.
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(StreamAnalysis {
        length: offset,
        observed_length: observed,
        sha256: digest,
        binary: null_seen || (offset > 0 && controls.saturating_mul(100) / offset > 30),
        non_utf8: validator.finish(),
    })
}

fn commit_stream_progress(
    shared: &Mutex<CollectedLines>,
    stream: StreamKind,
    length: u64,
    line_start: u64,
    lines: Vec<StreamLine>,
    maximum: usize,
) -> io::Result<()> {
    let mut state = shared
        .lock()
        .map_err(|_| io::Error::other("capture state lock poisoned"))?;
    match stream {
        StreamKind::Stdout => {
            state.stdout_bytes = length;
            state.stdout_line_start = line_start;
        }
        StreamKind::Stderr => {
            state.stderr_bytes = length;
            state.stderr_line_start = line_start;
        }
    }
    for event in lines {
        state.total += 1;
        match event.stream {
            StreamKind::Stdout => state.stdout += 1,
            StreamKind::Stderr => state.stderr += 1,
        }
        let line = LineMeta {
            line: state.total,
            stream: event.stream,
            offset: event.offset,
            length: event.length,
            score: 0,
            flags: 0,
        };
        if state.timeline.len() < maximum {
            state.timeline.push(line);
        } else {
            state.truncated = true;
        }
    }
    Ok(())
}

fn configured_u64(name: &str, default: u64, minimum: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(default, |value| value.max(minimum))
}

fn configured_usize(name: &str, default: usize, minimum: usize, maximum: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(default, |value| value.clamp(minimum, maximum))
}

#[derive(Default)]
struct Utf8Validator {
    tail: Vec<u8>,
    invalid: bool,
}

impl Utf8Validator {
    fn feed(&mut self, chunk: &[u8]) {
        if self.invalid {
            return;
        }
        let mut bytes = std::mem::take(&mut self.tail);
        bytes.extend_from_slice(chunk);
        match std::str::from_utf8(&bytes) {
            Ok(_) => {}
            Err(error) if error.error_len().is_some() => self.invalid = true,
            Err(error) => self.tail.extend_from_slice(&bytes[error.valid_up_to()..]),
        }
    }

    fn finish(self) -> bool {
        self.invalid || !self.tail.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis(bytes: &[u8]) -> StreamAnalysis {
        StreamAnalysis {
            length: bytes.len() as u64,
            observed_length: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
            binary: false,
            non_utf8: false,
        }
    }

    #[test]
    fn short_spool_stays_in_memory_and_replays_exactly() {
        let bytes = b"small output\n";
        let mut adaptive = AdaptiveSpool::new("test-memory", util::millis(SystemTime::now()));
        adaptive.write_all(bytes).unwrap();
        assert!(adaptive.file.is_none());
        let spool = adaptive.finish(analysis(bytes)).unwrap();
        let mut reader = spool.open().unwrap();
        let mut replayed = Vec::new();
        reader.read_to_end(&mut replayed).unwrap();
        assert_eq!(replayed, bytes);
    }

    #[test]
    fn large_spool_spills_once_and_removes_its_file_on_drop() {
        let bytes = vec![b'x'; MEMORY_SPOOL_BYTES + 1];
        let mut adaptive = AdaptiveSpool::new("test-file", util::millis(SystemTime::now()));
        adaptive.write_all(&bytes).unwrap();
        let path = adaptive
            .guard
            .as_ref()
            .and_then(SpoolGuard::path)
            .unwrap()
            .to_path_buf();
        assert!(path.is_file());
        let spool = adaptive.finish(analysis(&bytes)).unwrap();
        let mut reader = spool.open().unwrap();
        let mut replayed = Vec::new();
        reader.read_to_end(&mut replayed).unwrap();
        assert_eq!(replayed, bytes);
        drop(spool);
        assert!(!path.exists());
    }
}
