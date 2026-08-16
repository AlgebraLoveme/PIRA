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
