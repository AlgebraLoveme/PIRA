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
fn check_prints_one_result_id_and_announces_only_after_delay() {
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
    assert_eq!(stdout.matches("result=").count(), 1);
    let duration_ms: u128 = stdout
        .split("duration=")
        .nth(1)
        .unwrap()
        .split("ms")
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let final_id = stdout.split("result=").nth(1).unwrap().trim();
    let announcements: Vec<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("LIVE | result="))
        .collect();
    // Python startup and CI scheduling are not guaranteed to finish within 250 ms.
    // Do not weaken the real contract: below the delay there must be no LIVE;
    // a slower execution may announce exactly once, with the same durable identity.
    if duration_ms < 250 {
        assert!(announcements.is_empty(), "{stderr}");
    }
    assert!(announcements.len() <= 1, "{stderr}");
    if let Some(id) = announcements.first() {
        assert_eq!(*id, final_id);
    }
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
    for args in [
        vec!["stats", id],
        vec!["stats", "--brief", id],
        vec!["exec", id, "--code", "print(MSG_STATE)"],
    ] {
        let output = Command::new(binary())
            .args(&args)
            .args(["--store-dir", sandbox.path().to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("cancelled"));
    }

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
            r"import sys; sys.stdout.buffer.write(b'x\n'*400000); sys.stdout.buffer.flush(); sys.stdin.read(1)",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let manifest = wait_for_manifest(s.path());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let snapshot = loop {
        let data = fs::read(&manifest).unwrap_or_else(|error| {
            let status = child.try_wait();
            let live_entries = fs::read_dir(s.path().join("live")).map(|entries| {
                entries.map(|entry| entry.map(|entry| entry.file_name())).collect::<Vec<_>>()
            });
            drop(child.stdin.take());
            let _ = child.kill();
            let mut stdout = String::new();
            let mut stderr = String::new();
            use std::io::Read;
            if let Some(mut pipe) = child.stdout.take() { let _ = pipe.read_to_string(&mut stdout); }
            if let Some(mut pipe) = child.stderr.take() { let _ = pipe.read_to_string(&mut stderr); }
            let _ = child.wait();
            panic!("live read failed: {error:?}; pre-cleanup status={status:?}; live entries={live_entries:?}; stdout={stdout:?}; stderr={stderr:?}");
        });
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
                r"import sys,time; sys.stdout.buffer.write(b'out\n'); sys.stdout.buffer.flush(); sys.stderr.buffer.write(b'err\n'); sys.stderr.buffer.flush(); time.sleep(10)"])
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

