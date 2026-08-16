use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pira-ctx-search-{}-{}",
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

#[test]
fn repeatable_queries_rank_independently_and_keep_long_line_match_local() {
    let sandbox = Sandbox::new();
    let captured = Command::new(binary())
        .args([
            "capture",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--intent",
            "Create search fixture",
            "--",
            python(),
            "-c",
            "print('a'*5000+'NEEDLE_LOCAL'+'z'*5000); print('NEEDLE_LOCAL short')",
        ])
        .output()
        .unwrap();
    assert!(captured.status.success());
    let summary = String::from_utf8(captured.stdout).unwrap();
    let id = summary
        .lines()
        .find_map(|line| line.strip_prefix("Result: "))
        .and_then(|line| line.split(" | ").next())
        .expect("capture result ID");
    assert_eq!(summary.matches(id).count(), 1);
    assert!(!summary.contains("Retrieve:"));

    let searched = Command::new(binary())
        .args([
            "search",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            id,
            "-e",
            "NEEDLE_LOCAL",
            "-e",
            "ABSENT_QUERY",
        ])
        .output()
        .unwrap();
    assert!(searched.status.success());
    let output = String::from_utf8(searched.stdout).unwrap();
    assert!(output.contains("Query 1 \"NEEDLE_LOCAL\": 2 hits"));
    assert!(output.contains("Query 2 \"ABSENT_QUERY\": 0 hits"));
    assert!(output.contains("NEEDLE_LOCAL"));
    assert!(output.contains("bytes omitted"));
}
