#![cfg(any(target_os = "macos", target_os = "linux", windows))]

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pira-dec-relationships-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
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
    env!("CARGO_BIN_EXE_pira_dec")
}

fn add(store: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "add",
        "--store-dir",
        store.to_str().unwrap(),
        "--context",
        "Choose storage",
        "--choice",
        "Files",
        "--choice",
        "Database",
        "--decision",
        "1",
        "--maker",
        "human",
    ];
    args.extend_from_slice(extra);
    Command::new(binary()).args(args).output().unwrap()
}

#[test]
fn add_validates_and_displays_relationships() {
    let sandbox = Sandbox::new();
    let first = add(sandbox.path(), &[]);
    assert!(first.status.success());
    let first_id = String::from_utf8(first.stdout)
        .unwrap()
        .split(" | ")
        .next()
        .unwrap()
        .to_string();

    let second = add(sandbox.path(), &["--supersedes", &first_id]);
    assert!(second.status.success());
    let second_id = String::from_utf8(second.stdout)
        .unwrap()
        .split(" | ")
        .next()
        .unwrap()
        .to_string();
    let shown = Command::new(binary())
        .args([
            "show",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--json",
            &second_id,
        ])
        .output()
        .unwrap();
    assert!(shown.status.success());
    let view: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(view["supersedes"], first_id);
}

#[test]
fn add_rejects_missing_relationship_target() {
    let sandbox = Sandbox::new();
    let missing = "D-20260716-063012-a3f921c84d77e102";
    let output = add(sandbox.path(), &["--related", missing]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("does not exist")
    );
}

#[test]
fn relationship_is_revalidated_after_waiting_for_publication_lock() {
    use std::time::{Duration, Instant};
    let sandbox = Sandbox::new();
    let first = add(sandbox.path(), &[]);
    assert!(first.status.success());
    let id = String::from_utf8(first.stdout)
        .unwrap()
        .split(" | ")
        .next()
        .unwrap()
        .to_string();
    let workspace = fs::read_dir(sandbox.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(workspace.join(".write.lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut child = Command::new(binary())
        .args([
            "add",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--context",
            "concurrent relation",
            "--choice",
            "one",
            "--choice",
            "two",
            "--decision",
            "1",
            "--maker",
            "agent",
            "--related",
            &id,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // A staged file proves the child reached publication while we hold its lock.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_dir(workspace.join(".tmp"))
        .unwrap()
        .next()
        .is_none()
    {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not stage a record");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    fs::remove_file(workspace.join("records").join(format!("{id}.piradec"))).unwrap();
    drop(lock);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not finish after lock release");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("does not exist")
    );
    assert_eq!(fs::read_dir(workspace.join("records")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(workspace.join(".tmp")).unwrap().count(), 0);
}

#[test]
fn repeated_makers_keep_human_precedence() {
    let sandbox = Sandbox::new();
    for pair in [
        ["agent", "agent"],
        ["human", "human"],
        ["agent", "human"],
        ["human", "agent"],
    ] {
        // The shared add helper supplies human first; use the CLI directly here.
        let output = Command::new(binary())
            .args([
                "add",
                "--store-dir",
                sandbox.path().to_str().unwrap(),
                "--context",
                "authority",
                "--choice",
                "one",
                "--choice",
                "two",
                "--decision",
                "1",
                "--maker",
                pair[0],
                "--maker",
                pair[1],
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let id = String::from_utf8(output.stdout)
            .unwrap()
            .split(" | ")
            .next()
            .unwrap()
            .to_string();
        let output = Command::new(binary())
            .args([
                "show",
                &id,
                "--json",
                "--store-dir",
                sandbox.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            record["maker"],
            if pair.contains(&"human") {
                "human"
            } else {
                "agent"
            }
        );
    }
    let help = Command::new(binary())
        .args(["add", "--help"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("Repeats are accepted")
    );
}

fn show_legacy_makers(makers: &[&[u8]]) -> std::process::Output {
    let sandbox = Sandbox::new();
    let added = add(sandbox.path(), &[]);
    assert!(added.status.success(), "{added:?}");
    let text = String::from_utf8(added.stdout).unwrap();
    let id = text.split(" | ").next().unwrap();
    let workspace = fs::read_dir(sandbox.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = workspace.join("records").join(format!("{id}.piradec"));
    let encoded = fs::read(&path).unwrap();
    let body_len = u32::from_le_bytes(encoded[8..12].try_into().unwrap()) as usize;
    let mut body = encoded[12..12 + body_len].to_vec();
    // This fixture has no relationships, so its last TLV is the human maker.
    assert_eq!(&body[body.len() - 6..], &[6, 1, 0, 0, 0, 1]);
    body.truncate(body.len() - 6);
    for value in makers {
        body.push(6);
        body.extend_from_slice(&(value.len() as u32).to_le_bytes());
        body.extend_from_slice(value);
    }
    let mut legacy = encoded[..8].to_vec();
    legacy.extend_from_slice(&(body.len() as u32).to_le_bytes());
    legacy.extend_from_slice(&body);
    legacy.extend_from_slice(&Sha256::digest(&body));
    fs::write(path, legacy).unwrap();
    Command::new(binary())
        .args([
            "show",
            id,
            "--json",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

#[test]
fn legacy_maker_tags_keep_human_precedence_in_either_order() {
    let cases: &[(&[&[u8]], &str)] = &[
        (&[&[2], &[2]], "agent"),
        (&[&[1], &[2]], "human"),
        (&[&[2], &[1]], "human"),
    ];
    for (makers, expected) in cases {
        let output = show_legacy_makers(makers);
        assert!(output.status.success(), "{output:?}");
        let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(record["maker"], *expected);
    }
}

#[test]
fn legacy_record_requires_a_maker() {
    let output = show_legacy_makers(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"pira_dec: missing decision maker\n");
    assert!(output.stdout.is_empty());
}

#[test]
fn legacy_unknown_maker_is_rejected_even_after_human_authority() {
    let cases: &[&[&[u8]]] = &[&[&[0], &[1]], &[&[1], &[0]]];
    for makers in cases {
        let output = show_legacy_makers(makers);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stderr, b"pira_dec: unknown maker value\n");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn legacy_malformed_maker_is_rejected_even_after_human_authority() {
    let output = show_legacy_makers(&[&[1], &[2, 2]]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        output.stderr,
        b"pira_dec: duplicate or malformed singleton decision field\n"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn human_show_escapes_controls_without_changing_json() {
    let sandbox = Sandbox::new();
    let text = "a\x07\x08\r\x1b[31m\u{009b}31m\u{202e}\n\tb";
    let output = add(sandbox.path(), &["--choice", text]);
    assert!(output.status.success());
    let id = String::from_utf8(output.stdout)
        .unwrap()
        .split(" | ")
        .next()
        .unwrap()
        .to_string();
    let output = Command::new(binary())
        .args(["show", &id, "--store-dir", sandbox.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let shown = String::from_utf8(output.stdout).unwrap();
    assert!(
        !shown
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    );
    assert!(!shown.contains('\u{202e}'));
    assert!(shown.contains("\\u{1b}[31m"));
    assert!(shown.contains("\n\tb"));
    let output = Command::new(binary())
        .args([
            "show",
            &id,
            "--json",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["choices"][2], text);
}
