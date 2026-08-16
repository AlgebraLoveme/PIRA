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