#[cfg(unix)]
#[test]
fn detached_pipe_holder_cannot_block_exit_or_cancellation() {
    let sandbox = Sandbox::new("detached-pipe");
    let script = r#"
import json, os, pathlib, signal, subprocess, sys, time
binary, root = sys.argv[1:]
for cancel in (False, True):
    store = pathlib.Path(root) / str(cancel)
    store.mkdir()
    ready = store / 'detached.pid'
    code = r"""
import os, pathlib, sys, time
r,w=os.pipe()
pid=os.fork()
if pid == 0:
    os.close(r)
    os.setsid()
    os.write(w,b'ready');os.close(w)
    time.sleep(8)
    os._exit(0)
os.close(w);os.read(r,5);os.close(r)
print('retained-before-exit',flush=True)
pathlib.Path(sys.argv[1]).write_text(str(pid))
if sys.argv[2]=='True':time.sleep(8)
"""
    proc = subprocess.Popen([binary,'capture','--store-dir',str(store),'--intent','Bounded detached pipe fixture','--',sys.executable,'-c',code,str(ready),str(cancel)],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
        if cancel:
            until=time.monotonic()+4
            while not ready.exists():
                assert time.monotonic()<until, 'detached fixture did not start'
                time.sleep(.01)
            manifest=next((store/'live').glob('*.live.json'))
            result=json.loads(manifest.read_text())['metadata']['result_id']
            subprocess.run([binary,'cancel','--store-dir',str(store),result],check=True,capture_output=True,timeout=3)
        out,err=proc.communicate(timeout=4)
        assert (proc.returncode != 0) == cancel, (proc.returncode,out,err)
        assert b'drain expired' in err, err
        raw=subprocess.run([binary,'raw','--store-dir',str(store),'--last','--stdout'],check=True,capture_output=True,timeout=3)
        assert b'retained-before-exit' in raw.stdout, raw
        stats=subprocess.run([binary,'stats','--store-dir',str(store),'--last'],check=True,capture_output=True,timeout=3)
        assert b'truncat' in stats.stdout.lower(), stats.stdout
    finally:
        if proc.poll() is None:proc.kill()
        proc.communicate(timeout=3)
        if ready.exists():
            try:os.kill(int(ready.read_text()),signal.SIGKILL)
            except ProcessLookupError:pass
"#;
    let output = Command::new(python())
        .args(["-c", script, binary(), sandbox.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn replacement_character_workspace_keeps_native_identity_live_and_final() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let sandbox = Sandbox::new("native-workspace");
    let workspace = sandbox.path().join("workspace-�");
    fs::create_dir(&workspace).unwrap();
    let store = sandbox.path().join("store");
    let mut child = Command::new(binary())
        .current_dir(&workspace)
        .args([
            "capture",
            "--store-dir",
            store.to_str().unwrap(),
            "--intent",
            "Native workspace fixture",
            "--",
            python(),
            "-c",
            "import sys; print('kept',flush=True); sys.stdin.read(1)",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let manifest = wait_for_manifest(&store);
    let live: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    let hash = live["metadata"]["workspace_hash"].as_str().unwrap();
    assert!(hash.starts_with("native-v1-"));
    let id = live["metadata"]["result_id"].as_str().unwrap();
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let catalog = fs::read_to_string(store.join("indexes").join(format!("{hash}.jsonl"))).unwrap();
    assert!(catalog.contains(id));
    let stats = Command::new(binary())
        .current_dir(&workspace)
        .args(["stats", "--store-dir", store.to_str().unwrap(), "--last"])
        .output()
        .unwrap();
    assert!(stats.status.success());
    assert!(String::from_utf8_lossy(&stats.stdout).contains(id));
    let root = workspace.canonicalize().unwrap();
    let legacy: String = Sha256::digest(root.to_string_lossy().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let old_index = store
        .join("indexes")
        .join(format!("{}.jsonl", &legacy[..16]));
    fs::write(&old_index, b"legacy ownership unchanged").unwrap();
    let refused = Command::new(binary())
        .current_dir(&workspace)
        .args(["stats", "--store-dir", store.to_str().unwrap(), "--last"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("ambiguous legacy workspace"));
    assert_eq!(fs::read(old_index).unwrap(), b"legacy ownership unchanged");
    assert!(store.join(format!("{id}.piractx")).exists());
}

#[test]
fn live_retention_loss_updates_observation_and_watch_uncertainty() {
    let sandbox = Sandbox::new("live-retention");
    let script = r#"
import json, os, pathlib, subprocess, sys, time
binary, root = sys.argv[1:]
store = pathlib.Path(root) / 'store'
code = """
import os, sys
os.write(1, b'a'*2048)
os.write(2, b'b'*2048)
sys.stdin.buffer.read(1)
os.write(1, b'LOST_STDOUT')
sys.stdin.buffer.read(1)
os.write(2, b'LOST_STDERR')
sys.stdin.buffer.read(1)
"""
env = dict(os.environ, PIRA_CTX_MAX_RETAINED_BYTES='4096', PIRA_CTX_LIVE_CHECKPOINT_MS='100')
p = subprocess.Popen([binary, 'capture', '--store-dir', str(store), '--intent', 'Live retention fixture', '--', sys.executable, '-c', code], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
def snapshot(observed):
    deadline = time.monotonic()+10
    while time.monotonic() < deadline:
        for path in (store/'live').glob('*.live.json'):
            data = json.loads(read_shared_snapshot(path))
            if data['metadata']['observed_total_bytes'] == observed:
                return data
        assert p.poll() is None, 'capture exited before checkpoint'
        time.sleep(.02)
    raise AssertionError('live observed count did not reach '+str(observed))
def run(*args):
    return subprocess.run([binary, args[0], '--store-dir', str(store), *args[1:]], capture_output=True, timeout=10)
failed = False
try:
    first = snapshot(4096)
    md = first['metadata']; result = md['result_id']
    assert not md['retention_truncated'] and not md['timeline_truncated'], md
    assert md['stdout_bytes'] == md['stderr_bytes'] == 2048, md
    for out, err in ((2048+len(b'LOST_STDOUT'), 2048), (2048+len(b'LOST_STDOUT'), 2048+len(b'LOST_STDERR'))):
        p.stdin.write(b'x'); p.stdin.flush()
        current = snapshot(out+err); md = current['metadata']
        assert current['generation'] > first['generation'], current
        first = current
        assert md['observed_stdout_bytes'] == out and md['observed_stderr_bytes'] == err, md
        assert md['total_bytes'] == 4096 and md['retention_truncated'] and md['timeline_truncated'], md
    search = run('search', result, 'LOST_STDOUT')
    assert search.returncode == 0 and b'0 hits' in search.stdout and b'retention_truncated=1' in search.stdout, search
    watch = run('watch', '--capture', result, '--deadline', '4s', '--sample-every', '100ms', '--unchanged-after', '1s')
    assert watch.returncode == 10 and b'render reliable: false' in watch.stdout and b'rendered state is unreliable' in watch.stdout, watch
    assert b'Attention: visible output unchanged' not in watch.stdout, watch
    p.stdin.close(); p.stdin = None
    report, errors = p.communicate(timeout=10)
    assert p.returncode == 0, (report, errors)
    for stream, expected in (('--stdout', b'a'*2048), ('--stderr', b'b'*2048)):
        raw = run('raw', result, stream)
        assert raw.returncode == 0 and raw.stdout == expected, raw
    assert run('verify', result).returncode == 0
except BaseException:
    failed = True
    report_live_failure(p, store)
    raise
finally:
    if p.poll() is None: p.kill()
    output, errors = p.communicate(timeout=10)
    if failed: print(f'capture cleanup rc={p.returncode} stdout={output!r} stderr={errors!r}', file=sys.stderr)
"#;
    let output = Command::new(python())
        .args([
            "-c",
            &format!("{}\n{script}", include_str!("shared_snapshot.py")),
            binary(),
            sandbox.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn saturated_live_writes_never_claim_inactivity() {
    let sandbox = Sandbox::new("saturated-activity");
    let script = r#"
import json, os, pathlib, subprocess, sys, time
binary, root = sys.argv[1:]
store = pathlib.Path(root) / 'store'
code = """
import sys, threading, time
sys.stdout.buffer.write(b'a'*4096); sys.stdout.buffer.flush()
stop = threading.Event()
def write():
    while not stop.wait(.05):
        sys.stdout.buffer.write(b'b'*100); sys.stdout.buffer.flush()
writer = threading.Thread(target=write)
writer.start()
sys.stdin.buffer.read(1)
stop.set(); writer.join()
"""
env = dict(os.environ, PIRA_CTX_MAX_RETAINED_BYTES='4096', PIRA_CTX_LIVE_CHECKPOINT_MS='100')
p = subprocess.Popen([binary, 'capture', '--store-dir', str(store), '--intent', 'Saturation activity fixture', '--', sys.executable, '-c', code], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
failed = False
try:
    deadline = time.monotonic()+10
    while True:
        manifests = list((store/'live').glob('*.live.json'))
        if manifests:
            md = json.loads(read_shared_snapshot(manifests[0]))['metadata']
            if md['retention_truncated']: break
        assert p.poll() is None and time.monotonic()<deadline, 'no saturated live checkpoint'
        time.sleep(.02)
    before = md['observed_total_bytes']
    watch = subprocess.run([binary, 'watch', '--store-dir', str(store), '--capture', md['result_id'], '--deadline', '8s', '--review-after', '2s', '--sample-every', '100ms', '--inactive-after', '1s', '--attention', 'cache'], capture_output=True, timeout=12)
    after = json.loads(read_shared_snapshot(manifests[0]))['metadata']
    assert p.poll() is None, 'capture must still be running'
    assert md['total_bytes'] == after['total_bytes'] == 4096
    assert after['observed_total_bytes'] > before, (before, after)
    assert watch.returncode == 10, watch
    assert b'no raw activity observed' not in watch.stdout, watch.stdout
    assert b'sample output is incomplete' in watch.stdout, watch.stdout
except BaseException:
    failed = True
    report_live_failure(p, store)
    raise
finally:
    if p.poll() is None:
        p.stdin.close(); p.stdin = None
    try: output, errors = p.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        p.kill(); output, errors = p.communicate(timeout=10)
    if failed: print(f'capture cleanup rc={p.returncode} stdout={output!r} stderr={errors!r}', file=sys.stderr)
"#;
    let output = Command::new(python())
        .args([
            "-c",
            &format!("{}\n{script}", include_str!("shared_snapshot.py")),
            binary(),
            sandbox.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shared_snapshot_reader_preserves_atomic_replacement_and_errors() {
    let sandbox = Sandbox::new("shared-snapshot");
    let script = r#"
import errno, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
path = root / 'snapshot.json'
next_path = root / 'next.json'
path.write_text('{"generation": 1}', encoding='utf-8')
if os.name == 'nt':
    import ctypes
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    create = kernel.CreateFileW
    create.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                       wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    create.restype = wintypes.HANDLE
    close = kernel.CloseHandle
    close.argtypes = [wintypes.HANDLE]
    close.restype = wintypes.BOOL
    # Deterministically expose Python's incompatible open while DELETE is held.
    deleting = create(str(path), 0x10000, 7, None, 3, 0x80, None)
    assert deleting != ctypes.c_void_p(-1).value, ctypes.get_last_error()
    try:
        try: path.read_text(encoding='utf-8')
        except PermissionError as error: assert error.errno == errno.EACCES, error
        else: raise AssertionError('ordinary Python reader unexpectedly shared DELETE')
        assert json.loads(read_shared_snapshot(path)) == {'generation': 1}
    finally:
        assert close(deleting)
    # Genuine incompatible ownership is not retried, ignored or turned into a snapshot.
    exclusive = create(str(path), 0x80000000, 0, None, 3, 0x80, None)
    assert exclusive != ctypes.c_void_p(-1).value, ctypes.get_last_error()
    try:
        try: read_shared_snapshot(path)
        except OSError as error: assert error.winerror == 32, error
        else: raise AssertionError('incompatible sharing must fail')
    finally:
        assert close(exclusive)
with open_shared_snapshot(path) as previous:
    next_path.write_text('{"generation": 2}', encoding='utf-8')
    if os.name == 'nt':
        # Match Ctx's single namespace operation, not ReplaceFileW or os.replace.
        encoded = str(path.resolve()).encode('utf-16-le')
        class RenameInfo(ctypes.Structure):
            _fields_ = [('Flags', wintypes.DWORD), ('RootDirectory', wintypes.HANDLE),
                        ('FileNameLength', wintypes.DWORD), ('FileName', ctypes.c_uint16 * (len(encoded)//2+1))]
        info = RenameInfo()
        info.Flags = 3  # REPLACE_IF_EXISTS | POSIX_SEMANTICS
        info.FileNameLength = len(encoded)
        ctypes.memmove(ctypes.addressof(info) + RenameInfo.FileName.offset, encoded, len(encoded))
        source = create(str(next_path), 0x10000, 7, None, 3, 0x80, None)
        assert source != ctypes.c_void_p(-1).value, ctypes.get_last_error()
        rename = kernel.SetFileInformationByHandle
        rename.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
        rename.restype = wintypes.BOOL
        try:
            if not rename(source, 22, ctypes.byref(info), ctypes.sizeof(info)):
                raise ctypes.WinError(ctypes.get_last_error())
        finally:
            assert close(source)
    else:
        os.replace(next_path, path)
    assert json.load(previous) == {'generation': 1}
assert json.loads(read_shared_snapshot(path)) == {'generation': 2}
try: read_shared_snapshot(root / 'absent.json')
except FileNotFoundError: pass
else: raise AssertionError('missing snapshot must fail')
path.write_text('invalid JSON', encoding='utf-8')
try: json.loads(read_shared_snapshot(path))
except json.JSONDecodeError: pass
else: raise AssertionError('corrupt snapshot must fail')
"#;
    let output = Command::new(python())
        .args([
            "-c",
            &format!("{}\n{script}", include_str!("shared_snapshot.py")),
            sandbox.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
