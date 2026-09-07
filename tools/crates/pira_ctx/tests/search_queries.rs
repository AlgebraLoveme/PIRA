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
    assert_eq!(summary.matches(id).count(), 2);
    assert!(summary.contains("Retrieve:"));
    assert!(summary.contains("display_clipped_lines="));

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

fn capture_fixture(sandbox: &Sandbox, code: &str) -> (String, String) {
    let output = Command::new(binary())
        .current_dir(sandbox.path())
        .args([
            "capture",
            "--store-dir",
            sandbox.path().to_str().unwrap(),
            "--intent",
            "Create regression fixture",
            "--",
            python(),
            "-c",
            code,
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let summary = String::from_utf8(output.stdout).unwrap();
    let id = summary
        .lines()
        .find_map(|l| {
            l.strip_prefix("Result: ")
                .or_else(|| l.strip_prefix("Captured: "))
        })
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    (id, summary)
}

fn retrieve(sandbox: &Sandbox, command: &str, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(sandbox.path())
        .args([command, "--store-dir", sandbox.path().to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn long_line_middles_are_searched_and_oversized_lines_are_disclosed() {
    let sandbox = Sandbox::new();
    for code in [
        "print('a'*100000+'MIDDLE_MARKER'+'b'*100000)",
        "print('é'*80000+'MIDDLE_MARKER'+'ø'*80000)",
    ] {
        let (id, _) = capture_fixture(&sandbox, code);
        let output = retrieve(&sandbox, "search", &[&id, "MIDDLE_MARKER", "--regex"]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains("1 hits") && text.contains("MIDDLE_MARKER"),
            "{text}"
        );
    }
    let (id, _) = capture_fixture(&sandbox, "print('x'*(17*1024*1024))");
    let output = retrieve(&sandbox, "search", &[&id, "absent", "--regex"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("complete=0") && text.contains("skipped_lines=1"),
        "{text}"
    );
}

#[test]
fn range_notation_and_named_transforms_preserve_exact_results() {
    let sandbox = Sandbox::new();
    let (id, _) = capture_fixture(&sandbox, "print('one\\ntwo\\nthree')");
    for (pair, bounds) in [("1:2", ["1", "2"]), ("-2:-1", ["-2", "-1"])] {
        let guessed = retrieve(&sandbox, "range", &[&id, pair]);
        let canonical = retrieve(&sandbox, "range", &[&id, bounds[0], bounds[1]]);
        assert!(guessed.status.success());
        assert_eq!(guessed.stdout, canonical.stdout);
    }
    for op in ["head", "tail"] {
        let canonical = format!("--{op}");
        assert_eq!(
            retrieve(&sandbox, "transform", &[&id, op, "2"]).stdout,
            retrieve(&sandbox, "transform", &[&id, &canonical, "2"]).stdout
        );
    }
    assert!(!retrieve(&sandbox, "range", &[&id, "1:"]).status.success());
    assert!(retrieve(&sandbox, "list", &["--live"]).status.success());
}

#[test]
fn per_query_limits_and_byte_budget_disclose_omissions_without_starvation() {
    let sandbox = Sandbox::new();
    let (id, _) = capture_fixture(
        &sandbox,
        "\nfor i in range(120):\n print('ALPHA '+str(i)+' '+'x'*1500)\n print('BETA '+str(i)+' '+'y'*1500)",
    );
    let output = retrieve(
        &sandbox,
        "search",
        &[
            &id,
            "-e",
            "ALPHA",
            "-e",
            "BETA",
            "--limit",
            "100",
            "--context",
            "20",
        ],
    );
    assert!(output.status.success());
    assert!(output.stdout.len() <= 64 * 1024);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("q1 L") && text.contains("q2 L"), "{text}");
    assert!(text.contains("omitted=") && text.contains("byte_limited=1"));
    let narrow = retrieve(&sandbox, "search", &[&id, "ALPHA", "--limit", "2"]);
    let text = String::from_utf8(narrow.stdout).unwrap();
    assert!(text.contains("shown=2 omitted=118"), "{text}");
    assert!(!text.contains("score="));
}
