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
            "python3",
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
        .strip_prefix("PIRA live | result=")
        .expect("capture live ID announcement");
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
fn automatic_checkpoint_without_owner_lease_is_running_while_fresh() {
    let sandbox = Sandbox::new("automatic-checkpoint");
    let mut child = spawn_capture(sandbox.path(), "auto");
    let manifest = wait_for_manifest(sandbox.path());
    let id = manifest
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix(".live.json")
        .unwrap();
    assert!(list(sandbox.path()).contains(&format!("{id} | capture | running |")));
    assert!(
        !sandbox
            .path()
            .join("live/owners")
            .join(format!("{id}.lock"))
            .exists()
    );
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(!manifest.exists());
}
