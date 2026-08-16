#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pira-ctx-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_pira_ctx")
}

fn run(store: &Path, arguments: &[&str]) -> Output {
    let mut argv = vec![arguments[0], "--store-dir", store.to_str().unwrap()];
    argv.extend_from_slice(&arguments[1..]);
    Command::new(binary()).args(argv).output().unwrap()
}

#[test]
fn completed_probe_reports_final_exit_instead_of_an_idle_attempt() {
    let sandbox = Sandbox::new("watch-complete-probe");
    let output = run(
        sandbox.path(),
        &["watch", "--deadline", "2s", "--", "sh", "-c", "exit 0"],
    );
    assert_eq!(output.status.code(), Some(0));
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(report.contains("Monitor: Complete | Job: Succeeded | Probe: exit 0"));
    assert!(!report.contains("Attempt: Idle"));
    assert!(!report.contains("Detail: probe exit 0"));
}

fn start_pending_watch(store: &Path, extra: &[&str]) -> (Child, String) {
    let mut arguments = vec!["watch", "--store-dir", store.to_str().unwrap()];
    arguments.extend_from_slice(extra);
    arguments.extend(["--deadline", "10s", "--sample-every", "1s"]);
    arguments.extend(["--", "sh", "-c", "echo pending; exit 75"]);
    let mut child = Command::new(binary())
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut announcement = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut announcement)
        .unwrap();
    let id = announcement
        .trim()
        .strip_prefix("PIRA watch live | result=")
        .expect("watch ID announcement")
        .to_string();
    assert!(
        store
            .join("watch/state")
            .join(format!("{id}.json"))
            .is_file()
    );
    wait_for(store, &id, "\"job\":\"pending\"");
    (child, id)
}

