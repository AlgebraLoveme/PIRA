use super::*;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

// No copied executable, shell descendant, credential, network peer or model.
struct Peer {
    child: Child,
    root: PathBuf,
}
impl Peer {
    fn start(mode: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "team-ssh-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::private_dir(&root).unwrap();
        let script = r#"
import os,sys,time
from pathlib import Path
mode,ready=sys.argv[1:]
if mode == 'write': sys.stdin.buffer.read(1)
else: sys.stdin.buffer.read()
if mode == 'reply': sys.stdout.buffer.write(b'fixture reply')
elif mode == 'limit': sys.stdout.buffer.write(b'x'*(16*1024*1024+1))
elif mode == 'failed': sys.exit(7)
else:
    if mode == 'wait': os.close(1)
    Path(ready).write_text('ready')
    time.sleep(3) # bounded natural-exit fail-safe if cancellation regresses
"#;
        let mut command = Command::new(if cfg!(windows) { "python" } else { "python3" });
        command
            .args(["-B", "-c", script, mode])
            .arg(root.join("ready"))
            .env_clear();
        for name in ["PATH", "SYSTEMROOT", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let child = command
            .env("HOME", &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self { child, root }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn ssh_cancel_during_blocked_write_reaps_only_owned_peer() {
    let mut peer = Peer::start("write");
    let ready = peer.root.join("ready");
    let request = vec![b'x'; 2 * 1024 * 1024];
    let start = Instant::now();
    let result = exchange(&mut peer.child, &request, || ready.exists()).unwrap_err();
    assert!(result.contains("cancelled during write"), "{result}");
    assert!(peer.child.try_wait().unwrap().is_some());
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn ssh_cancel_during_blocked_read_reaps_only_owned_peer() {
    let mut peer = Peer::start("read");
    let ready = peer.root.join("ready");
    let start = Instant::now();
    let result = exchange(&mut peer.child, b"fixture input", || ready.exists()).unwrap_err();
    assert!(result.contains("cancelled during read"), "{result}");
    assert!(peer.child.try_wait().unwrap().is_some());
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn ssh_cancel_during_child_wait_is_observed() {
    let mut peer = Peer::start("wait");
    peer.child.stdin.take();
    let ready = peer.root.join("ready");
    let start = Instant::now();
    let error = wait(&mut peer.child, &|| ready.exists()).unwrap_err();
    assert!(error.to_string().contains("cancelled during wait"));
    peer.child.kill().unwrap();
    peer.child.wait().unwrap();
    assert!(peer.child.try_wait().unwrap().is_some());
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn ssh_reply_and_exit_preserve_complete_bounded_output() {
    let mut peer = Peer::start("reply");
    assert_eq!(
        exchange(&mut peer.child, b"fixture input", || false).unwrap(),
        b"fixture reply"
    );
    assert!(peer.child.try_wait().unwrap().unwrap().success());
}

#[test]
fn ssh_oversized_reply_is_bounded_and_peer_reaped() {
    let mut peer = Peer::start("limit");
    let error = exchange(&mut peer.child, b"fixture input", || false).unwrap_err();
    assert!(error.contains("exceeds protocol limit"));
    assert!(peer.child.try_wait().unwrap().is_some());
}

#[test]
fn ssh_failed_peer_is_reaped_without_claiming_success() {
    let mut peer = Peer::start("failed");
    let error = exchange(&mut peer.child, b"fixture input", || false).unwrap_err();
    assert!(error.contains("SSH process failed"));
    assert!(peer.child.try_wait().unwrap().is_some());
}

#[test]
fn uncertain_delivery_preserves_request_and_operation_identity() {
    let client = crate::remote::Client {
        endpoint: serde_json::Value::Null,
        authority: PathBuf::new(),
        state: PathBuf::new(),
        id: "retained-request".into(),
        operation: Some("retained-operation".into()),
        run_request: None,
        cwd: None,
        detach: false,
        artifact: false,
        file: None,
        args: vec![],
    };
    let message = client.delivery_error("cancelled during read");
    for required in [
        "retained-request",
        "retained-operation",
        "unknown",
        "never replay",
        "remote worker was not interrupted",
    ] {
        assert!(message.contains(required), "{message}");
    }
}
