use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::process::ProcessTree;
use super::state::{JobStatus, SourceKind, WatchState};
use crate::{storage, util};

const MAX_TAIL: usize = 16 * 1024;
const MAX_STREAM_OUTPUT: usize = 512 * 1024;

pub struct Sample {
    pub job: JobStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub activity: bool,
    pub detail: String,
    pub reliable: bool,
}

pub fn collect(
    state: &mut WatchState,
    store: &std::path::Path,
    attempt_timeout_ms: u64,
    stop: impl Fn() -> bool,
) -> Result<Sample, String> {
    match state.source_kind {
        SourceKind::Probe => probe(state, attempt_timeout_ms, stop),
        SourceKind::Capture => capture(state, store),
    }
}

fn capture(state: &mut WatchState, store: &std::path::Path) -> Result<Sample, String> {
    let target = state.source.first().ok_or("capture source missing")?;
    let path = if let Some(path) = state.capture_path.as_ref() {
        path.clone()
    } else {
        storage::resolve_result(store, target)?
    };
    let stored = storage::read_result_path(&path)?;
    let (mut stored, mut growth) =
        stored.sample_growth(state.stdout_offset, state.stderr_offset, MAX_TAIL as u64)?;
    if stored.is_running() && !storage::live_result_is_active(store, &stored) {
        // Owner release follows final publication. Recheck this exact result first.
        let latest = storage::read_result_path(&path)?;
        if latest.is_running() {
            return Err(
                "capture owner lost; interrupted checkpoint cannot establish child outcome".into(),
            );
        }
        (stored, growth) =
            latest.sample_growth(state.stdout_offset, state.stderr_offset, MAX_TAIL as u64)?;
    }
    let activity =
        growth.stdout_total > state.stdout_offset || growth.stderr_total > state.stderr_offset;
    state.stdout_offset = growth.stdout_total;
    state.stderr_offset = growth.stderr_total;
    let job = if stored.is_running() {
        JobStatus::Pending
    } else if stored.metadata.exit_code == 0 {
        JobStatus::Succeeded
    } else {
        JobStatus::Failed
    };
    Ok(Sample {
        job,
        stdout: growth.stdout,
        stderr: growth.stderr,
        activity,
        detail: if growth.truncated {
            "capture growth tail clipped".into()
        } else {
            "capture sampled".into()
        },
        reliable: !growth.truncated
            && !stored.metadata.drain_truncated
            && !stored.metadata.retention_truncated,
    })
}

fn probe(
    state: &WatchState,
    attempt_timeout_ms: u64,
    stop: impl Fn() -> bool,
) -> Result<Sample, String> {
    let mut command = Command::new(state.source.first().ok_or("watch source is empty")?);
    command
        .args(&state.source[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.current_dir(crate::native_path::resolve(
        state.source_cwd_native.as_ref(),
        &state.source_cwd,
    )?);
    let mut tree = ProcessTree::spawn(&mut command, "probe")?;
    let stdout = tree.child.stdout.take().ok_or("probe stdout unavailable")?;
    let stderr = tree.child.stderr.take().ok_or("probe stderr unavailable")?;
    let out = spawn_reader(move || read_tail(stdout));
    let err = spawn_reader(move || read_tail(stderr));
    let started = Instant::now();
    let status = loop {
        if stop() {
            tree.terminate_tree();
            let s = tree.child.wait().map_err(|e| e.to_string())?;
            break (s, true, false);
        }
        if started.elapsed() >= Duration::from_millis(attempt_timeout_ms) {
            tree.terminate_tree();
            let s = tree.child.wait().map_err(|e| e.to_string())?;
            break (s, false, true);
        }
        if let Some(s) = tree.child.try_wait().map_err(|e| e.to_string())? {
            tree.terminate_tree();
            break (s, false, false);
        }
        thread::sleep(Duration::from_millis(50));
    };
    let drain_deadline = Instant::now() + Duration::from_secs(1);
    let (stdout, stdout_complete) = receive_reader(out, drain_deadline, "probe stdout")?;
    let (stderr, stderr_complete) = receive_reader(err, drain_deadline, "probe stderr")?;
    if status.1 {
        return Err("watch stop requested".into());
    }
    if status.2 {
        return Err("probe attempt timed out".into());
    }
    let code = util::status_code(status.0);
    let job = if code == 0 {
        JobStatus::Succeeded
    } else if code == 2 {
        JobStatus::Failed
    } else if code == state.pending_exit {
        JobStatus::Pending
    } else {
        return Err(format!("probe exited with unexpected status {code}"));
    };
    let activity = !stdout.is_empty() || !stderr.is_empty();
    Ok(Sample {
        job,
        activity,
        stdout,
        stderr,
        detail: format!("probe exit {code}"),
        reliable: stdout_complete && stderr_complete,
    })
}

fn spawn_reader<T: Send + 'static>(
    read: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> mpsc::Receiver<Result<T, String>> {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = tx.send(read());
    });
    rx
}

fn receive_reader<T>(
    receiver: mpsc::Receiver<Result<T, String>>,
    deadline: Instant,
    label: &str,
) -> Result<T, String> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| format!("{label} did not close after process-tree cleanup"))?
}

fn read_tail<R: Read>(mut reader: R) -> Result<(Vec<u8>, bool), String> {
    let mut tail = Vec::new();
    let mut total = 0usize;
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n);
        if total > MAX_STREAM_OUTPUT {
            return Err("probe aggregate output exceeds 1 MiB".into());
        }
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > MAX_TAIL {
            tail.drain(..tail.len() - MAX_TAIL);
        }
    }
    Ok((tail, total <= MAX_TAIL))
}

