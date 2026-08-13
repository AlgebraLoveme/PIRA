use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::process::ProcessTree;
use super::state::AnalyzerSpec;

const MAX_ANALYZER_BYTES: usize = 1024 * 1024;
const MAX_RESULT_BYTES: usize = 16 * 1024;

#[derive(Serialize)]
pub struct AnalyzerInput<'a> {
    pub source: &'a str,
    pub job_state: &'a str,
    pub raw_stdout: &'a str,
    pub raw_stderr: &'a str,
    pub visible_stdout: &'a str,
    pub visible_stderr: &'a str,
    pub attempts: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct AnalyzerOutput {
    #[serde(default)]
    pub progress: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub attention: bool,
}

pub fn load_code(path: &std::path::Path) -> Result<String, String> {
    let bytes = crate::util::read_file_limited(path, MAX_ANALYZER_BYTES as u64, "watch analyzer")?;
    String::from_utf8(bytes).map_err(|_| "watch analyzer must be UTF-8 Python".to_string())
}

fn directory(store: &Path) -> PathBuf {
    super::state::root(store).join("analyzers")
}

pub fn store_code(store: &Path, code: &str) -> Result<String, String> {
    if code.len() > MAX_ANALYZER_BYTES {
        return Err("analyzer code exceeds 1 MiB".into());
    }
    let hash = crate::util::hex(&Sha256::digest(code.as_bytes()));
    let directory = directory(store);
    crate::storage::ensure_private_dir(&directory)?;
    let path = directory.join(format!("{hash}.py"));
    if path.is_file() {
        let existing = crate::util::read_file_limited(
            &path,
            MAX_ANALYZER_BYTES as u64,
            "stored watch analyzer",
        )?;
        if existing == code.as_bytes() {
            return Ok(hash);
        }
        return Err("stored analyzer hash collision or corruption".into());
    }
    let temporary = directory.join(format!(".{hash}.{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    if let Err(error) = file
        .write_all(code.as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(format!("store watch analyzer: {error}"));
    }
    drop(file);
    if let Err(error) = crate::storage::atomic_replace(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("publish watch analyzer: {error}"));
    }
    Ok(hash)
}

pub fn spec(store: &Path, code: &str, revision: u64) -> Result<AnalyzerSpec, String> {
    Ok(AnalyzerSpec {
        revision,
        code_hash: store_code(store, code)?,
        code: String::new(),
    })
}

fn resolve_code(store: &Path, spec: &AnalyzerSpec) -> Result<String, String> {
    if !spec.code.is_empty() {
        return Ok(spec.code.clone());
    }
    if spec.code_hash.len() != 64 || !spec.code_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid stored analyzer hash".into());
    }
    load_code(&directory(store).join(format!("{}.py", spec.code_hash)))
}

