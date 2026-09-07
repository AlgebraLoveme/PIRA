use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pira-dec-search-{}-{}",
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

#[test]
fn empty_human_search_is_explicit_and_remains_a_no_match_exit() {
    let sandbox = Sandbox::new();
    let output = Command::new(env!("CARGO_BIN_EXE_pira_dec"))
        .args([
            "search",
            "--field",
            "context",
            "--regex",
            "will-not-match",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "decisions_matched=0 complete=1\n"
    );
    assert!(output.stderr.is_empty());
}

fn run(s: &Sandbox, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_pira_dec"))
        .current_dir(s.path())
        .args(args)
        .args(["--store-dir", s.path().to_str().unwrap()])
        .output()
        .unwrap()
}

#[test]
fn limits_are_disclosed_and_context_matches_are_visible() {
    let s = Sandbox::new();
    let mut ids = Vec::new();
    for context in [
        "older cache rationale",
        "middle cache rationale",
        "newer cache rationale",
    ] {
        let out = run(
            &s,
            &[
                "add",
                "--context",
                context,
                "--choice",
                "selected",
                "--choice",
                "alternative",
                "--decision",
                "1",
                "--maker",
                "agent",
            ],
        );
        assert!(out.status.success());
        ids.push(
            String::from_utf8(out.stdout)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_string(),
        );
    }
    let out = run(
        &s,
        &[
            "search", "--field", "context", "--regex", "cache", "--limit", "2",
        ],
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("has_more=1") && text.contains("match="),
        "{text}"
    );
    let out = run(&s, &["list", "--limit", "2", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["has_more"], true);
    assert_eq!(json["decisions"].as_array().unwrap().len(), 2);
    let out = run(
        &s,
        &[
            "search", "--field", "context", "--regex", "cache", "--limit", "3", "--json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["has_more"], false);
    assert!(run(&s, &["show", &ids[0]]).status.success());
    assert!(!run(&s, &["show", "D-"]).status.success());
    // A complete-ID read still verifies embedded identity and content integrity.
    let mut stack = vec![s.path().to_path_buf()];
    let mut record = None;
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_stem().is_some_and(|stem| stem == ids[0].as_str()) {
                record = Some(path);
            }
        }
    }
    fs::write(record.unwrap(), b"corrupt").unwrap();
    assert!(!run(&s, &["show", &ids[0]]).status.success());
    let out = run(&s, &["list", "--limit", "1", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["skipped_count"], 1);
}
