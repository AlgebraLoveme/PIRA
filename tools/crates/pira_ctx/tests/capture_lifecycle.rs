use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

#[cfg(windows)]
fn python() -> &'static str {
    "python"
}

#[cfg(not(windows))]
fn python() -> &'static str {
    "python3"
}

fn sleep_command() -> Vec<&'static str> {
    #[cfg(windows)]
    {
        vec![
            "python",
            "-c",
            "import time; time.sleep(2); print('done', flush=True)",
        ]
    }
    #[cfg(not(windows))]
    {
        vec![
            python(),
            "-c",
            "import time; time.sleep(2); print('done', flush=True)",
        ]
    }
}

fn spawn_capture(store: &Path, mode: &str) -> Child {
    let mut arguments = vec![
        mode,
        "--store-dir",
        store.to_str().unwrap(),
        "--intent",
        "Test live capture lifecycle",
        "--",
    ];
    arguments.extend(sleep_command());
    Command::new(binary())
        .env("PIRA_CTX_LIVE_CHECKPOINT_MS", "100")
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn wait_for_manifest(store: &Path) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(entries) = fs::read_dir(store.join("live"))
            && let Some(path) = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        {
            return path;
        }
        assert!(Instant::now() < deadline, "live manifest was not published");
        thread::sleep(Duration::from_millis(20));
    }
}

fn list(store: &Path) -> String {
    let output = Command::new(binary())
        .args(["list", "--store-dir", store.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    String::from_utf8(output.stdout).unwrap()
}

fn full_id(store: &Path, handle: &str) -> String {
    let output = Command::new(binary())
        .args(["stats", "--store-dir", store.to_str().unwrap(), handle])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Result: "))
        .expect("full stored ID")
        .to_string()
}

#[test]
fn announced_capture_is_running_and_releases_owner_file() {
    let sandbox = Sandbox::new("announced-capture");
    let mut child = spawn_capture(sandbox.path(), "capture");
    let mut announcement = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut announcement)
        .unwrap();
    let id = announcement
        .trim()
        .strip_prefix("LIVE | result=")
        .expect("capture live ID announcement");
    let full = full_id(sandbox.path(), id);
    let id = full.as_str();
    assert!(list(sandbox.path()).contains(&format!("{id} | capture | running |")));
    assert!(
        sandbox
            .path()
            .join("live/owners")
            .join(format!("{id}.lock"))
            .is_file()
    );
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(sandbox.path().join(format!("{id}.piractx")).is_file());
    assert!(
        !sandbox
            .path()
            .join("live/owners")
            .join(format!("{id}.lock"))
            .exists()
    );
}

#[test]
fn quick_check_prints_no_live_announcement_and_one_result_id() {
    let sandbox = Sandbox::new("quick-check");
    let output = Command::new(binary())
        .args([
            "check",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--intent",
            "Test compact lifecycle output",
            "--",
            python(),
            "-c",
            "print('ok')",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("LIVE | result="));
    assert_eq!(stdout.matches("result=").count(), 1);
}

#[test]
fn cancel_terminates_capture_and_records_cancelled_state() {
    let sandbox = Sandbox::new("cancel-capture");
    let mut child = spawn_capture(sandbox.path(), "capture");
    let mut announcement = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut announcement)
        .unwrap();
    let id = announcement
        .trim()
        .strip_prefix("LIVE | result=")
        .expect("capture live ID announcement");
    let full = full_id(sandbox.path(), id);
    let cancel = Command::new(binary())
        .args([
            "cancel",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            id,
        ])
        .output()
        .unwrap();
    assert_eq!(cancel.status.code(), Some(0));
    let id = full.as_str();
    assert!(String::from_utf8(cancel.stdout).unwrap().contains(id));
    let completed = child.wait_with_output().unwrap();
    assert!(!completed.status.success());
    assert!(
        String::from_utf8(completed.stdout)
            .unwrap()
            .contains("state=cancelled")
    );
    assert!(list(sandbox.path()).contains(&format!("{id} | capture | cancelled |")));
    assert!(
        !sandbox
            .path()
            .join("live")
            .join(format!("{id}.cancel"))
            .exists()
    );
}

#[test]
fn cancelled_check_remains_status_only() {
    let sandbox = Sandbox::new("cancel-check");
    let mut child = spawn_capture(sandbox.path(), "check");
    let mut announcement = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut announcement)
        .unwrap();
    let id = announcement
        .trim()
        .strip_prefix("LIVE | result=")
        .expect("check live ID announcement");
    let cancel = Command::new(binary())
        .args([
            "cancel",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            id,
        ])
        .output()
        .unwrap();
    assert!(cancel.status.success());
    let completed = child.wait_with_output().unwrap();
    assert!(!completed.status.success());
    let text = String::from_utf8(completed.stdout).unwrap();
    assert!(text.starts_with("CANCELLED |"), "{text}");
    assert_eq!(text.lines().count(), 1);
    assert!(!text.contains("PROGRAM data:"));
}