pub fn run(
    spec: &AnalyzerSpec,
    input: &AnalyzerInput<'_>,
    timeout_ms: u64,
    store: &Path,
    cwd: &std::path::Path,
    stop: impl Fn() -> bool,
) -> Result<AnalyzerOutput, String> {
    let code = resolve_code(store, spec)?;
    let mut command = Command::new("python3");
    command
        .args(["-c", &code])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.current_dir(cwd);
    let mut tree = ProcessTree::spawn(&mut command, "analyzer")?;
    let bytes = serde_json::to_vec(input).map_err(|e| e.to_string())?;
    let mut stdin = tree
        .child
        .stdin
        .take()
        .ok_or("analyzer stdin unavailable")?;
    let stdout = tree
        .child
        .stdout
        .take()
        .ok_or("analyzer stdout unavailable")?;
    let stderr = tree
        .child
        .stderr
        .take()
        .ok_or("analyzer stderr unavailable")?;
    let writer = spawn_io(move || stdin.write_all(&bytes).map_err(|e| e.to_string()));
    let stdout_reader = spawn_io(move || read_bounded(stdout, MAX_RESULT_BYTES));
    let stderr_reader = spawn_io(move || read_bounded(stderr, MAX_RESULT_BYTES));

    let started = Instant::now();
    let (status, stopped, timed_out) = loop {
        if stop() {
            tree.terminate_tree();
            break (
                tree.child
                    .wait()
                    .map_err(|e| format!("wait analyzer: {e}"))?,
                true,
                false,
            );
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            tree.terminate_tree();
            break (
                tree.child
                    .wait()
                    .map_err(|e| format!("wait analyzer: {e}"))?,
                false,
                true,
            );
        }
        if let Some(status) = tree
            .child
            .try_wait()
            .map_err(|e| format!("wait analyzer: {e}"))?
        {
            tree.terminate_tree();
            break (status, false, false);
        }
        thread::sleep(Duration::from_millis(50));
    };

    let drain_deadline = Instant::now() + Duration::from_secs(1);
    receive_io(writer, drain_deadline, "analyzer stdin")?;
    let (stdout, stdout_overflow) = receive_io(stdout_reader, drain_deadline, "analyzer stdout")?;
    let (stderr, _) = receive_io(stderr_reader, drain_deadline, "analyzer stderr")?;
    if stopped {
        return Err("watch stop requested".into());
    }
    if timed_out {
        return Err("analyzer attempt timed out".into());
    }
    if stdout_overflow {
        return Err("analyzer output exceeds 16 KiB".into());
    }
    if !status.success() {
        return Err(format!(
            "analyzer exit {}: {}",
            crate::util::status_code(status),
            crate::util::single_line_clip(&String::from_utf8_lossy(&stderr), 1000)
        ));
    }
    let mut parsed: AnalyzerOutput = serde_json::from_slice(&stdout)
        .map_err(|e| format!("analyzer must emit one JSON object: {e}"))?;
    if parsed.progress.len() > 4096 || parsed.summary.len() > 8192 {
        return Err("analyzer fields exceed limits".into());
    }
    parsed.progress = crate::util::sanitize_terminal(&parsed.progress);
    parsed.summary = crate::util::sanitize_terminal(&parsed.summary);
    Ok(parsed)
}

fn spawn_io<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> mpsc::Receiver<Result<T, String>> {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx
}

fn receive_io<T>(
    receiver: mpsc::Receiver<Result<T, String>>,
    deadline: Instant,
    label: &str,
) -> Result<T, String> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| format!("{label} did not close after process-tree cleanup"))?
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Result<(Vec<u8>, bool), String> {
    let mut output = Vec::new();
    let mut overflow = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        overflow |= count > remaining;
    }
    Ok((output, overflow))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> AnalyzerInput<'static> {
        AnalyzerInput {
            source: "probe",
            job_state: "pending",
            raw_stdout: "step=2",
            raw_stderr: "",
            visible_stdout: "step=2",
            visible_stderr: "",
            attempts: 2,
        }
    }

    #[test]
    fn analyzer_returns_structured_progress() {
        let spec = AnalyzerSpec {
            revision: 1,
            code_hash: String::new(),
            code: "import json; d=json.load(__import__('sys').stdin); print(json.dumps({'progress': d['visible_stdout']}))".into(),
        };
        let result = run(
            &spec,
            &input(),
            2_000,
            std::path::Path::new("."),
            std::path::Path::new("."),
            || false,
        )
        .unwrap();
        assert_eq!(result.progress, "step=2");
    }

    #[test]
    fn analyzer_timeout_is_bounded() {
        let spec = AnalyzerSpec {
            revision: 1,
            code_hash: String::new(),
            code: "import time; time.sleep(2)".into(),
        };
        let error = run(
            &spec,
            &input(),
            100,
            std::path::Path::new("."),
            std::path::Path::new("."),
            || false,
        )
        .unwrap_err();
        assert_eq!(error, "analyzer attempt timed out");
    }

    #[cfg(unix)]
    #[test]
    fn analyzer_direct_exit_cleans_pipe_holding_descendant() {
        let spec = AnalyzerSpec {
            revision: 1,
            code_hash: String::new(),
            code: "import subprocess; subprocess.Popen(['sleep', '5']); print('{}')".into(),
        };
        let started = Instant::now();
        let result = run(
            &spec,
            &input(),
            1_000,
            std::path::Path::new("."),
            std::path::Path::new("."),
            || false,
        )
        .unwrap();
        assert!(result.progress.is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
