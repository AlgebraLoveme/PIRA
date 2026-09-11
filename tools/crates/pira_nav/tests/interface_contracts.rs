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

#[test]
fn signed_ranges_stay_within_the_resolved_content() {
    let s = Sandbox::new();
    s.write(
        "source.rs",
        "// outside\nfn alpha() {\n let first = 1;\n let second = 2;\n}\nfn beta() {}\n",
    );
    s.write(
        "notes.md",
        "# Root\nintro\n## Section\none\ntwo\n## Other\noutside\n",
    );
    for target in [
        "source.rs::alpha",
        "source.rs:2-5",
        "source.rs:2--2",
        "source.rs:3:2",
    ] {
        assert_eq!(
            s.text(&["show", target, "--range", "-2:-1"]),
            s.text(&["show", target, "--tail", "2"])
        );
        assert_eq!(
            s.text(&["show", target, "--range", "1:2"]),
            s.text(&["show", target, "--head", "2"])
        );
    }
    assert_eq!(
        s.text(&["show", "notes.md::Root::Section", "--range", "-1:-1"]),
        s.text(&["show", "notes.md::Root::Section", "--tail", "1"])
    );
    assert_eq!(
        s.text(&["show", "source.rs:-2--1"]),
        s.text(&["show", "source.rs", "--tail", "2"])
    );
    let batch = s.text(&[
        "show",
        "source.rs::alpha",
        "--range",
        "2:-2",
        "notes.md::Root::Section",
        "--range",
        "-1:-1",
    ]);
    assert!(batch.contains("let first") && batch.contains("let second") && batch.contains("two"));
    assert!(!batch.contains("fn beta") && !batch.contains("outside"));
    let outline = s.text(&["outline", "source.rs", "--selectors"]);
    let selector = outline
        .split_whitespace()
        .find_map(|word| word.strip_prefix("selector="))
        .unwrap()
        .trim_matches('"');
    assert_eq!(
        s.text(&["show", selector, "--range", "-1:-1"]),
        s.text(&["show", selector, "--tail", "1"])
    );
}

#[test]
fn signed_range_boundaries_errors_and_clipping() {
    let s = Sandbox::new();
    s.write("lines.txt", "one\r\n二\r\nthree");
    s.write("empty.txt", "");
    assert_eq!(
        s.text(&["show", "lines.txt", "--range", "2:99"]),
        s.text(&["show", "lines.txt:2-3"])
    );
    assert_eq!(
        s.text(&["show", "lines.txt:-2--1"]),
        s.text(&["show", "lines.txt", "--range", "2:-1"])
    );
    for range in [
        "0:1",
        "1:0",
        "-4:-1",
        "4:9",
        "3:2",
        "-1:-2",
        "1:-4",
        "1:9223372036854775808",
        "x:2",
    ] {
        assert!(
            !s.run(&["show", "lines.txt", "--range", range])
                .status
                .success(),
            "{range}"
        );
    }
    for args in [
        vec!["show", "empty.txt", "--range", "1:-1"],
        vec!["show", "lines.txt:0-2"],
        vec!["show", "lines.txt", "--range", "1:2", "--tail", "1"],
        vec!["show", "lines.txt:2", "--window", "1", "--range", "1:2"],
        vec!["show", "--range", "1:2", "lines.txt"],
    ] {
        assert!(!s.run(&args).status.success(), "{args:?}");
    }
    let capped = s.text(&["show", "lines.txt", "--range", "1:-1", "--max-bytes", "1"]);
    assert!(capped.contains("shown=0") && capped.contains("byte_limited=1"));
}

#[test]
fn markdown_outline_provides_canonical_segments_and_failed_targets_offer_exact_repairs() {
    let s = Sandbox::new();
    for title in [
        "Retry [edge:cases]",
        "Reader's \"notes\"",
        "One :: Two",
        "路径 [检查]",
    ] {
        s.write(
            "notes.md",
            &format!("# Root\n## {title}\nselected body\n## Other\noutside\n"),
        );
        let segment = format!("[{}]", serde_json::to_string(title).unwrap());
        let target = format!("notes.md::Root::{segment}");
        let outline = s.text(&["outline", "notes.md"]);
        assert!(outline.contains(&segment), "{outline}");
        let failure = s.run(&["show", &format!("notes.md::Root::{title}")]);
        assert_eq!(failure.status.code(), Some(3));
        let error = String::from_utf8(failure.stderr).unwrap();
        assert!(
            error.contains(&serde_json::to_string(&target).unwrap()),
            "{error}"
        );
        let exact = s.text(&["show", &target]);
        assert!(exact.contains("selected body") && !exact.contains("outside"));
    }
    s.write(
        "notes.md",
        "# A\n## Repeat [x]\none\n# B\n## Repeat [x]\ntwo\n",
    );
    let failure = s.run(&["show", "notes.md::Repeat [x]"]);
    assert_eq!(failure.status.code(), Some(3));
    assert!(
        !String::from_utf8(failure.stderr)
            .unwrap()
            .contains("canonical target=")
    );
    s.write("code.py", "def ordinary():\n    return 1\n");
    assert!(s.text(&["outline", "code.py"]).contains("ordinary"));
    assert!(s.text(&["show", "code.py::ordinary"]).contains("return 1"));
}

