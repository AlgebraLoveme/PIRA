use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "pira-nav-contracts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn write(&self, name: &str, text: &str) {
        fs::write(self.0.join(name), text).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pira_nav"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
    fn text(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn aliases_preserve_canonical_query_and_depth_meanings() {
    let s = Sandbox::new();
    s.write("source.rs", "fn alpha() {}\nfn beta() {}\n");
    assert_eq!(
        s.text(&["symbols", "-e", "alpha", "-e", "beta", "source.rs"]),
        s.text(&[
            "symbols",
            "--query",
            "alpha",
            "--query",
            "beta",
            "source.rs"
        ])
    );
    assert_eq!(
        s.text(&["outline", "source.rs", "--max-depth", "1", "--limit", "1"]),
        s.text(&["outline", "source.rs", "--depth", "1", "--max-items", "1"])
    );
    assert_eq!(
        s.text(&["map", ".", "--limit", "1"]),
        s.text(&["map", ".", "--max-items", "1"])
    );
    assert!(
        !s.run(&["outline", "source.rs", "--limit", "0"])
            .status
            .success()
    );
}

#[test]
fn slices_apply_to_resolved_symbols_sections_ranges_and_batches() {
    let s = Sandbox::new();
    s.write(
        "source.rs",
        "fn alpha() {\n let first = 1;\n let second = 2;\n}\nfn beta() {}\n",
    );
    s.write(
        "notes.md",
        "# Root\nintro\n## Section\none\ntwo\n## Other\nnot-selected\n",
    );
    let code = s.text(&["show", "source.rs::alpha", "--head", "2"]);
    assert!(
        code.contains("let first") && !code.contains("let second") && !code.contains("fn beta")
    );
    let doc = s.text(&["show", "notes.md::Root::Section", "--tail", "1"]);
    assert!(doc.contains("two") && !doc.contains("not-selected"));
    let range = s.text(&["show", "source.rs:1-4", "--tail", "2"]);
    assert!(range.contains("let second") && !range.contains("let first"));
    let batch = s.text(&[
        "show",
        "source.rs::alpha",
        "--head",
        "1",
        "notes.md::Root::Section",
        "--tail",
        "1",
    ]);
    assert!(batch.contains("fn alpha") && batch.contains("two"));
    assert!(
        !s.run(&["show", "source.rs::missing", "--head", "1"])
            .status
            .success()
    );
    assert!(
        !s.run(&["show", "source.rs:0-4", "--head", "1"])
            .status
            .success()
    );
}

#[test]
fn outline_shares_budget_with_later_files() {
    let s = Sandbox::new();
    s.write(
        "large.rs",
        &(0..70)
            .map(|i| format!("fn item_{i}() {{}}\n"))
            .collect::<String>(),
    );
    s.write("small.rs", "fn important() {}\n");
    for paths in [["large.rs", "small.rs"], ["small.rs", "large.rs"]] {
        let text = s.text(&["outline", paths[0], paths[1]]);
        assert!(text.contains("important"), "{text}");
        assert!(!text.contains("symbols=1 shown=0"), "{text}");
    }
}

#[test]
fn maps_accept_multiple_roots_and_globs_without_reincluding_ignored_files() {
    let s = Sandbox::new();
    fs::create_dir(s.0.join("a")).unwrap();
    fs::create_dir(s.0.join("b")).unwrap();
    s.write("a/alpha.rs", "fn alpha() {}\n");
    s.write("b/beta.rs", "fn beta() {}\n");
    s.write(".gitignore", "hidden.rs\n");
    s.write("hidden.rs", "fn hidden() {}\n");
    let multiple = s.text(&["map", "a", "b"]);
    assert!(multiple.contains("alpha") && multiple.contains("beta"));
    let filtered = s.text(&["map", ".", "-g", "!b/**"]);
    assert!(filtered.contains("alpha") && !filtered.contains("beta"));
    let positive = s.text(&["map", ".", "-g", "*.rs"]);
    assert!(!positive.contains("hidden.rs"));
    let overlap = s.text(&["map", "a", "a"]);
    assert!(overlap.contains("files=1"));
}

#[test]
fn search_limits_and_byte_pressure_keep_each_query_identifiable() {
    let s = Sandbox::new();
    s.write(
        "a.txt",
        &(0..20)
            .map(|i| format!("ALPHA {i} {}\n", "x".repeat(1200)))
            .collect::<String>(),
    );
    s.write(
        "b.txt",
        &(0..20)
            .map(|i| format!("BETA {i} {}\n", "y".repeat(1200)))
            .collect::<String>(),
    );
    let text = s.text(&[
        "search",
        "-e",
        "ALPHA",
        "-e",
        "BETA",
        ".",
        "--limit",
        "3",
        "--max-bytes",
        "1000",
    ]);
    assert!(text.contains("a.txt") && text.contains("b.txt"), "{text}");
    assert!(text.contains("byte_limited=1") && text.contains("per_query_limit=3"));
    assert!(text.contains("query index=1") && text.contains("query index=2"));
    s.write("bad.txt", "ok");
    fs::write(s.0.join("bad.txt"), [255, 254, 253]).unwrap();
    let text = s.text(&["search", "ALPHA", "."]);
    assert!(
        text.contains("skipped file=\"bad.txt\" reason=non_utf8"),
        "{text}"
    );
}

#[test]
fn repeated_aliases_fail_without_silent_overrides() {
    let s = Sandbox::new();
    s.write("source.rs", "fn alpha() {}\nfn beta() {}\n");
    for args in [
        vec!["outline", "source.rs", "--limit", "1", "--max-items", "2"],
        vec!["outline", "source.rs", "--depth", "1", "--max-depth", "2"],
        vec!["map", ".", "--limit", "1", "--max-items", "2"],
        vec![
            "symbols",
            "alpha",
            "source.rs",
            "--limit",
            "1",
            "--max-items",
            "2",
        ],
        vec![
            "search",
            "alpha",
            "source.rs",
            "--limit",
            "1",
            "--max-per-query",
            "2",
        ],
    ] {
        let o = s.run(&args);
        assert!(!o.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("only once"));
    }
}
#[test]
fn overlapping_context_preserves_hits_without_duplicate_source() {
    let s = Sandbox::new();
    s.write("source.rs", "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n");
    let o = s.text(&["search", "fn", "source.rs", "--limit", "3"]);
    for name in ["alpha", "beta", "gamma"] {
        assert_eq!(o.matches(&format!("fn {name}()")).count(), 1);
    }
    assert_eq!(o.matches("[q1]").count(), 3, "{o}");
}