pub fn hash(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for part in parts {
        h.update(part)
    }
    crate::util::hex(&h.finalize())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn probe_state(command: Vec<String>) -> WatchState {
        let mut state: WatchState = serde_json::from_str(
            r#"{
            "schema": 2,
            "id": "watch-1",
            "workspace_hash": "workspace",
            "created_ms": 0,
            "updated_ms": 0,
            "deadline_ms": 10000,
            "source_kind": "probe",
            "source": ["true"],
            "source_cwd": ".",
            "capture_path": null,
            "intent": null,
            "sample_every_ms": 1000,
            "attempt_timeout_ms": 1000,
            "pending_exit": 10,
            "attention_policy": "return",
            "configuration_revision": 0,
            "inactive_after_ms": null,
            "unchanged_after_ms": null,
            "no_progress_after_ms": null,
            "analyzer": null,
            "monitor": "active",
            "job": "unknown",
            "attempt": "idle",
            "attempts": 0,
            "sample_ms": null,
            "next_sample_ms": 0,
            "stdout_offset": 0,
            "stderr_offset": 0,
            "raw_stdout": [],
            "raw_stderr": [],
            "visible_stdout": "",
            "visible_stderr": "",
            "stdout_view": {"engine":"vt100-0.16.2-20x80-v1","transcript":[],"reliable":true,"failure":null},
            "stderr_view": {"engine":"vt100-0.16.2-20x80-v1","transcript":[],"reliable":true,"failure":null},
            "raw_hash": "",
            "visible_hash": "",
            "progress_hash": "",
            "progress": "",
            "analyzer_summary": "",
            "analyzer_error": null,
            "last_activity_ms": null,
            "last_visible_change_ms": null,
            "last_progress_ms": null,
            "attention_reason": null,
            "attention_sequence": 0,
            "detail": "",
            "rendered_reliable": true
        }"#,
        )
        .unwrap();
        state.source = command;
        state
    }

    #[test]
    fn clipped_terminal_prefix_remains_unreliable_for_capture_attention() {
        assert!(read_tail(&b"ok"[..]).unwrap().1);
        assert!(!read_tail(&vec![b'x'; MAX_TAIL + 1][..]).unwrap().1);
        let mut state = probe_state(vec!["true".into()]);
        state.source_kind = SourceKind::Capture;
        state.unchanged_after_ms = Some(1);
        for reliable in [false, true] {
            super::super::incorporate_sample(
                &mut state,
                Sample {
                    job: JobStatus::Pending,
                    stdout: b"same".to_vec(),
                    stderr: vec![],
                    activity: true,
                    detail: String::new(),
                    reliable,
                },
                std::path::Path::new("."),
                false,
                100,
            )
            .unwrap();
            assert!(!state.rendered_reliable);
            super::super::evaluate_attention(&mut state, None, reliable);
            assert!(
                state
                    .attention_reason
                    .as_ref()
                    .unwrap()
                    .contains("unreliable")
            );
        }
    }

    #[test]
    fn incomplete_observation_blocks_inactivity_and_analyzer_stall_claims() {
        for analyzer in [false, true] {
            let mut state = probe_state(vec!["true".into()]);
            if analyzer {
                state.no_progress_after_ms = Some(1);
            } else {
                state.inactive_after_ms = Some(1);
            }
            super::super::evaluate_attention(&mut state, None, false);
            assert!(
                state
                    .attention_reason
                    .as_ref()
                    .unwrap()
                    .contains("sample output is incomplete")
            );
            // Complete silent samples still support the original configured attention.
            super::super::evaluate_attention(&mut state, None, true);
            assert_eq!(
                state.attention_reason.as_deref(),
                Some(if analyzer {
                    "analyzer progress unchanged"
                } else {
                    "no raw activity observed"
                })
            );
        }
    }

    #[test]
    fn incomplete_pending_samples_request_default_attention_once() {
        for rendered in [true, false] {
            let mut state = probe_state(vec!["true".into()]);
            state.rendered_reliable = rendered;
            assert!(super::super::evaluate_attention(&mut state, None, false));
            assert!(state.attention_reason.is_some());
            assert!(!super::super::evaluate_attention(&mut state, None, false));
            assert_eq!(state.attention_sequence, 1);
        }
    }

    #[test]
    fn empty_probe_source_is_rejected_before_sampling() {
        let state = probe_state(vec![]);
        assert!(
            probe(&state, 100, || false)
                .err()
                .unwrap()
                .contains("source is empty")
        );
        assert!(
            super::super::validate(&state, &state.id)
                .unwrap_err()
                .contains("source is empty")
        );
    }

    #[test]
    fn visible_change_tracks_stream_boundaries_not_concatenation() {
        let mut state = probe_state(vec!["true".into()]);
        for (out, err, changed) in [
            ("ab", "c", true),
            ("a", "bc", true),
            ("a", "bc", false),
            ("", "abc", true),
        ] {
            state.last_visible_change_ms = Some(0);
            let sample = Sample {
                stdout: out.as_bytes().to_vec(),
                stderr: err.as_bytes().to_vec(),
                job: JobStatus::Pending,
                detail: String::new(),
                activity: true,
                reliable: true,
            };
            super::super::incorporate_sample(
                &mut state,
                sample,
                std::path::Path::new("."),
                false,
                100,
            )
            .unwrap();
            assert_eq!(state.last_visible_change_ms != Some(0), changed);
        }
    }

    #[test]
    fn direct_exit_with_pipe_holding_descendant_does_not_hang() {
        let state = probe_state(vec![
            "sh".into(),
            "-c".into(),
            "sleep 5 & printf done".into(),
        ]);
        let started = Instant::now();
        let sample = probe(&state, 1_000, || false).unwrap();
        assert_eq!(sample.job, JobStatus::Succeeded);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