#[test]
fn mixed_query_show_preserves_ranges_order_and_independent_failures() {
    let s = Sandbox::new();
    s.write(
        "source.py",
        "def alpha():\n    value = 7\n    return value\n",
    );
    s.write("notes.md", "# Topic\nfirst\nlast\n");
    let text = s.text(&[
        "query",
        "--show",
        "source.py::alpha",
        "--range",
        "-1:-1",
        "--references",
        "missing.py::absent",
        "--show",
        "notes.md",
        "--range",
        "-2:-1",
    ]);
    assert!(text.contains("    return value"));
    assert!(!text.contains("    value = 7"));
    assert!(text.find("    return value").unwrap() < text.find("query error").unwrap());
    assert!(text.find("query error").unwrap() < text.find("first\nlast").unwrap());
    assert!(text.contains("requests=3 succeeded=2 failed=1 complete=0"));
    s.write("--odd.md", "dash file\n");
    assert!(
        s.text(&["query", "--show", "--odd.md", "--range", "-1:-1"])
            .contains("dash file")
    );
    let capped = s.text(&["query", "--show", "notes.md", "--max-bytes", "1"]);
    assert!(capped.contains("byte_limited=1"));
    assert!(!capped.contains("first\nlast"));
    for args in [
        vec!["query", "--show"],
        vec!["query", "--range", "-1:-1", "--show", "notes.md"],
        vec!["query", "--show", "notes.md", "--range", "0:1"],
        vec![
            "query", "--show", "notes.md", "--range", "1:1", "--range", "2:2",
        ],
        vec!["query", "--show", "missing.md"],
    ] {
        assert!(!s.run(&args).status.success(), "{args:?}");
    }
}

#[test]
fn mixed_query_missing_lsp_does_not_block_source() {
    let s = Sandbox::new();
    s.write("source.py", "def alpha():\n    return 8\n");
    let output = Command::new(env!("CARGO_BIN_EXE_pira_nav"))
        .current_dir(&s.0)
        .env("PATH", "")
        .args([
            "query",
            "--references",
            "source.py::alpha",
            "--show",
            "source.py::alpha",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("requires an LSP"));
    assert!(text.contains("    return 8"));
    assert!(text.contains("succeeded=1 failed=1 complete=0"));
}

#[test]
fn canonical_targets_do_not_fall_back_to_legacy_heading_spellings() {
    let s = Sandbox::new();
    s.write("notes.md", "# A::B\nliteral\n# A\n## B\nnested\n");
    let nested = s.text(&["show", "notes.md::A::B"]);
    assert!(nested.contains("nested"));
    assert!(!nested.contains("literal"));
    let literal = s.text(&["show", "notes.md::[\"A::B\"]"]);
    assert!(literal.contains("literal"));
    assert!(!literal.contains("nested"));
    s.write("only.md", "# A::B\nliteral\n");
    assert!(!s.run(&["show", "only.md::A::B"]).status.success());

    s.write(
        "code.py",
        "class Box:\n    def run(self):\n        return 1\n",
    );
    // An unambiguous native-language spelling remains a compatible guess.
    assert_eq!(
        s.text(&["show", "code.py::Box.run"]),
        s.text(&["show", "code.py::Box::run"])
    );
    s.write("dupes.py", "class A:\n    def run(self):\n        return 1\nclass B:\n    def run(self):\n        return 2\n");
    assert!(!s.run(&["show", "dupes.py::run"]).status.success());
}

#[test]
fn query_rejects_options_with_no_applicable_operation_before_output() {
    let s = Sandbox::new();
    s.write("notes.md", "# Notes\ntext\n");
    for args in [
        vec!["query", "--show", "notes.md", "--limit", "2"],
        vec!["query", "--hover", "file.py::item", "--limit", "2"],
        vec![
            "query",
            "--references",
            "file.py::item",
            "--max-bytes",
            "20",
        ],
        vec!["query", "--show", "notes.md", "--include-declaration"],
    ] {
        let output = s.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("requires"));
    }
}

#[test]
fn incomplete_inventory_cannot_establish_name_uniqueness() {
    let s = Sandbox::new();
    s.write("small.json", r#"[{"needle":1},{"needle":2}]"#);
    let small = s.run(&["show", "small.json::needle"]);
    assert_eq!(small.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&small.stderr).contains("ambiguous"));
    s.write(
        "large.json",
        &format!("[{{\"needle\":1}},{}{{\"needle\":2}}]", "{},".repeat(20000)),
    );
    assert!(
        s.text(&["outline", "large.json", "--limit", "2"])
            .contains("truncated=1")
    );
    let large = s.run(&["show", "large.json::needle"]);
    assert_eq!(large.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&large.stderr).contains("cannot establish uniqueness"));
    assert!(large.stdout.is_empty());
    assert!(s.run(&["show", "large.json:1-1"]).status.success());
}