fn wait_for(store: &Path, id: &str, needle: &str) {
    let path = store.join("watch/state").join(format!("{id}.json"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if fs::read_to_string(&path).is_ok_and(|value| value.contains(needle)) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("watch state did not contain {needle}");
}

#[test]
fn active_watch_announces_id_supports_latest_and_acknowledges_stop() {
    let sandbox = Sandbox::new("watch-active");
    let (mut owner, id) = start_pending_watch(sandbox.path(), &[]);

    let latest = run(sandbox.path(), &["watch", &id, "--latest"]);
    assert_eq!(latest.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&latest.stdout).contains("Job: Pending"));

    let invalid = run(sandbox.path(), &["watch", &id, "--no-progress-after", "1s"]);
    assert_eq!(invalid.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("effective analyzer"));

    let listed = run(sandbox.path(), &["list", "--limit", "5"]);
    let listing = String::from_utf8_lossy(&listed.stdout);
    assert!(listing.contains("id | kind | state"));
    assert!(listing.contains(&format!("{id} | watch | active")));

    let stopped = run(sandbox.path(), &["watch", &id, "--stop"]);
    assert_eq!(stopped.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("stopped"));
    assert_eq!(owner.wait().unwrap().code(), Some(23));
}

#[test]
fn paused_watch_lists_as_paused_and_latest_still_succeeds() {
    let sandbox = Sandbox::new("watch-paused");
    let (mut owner, id) = start_pending_watch(sandbox.path(), &["--review-after", "1s"]);
    assert_eq!(owner.wait().unwrap().code(), Some(10));
    wait_for(sandbox.path(), &id, "\"monitor\":\"paused\"");

    let listed = run(sandbox.path(), &["list", "--limit", "5"]);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(&format!("{id} | watch | paused")));
    assert_eq!(
        run(sandbox.path(), &["watch", &id, "--latest"])
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn first_analyzer_replacement_is_applied() {
    let sandbox = Sandbox::new("watch-first-analyzer-update");
    let first = "import json,sys; json.load(sys.stdin); print(json.dumps({'progress':'first'}))";
    let second = "import json,sys; json.load(sys.stdin); print(json.dumps({'progress':'second'}))";
    let (mut owner, id) = start_pending_watch(
        sandbox.path(),
        &["--analyzer-code", first, "--review-after", "2500ms"],
    );
    let update = run(
        sandbox.path(),
        &["watch", &id, "--set-analyzer-code", second],
    );
    assert_eq!(update.status.code(), Some(0));
    assert_eq!(owner.wait().unwrap().code(), Some(10));
    let latest = run(sandbox.path(), &["watch", &id, "--latest"]);
    let report = String::from_utf8_lossy(&latest.stdout);
    assert!(report.contains("analyzer revision: 2"));
    assert!(report.contains("second"));
}

#[test]
fn stopping_paused_watch_is_direct_and_terminal_stop_is_noop() {
    let sandbox = Sandbox::new("watch-ownerless-stop");
    let (mut owner, id) = start_pending_watch(sandbox.path(), &["--review-after", "1s"]);
    assert_eq!(owner.wait().unwrap().code(), Some(10));

    let stopped = run(sandbox.path(), &["watch", &id, "--stop"]);
    assert_eq!(stopped.status.code(), Some(0));
    let latest = run(sandbox.path(), &["watch", &id, "--latest"]);
    assert!(String::from_utf8_lossy(&latest.stdout).contains("Monitor: Stopped"));
    let again = run(sandbox.path(), &["watch", &id, "--stop"]);
    assert_eq!(again.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&again.stdout).contains("state unchanged"));
}

#[test]
fn capture_announces_a_discoverable_id_before_completion() {
    let sandbox = Sandbox::new("capture-live");
    let mut child = Command::new(binary())
        .args([
            "capture",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--intent",
            "Test live ID",
            "--",
            "sh",
            "-c",
            "sleep 1; echo done",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut announcement = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut announcement)
        .unwrap();
    let id = announcement
        .trim()
        .strip_prefix("LIVE | result=")
        .expect("capture live ID announcement");
    assert!(
        sandbox
            .path()
            .join("live")
            .join(format!("{id}.live.json"))
            .is_file()
    );
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(sandbox.path().join(format!("{id}.piractx")).is_file());
}

#[test]
fn clearing_analyzer_clears_no_progress_threshold() {
    let sandbox = Sandbox::new("watch-clear-analyzer");
    let analyzer = "import json,sys; json.load(sys.stdin); print(json.dumps({'progress':'same'}))";
    let (mut owner, id) = start_pending_watch(
        sandbox.path(),
        &["--analyzer-code", analyzer, "--no-progress-after", "5s"],
    );
    let cleared = run(sandbox.path(), &["watch", &id, "--clear-analyzer"]);
    assert_eq!(cleared.status.code(), Some(0));
    let deadline = Instant::now() + Duration::from_secs(3);
    let state_path = sandbox
        .path()
        .join("watch/state")
        .join(format!("{id}.json"));
    while Instant::now() < deadline {
        if fs::read_to_string(&state_path).is_ok_and(|value| {
            value.contains("\"analyzer\":null") && value.contains("\"no_progress_after_ms\":null")
        }) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let state = fs::read_to_string(state_path).unwrap();
    assert!(state.contains("\"analyzer\":null"));
    assert!(state.contains("\"no_progress_after_ms\":null"));
    assert_eq!(
        run(sandbox.path(), &["watch", &id, "--stop"]).status.code(),
        Some(0)
    );
    assert_eq!(owner.wait().unwrap().code(), Some(23));
}

#[test]
fn current_selects_exactly_one_live_capture_in_detected_thread() {
    let sandbox = Sandbox::new("watch-current");
    let thread_id = format!("watch-current-{}", std::process::id());
    let mut capture = Command::new(binary())
        .env("PIRA_CTX_THREAD_ID", &thread_id)
        .args([
            "capture",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--intent",
            "Test current capture",
            "--",
            "sh",
            "-c",
            "sleep 2; echo done",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut announcement = String::new();
    BufReader::new(capture.stderr.take().unwrap())
        .read_line(&mut announcement)
        .unwrap();
    let capture_id = announcement.trim().strip_prefix("LIVE | result=").unwrap();

    let mut watch = Command::new(binary())
        .env("PIRA_CTX_THREAD_ID", &thread_id)
        .args([
            "watch",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--current",
            "--deadline",
            "5s",
            "--sample-every",
            "100ms",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut watch_announcement = String::new();
    BufReader::new(watch.stderr.take().unwrap())
        .read_line(&mut watch_announcement)
        .unwrap();
    assert!(watch_announcement.starts_with("PIRA watch live | result="));
    assert_eq!(capture.wait().unwrap().code(), Some(0));
    assert_eq!(watch.wait().unwrap().code(), Some(0));
    assert!(
        sandbox
            .path()
            .join(format!("{capture_id}.piractx"))
            .is_file()
    );
}

#[test]
fn current_rejects_zero_live_captures() {
    let sandbox = Sandbox::new("watch-current-none");
    let output = Command::new(binary())
        .env("PIRA_CTX_THREAD_ID", "watch-current-none")
        .args([
            "watch",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--current",
            "--deadline",
            "2s",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no live capture"));
}

#[test]
fn current_rejects_multiple_live_captures_and_names_candidates() {
    let sandbox = Sandbox::new("watch-current-many");
    let thread_id = format!("watch-current-many-{}", std::process::id());
    let mut captures = Vec::new();
    let mut ids = Vec::new();
    for index in 0..2 {
        let mut child = Command::new(binary())
            .env("PIRA_CTX_THREAD_ID", &thread_id)
            .args([
                "capture",
                "--store-dir",
                sandbox.path().to_str().unwrap(),
                "--intent",
                &format!("Test current candidate {index}"),
                "--",
                "sh",
                "-c",
                "sleep 2; echo done",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut announcement = String::new();
        BufReader::new(child.stderr.take().unwrap())
            .read_line(&mut announcement)
            .unwrap();
        ids.push(
            announcement
                .trim()
                .strip_prefix("LIVE | result=")
                .unwrap()
                .to_string(),
        );
        captures.push(child);
    }
    let output = Command::new(binary())
        .env("PIRA_CTX_THREAD_ID", &thread_id)
        .args([
            "watch",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--current",
            "--deadline",
            "2s",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(125));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("multiple live captures"));
    assert!(ids.iter().all(|id| error.contains(id)));
    for mut child in captures {
        assert_eq!(child.wait().unwrap().code(), Some(0));
    }
}