#[test]
fn automatic_checkpoint_keeps_owner_lease_when_snapshot_is_old() {
    let sandbox = Sandbox::new("automatic-checkpoint");
    let mut child = spawn_capture(sandbox.path(), "auto");
    let manifest = wait_for_manifest(sandbox.path());
    let id = manifest
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix(".live.json")
        .unwrap()
        .to_string();
    let mut stored: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(stored["owner_lock"], true);
    stored["checkpoint_unix_ms"] = serde_json::json!(1);
    fs::write(&manifest, serde_json::to_vec(&stored).unwrap()).unwrap();
    assert!(list(sandbox.path()).contains(&format!("{id} | capture | running |")));
    let cancel = Command::new(binary())
        .args([
            "cancel",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            &id,
        ])
        .output()
        .unwrap();
    assert!(
        cancel.status.success(),
        "{}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    assert!(!child.wait().unwrap().success());
    assert!(!manifest.exists());
}

#[test]
fn large_live_index_is_bounded_disclosed_and_final_capture_remains_complete() {
    use std::io::Write;
    let s = Sandbox::new("large-live-index");
    let mut child = Command::new(binary())
        .env("PIRA_CTX_LIVE_CHECKPOINT_MS", "100")
        .args([
            "check",
            "--store-dir",
            s.path().to_str().unwrap(),
            "--intent",
            "Inspect high-volume live output",
            "--",
            python(),
            "-c",
            r"import sys; sys.stdout.write('x\n'*400000); sys.stdout.flush(); sys.stdin.read(1)",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let manifest = wait_for_manifest(s.path());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let snapshot = loop {
        let data = fs::read(&manifest).unwrap();
        assert!(data.len() <= 16 * 1024 * 1024);
        let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
        if v["metadata"]["stdout_bytes"] == 800000 {
            break v;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live checkpoint did not advance"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(snapshot["owner_lock"], true);
    assert_eq!(snapshot["metadata"]["timeline_truncated"], true);
    let id = snapshot["metadata"]["result_id"].as_str().unwrap();
    let search = Command::new(binary())
        .args(["search", "--store-dir", s.path().to_str().unwrap(), id, "x"])
        .output()
        .unwrap();
    assert!(search.status.success());
    let text = String::from_utf8(search.stdout).unwrap();
    assert!(
        text.contains("Index: truncated; search covered only the indexed retained prefix"),
        "{text}"
    );
    let plan = s.path().join("count.json");
    fs::write(&plan, r#"{"steps":[{"op":"count"}]}"#).unwrap();
    for options in [vec!["--count"], vec!["--plan", plan.to_str().unwrap()]] {
        let result = Command::new(binary())
            .args(["transform", "--store-dir", s.path().to_str().unwrap(), id])
            .args(options)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(125));
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("requires a complete line index"));
    }
    let raw = Command::new(binary())
        .args([
            "raw",
            "--store-dir",
            s.path().to_str().unwrap(),
            id,
            "--stdout",
        ])
        .output()
        .unwrap();
    assert!(raw.status.success());
    assert_eq!(raw.stdout, b"x\n".repeat(400000));
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    let count = Command::new(binary())
        .args([
            "transform",
            "--store-dir",
            s.path().to_str().unwrap(),
            id,
            "--count",
        ])
        .output()
        .unwrap();
    assert!(count.status.success());
    assert_eq!(String::from_utf8(count.stdout).unwrap().trim(), "400000");
    let stats = Command::new(binary())
        .args(["stats", "--store-dir", s.path().to_str().unwrap(), id])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&stats.stdout).contains("indexed_lines=400000 truncated=false")
    );
}

#[test]
fn checkpoint_publication_failure_is_warned_without_changing_child_status() {
    let s = Sandbox::new("checkpoint-failure");
    let child = Command::new(binary())
        .env("PIRA_CTX_LIVE_CHECKPOINT_MS", "500")
        .args([
            "check",
            "--store-dir",
            s.path().to_str().unwrap(),
            "--intent",
            "Check checkpoint failure diagnostics",
            "--",
            python(),
            "-c",
            "import time; print('ready',flush=True); time.sleep(1.2)",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let manifest = wait_for_manifest(s.path());
    let id = manifest
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix(".live.json")
        .unwrap();
    fs::create_dir(
        s.path()
            .join("live")
            .join(format!(".{id}.{}.tmp", child.id())),
    )
    .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    let text = String::from_utf8(result.stderr).unwrap();
    assert_eq!(
        text.matches("live checkpointing stopped").count(),
        1,
        "{text}"
    );
}

#[test]
fn cancelled_short_exact_retains_partial_output_and_live_id() {
    for redirected_stdout in [false, true] {
        let s = Sandbox::new("cancel-exact");
        let data = s.path().join("data.bin");
        let mut command = Command::new(binary());
        command.env("PIRA_CTX_LIVE_CHECKPOINT_MS", "100")
            .args(["exact", "--store-dir", s.path().to_str().unwrap(), "--intent", "Cancel short exact output", "--", python(), "-c",
                "import sys,time; print('out',flush=True); print('err',file=sys.stderr,flush=True); time.sleep(10)"])
            .stderr(Stdio::piped());
        if redirected_stdout {
            command.stdout(Stdio::from(fs::File::create(&data).unwrap()));
        } else {
            command.stdout(Stdio::piped());
        }
        let child = command.spawn().unwrap();
        let manifest = wait_for_manifest(s.path());
        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        let id = snapshot["metadata"]["result_id"].as_str().unwrap();
        let cancel = Command::new(binary())
            .args(["cancel", "--store-dir", s.path().to_str().unwrap(), id])
            .output()
            .unwrap();
        assert!(
            cancel.status.success(),
            "{}",
            String::from_utf8_lossy(&cancel.stderr)
        );
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        let report = if redirected_stdout {
            &result.stderr
        } else {
            &result.stdout
        };
        assert!(
            String::from_utf8_lossy(report)
                .contains("Cancelled command: partial captured output retained.")
        );
        let raw = Command::new(binary())
            .args([
                "raw",
                "--store-dir",
                s.path().to_str().unwrap(),
                id,
                "--stderr",
            ])
            .output()
            .unwrap();
        assert!(
            raw.status.success(),
            "{}",
            String::from_utf8_lossy(&raw.stderr)
        );
        assert_eq!(raw.stdout, b"err\n");
        assert!(list(s.path()).contains("cancelled"));
        if redirected_stdout {
            assert_eq!(fs::read(&data).unwrap(), b"out\n");
        }
    }
}
