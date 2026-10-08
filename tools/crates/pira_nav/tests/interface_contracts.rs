#[cfg(unix)]
mod common;
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
fn root_array_descendants_and_selectors_round_trip() {
    let s = Sandbox::new();
    for (file, source) in [("root.json", "[{\"x\":1}]"), ("root.yaml", "- x: 1\n")] {
        s.write(file, source);
        let shown = s.text(&["show", &format!("{file}::[0]::x")]);
        assert!(shown.contains("[0]::x"));
        let outline = s.text(&["outline", file, "--selectors"]);
        for selector in outline
            .split_whitespace()
            .filter_map(|word| word.strip_prefix("selector="))
        {
            assert!(s.text(&["show", selector]).contains("--- begin ---"));
        }
    }
}

#[test]
fn toml_subtables_keep_their_array_parent_occurrence() {
    let s = Sandbox::new();
    s.write(
        "tables.toml",
        "\
[[products]]
name = 'one'
[products.details]
id = 1
[[products]]
name = 'two'
[products.details]
id = 2
[ordinary.dotted]
id = 3
[\"products.details\"]
id = 4
",
    );
    for (path, value) in [
        ("products[0]::details::id", 1),
        ("products[1]::details::id", 2),
        ("ordinary::dotted::id", 3),
        ("[\"products.details\"]::id", 4),
    ] {
        let shown = s.text(&["show", &format!("tables.toml::{path}")]);
        assert!(
            shown.contains(&format!("\n--- begin ---\nid = {value}\n--- end ---")),
            "{shown}"
        );
    }
    let outline = s.text(&["outline", "tables.toml", "--selectors"]);
    assert!(!outline.contains("table products::details "));
    for selector in outline
        .split_whitespace()
        .filter_map(|s| s.strip_prefix("selector="))
    {
        s.text(&["show", selector]);
    }
}

#[test]
fn toml_nested_array_counters_are_scoped_to_resolved_parents() {
    let s = Sandbox::new();
    s.write(
        "tables.toml",
        "\
[[a]]
[[a.b]]
x = 1
[[a.b]]
x = 2
[a.b.details]
x = 3
[[a]]
[[a.b]]
x = 4
[a.b.details]
x = 5
[[\"a.b\"]]
x = 6
[[\"a.b\".children]]
x = 7
",
    );
    for (path, value) in [
        ("a[0]::b[0]::x", 1),
        ("a[0]::b[1]::x", 2),
        ("a[0]::b[1]::details::x", 3),
        ("a[1]::b[0]::x", 4),
        ("a[1]::b[0]::details::x", 5),
        ("[\"a.b\"][0]::x", 6),
        ("[\"a.b\"][0]::children[0]::x", 7),
    ] {
        let shown = s.text(&["show", &format!("tables.toml::{path}")]);
        assert!(
            shown.contains(&format!("\n--- begin ---\nx = {value}\n--- end ---")),
            "{shown}"
        );
    }
}

#[test]
fn numeric_structural_names_are_not_coordinates() {
    let s = Sandbox::new();
    s.write("names.md", "# 2024\nfirst\n# 1-2\nsecond\n");
    s.write("names.json", "{\"2024\":1,\"1-2\":2}");
    for file in ["names.md", "names.json"] {
        for name in ["2024", "1-2"] {
            let target = format!("{file}::{name}");
            assert!(s.text(&["show", &target]).contains("--- begin ---"));
            assert!(
                s.text(&["show", &target, "--range", "1:1"])
                    .contains("--- begin ---")
            );
            assert!(
                s.text(&["query", "--show", &target])
                    .contains("--- begin ---")
            );
        }
        assert!(
            s.text(&["show", &format!("{file}:1-1")])
                .contains("range=L1-L1")
        );
    }
    assert!(s.text(&["show", "names.md:1:1"]).contains("2024"));
    #[cfg(unix)]
    {
        s.write("file::name.py", "def target(): pass\n");
        assert!(
            s.text(&["show", "file::name.py:1-1"])
                .contains("def target")
        );
        assert!(
            s.text(&["show", "file::name.py:1:1"])
                .contains("def target")
        );
    }
}

#[test]
fn jsonc_trailing_commas_do_not_hide_missing_values() {
    let s = Sandbox::new();
    for source in ["[,]", "{,}", "[/*c*/,]", "{/*c*/,}", "{\"a\":,}", "[1,,]"] {
        s.write("bad.jsonc", source);
        assert!(
            !s.run(&["outline", "bad.jsonc", "--native"])
                .status
                .success(),
            "{source}"
        );
    }
    for source in ["[1,]", "{\"a\":1,}", "[[],{},\"\",true,null,/*c*/]"] {
        s.write("good.jsonc", source);
        assert!(
            s.text(&["outline", "good.jsonc", "--native"])
                .contains("symbols=")
        );
    }
}

#[cfg(unix)]
#[test]
fn bash_imports_preserve_literals_and_leave_expansions_unresolved() {
    let s = Sandbox::new();
    fs::create_dir(s.0.join("sub")).unwrap();
    s.write("lib.sh", "echo wrong\n");
    s.write("sub/lib.sh", "echo right\n");
    s.write("sub/space  name.sh", "echo spaced\n");
    s.write("main.sh", "source sub/lib.sh\n.\t\"sub/space  name.sh\"\nsource sub/space\\ \\ name.sh\nsource \"$BASE/lib.sh\"\nsource sub/*.sh\nsource missing.sh\n");
    let text = common::success(common::lsp(
        &s.0,
        &["imports", "main.sh"],
        serde_json::json!({"positions": {
            "main.sh:0:7": ["sub/lib.sh"], "main.sh:1:3": ["sub/space  name.sh"], "main.sh:2:7": ["sub/space  name.sh"]
        }}),
    ));
    assert!(text.contains("local=3 external=0 unresolved=3"), "{text}");
    assert!(text.contains("target=\"sub/lib.sh\""), "{text}");
    assert_eq!(
        text.matches("target=\"sub/space  name.sh\"").count(),
        2,
        "{text}"
    );
    assert!(!text.contains("target=\"lib.sh\""), "{text}");
}

#[cfg(unix)]
#[test]
fn structural_operands_reject_file_directory_and_parent_symlinks() {
    let s = Sandbox::new();
    fs::create_dir(s.0.join("real")).unwrap();
    s.write("real/a.py", "def alpha(): pass\n");
    std::os::unix::fs::symlink("real/a.py", s.0.join("file.py")).unwrap();
    std::os::unix::fs::symlink("real", s.0.join("linked")).unwrap();
    for args in [
        vec!["show", "file.py"],
        vec!["show", "file.py:1-1"],
        vec!["outline", "linked/a.py"],
        vec!["map", "linked"],
        vec!["symbols", "alpha", "linked"],
        vec!["imports", "file.py"],
        vec!["dependents", "linked/a.py"],
        vec!["definition", "file.py:1:5"],
    ] {
        let output = s.run(&args);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("symlink"), "{args:?}: {text}");
        assert!(!text.contains("def alpha()"), "{args:?}: {text}");
    }
    assert!(s.text(&["outline", "real/a.py"]).contains("alpha"));
    s.write("main.sh", "source file.py\n");
    let edge = common::success(common::lsp(
        &s.0,
        &["imports", "main.sh"],
        serde_json::json!({"default": ["file.py"]}),
    ));
    assert!(edge.contains("resolution=blocked"), "{edge}");
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

#[test]
fn global_flags_remain_opaque_when_used_as_operands() {
    let s = Sandbox::new();
    s.write("patterns.txt", "--language\n--help\n--native\n--lsp\n--\n");
    for flag in ["--language", "--help", "--native", "--lsp", "--"] {
        s.write("patterns.txt", &format!("{flag}\n"));
        let output = s.text(&["search", "-e", flag, "patterns.txt", "-C", "0"]);
        assert!(output.contains("matching_lines=1"), "{output}");
        s.write(flag, "operand contents\n");
        assert!(
            s.text(&["query", "--show", flag])
                .contains("operand contents")
        );
    }
    assert!(s.text(&["search", "--help"]).contains("pira_nav search"));
    assert!(
        s.text(&["symbols", "--contains", "--help"])
            .contains("pira_nav symbols")
    );
}

#[test]
fn unique_symbol_excerpt_excludes_same_line_neighbors() {
    let s = Sandbox::new();
    for source in [
        "fn alpha() {} fn beta() {}",
        "fn beta() {} fn alpha() { /*é*/ }\n",
    ] {
        s.write("inline.rs", source);
        let output = s.text(&["symbols", "alpha", "inline.rs"]);
        assert!(output.contains("fn alpha()"), "{output}");
        assert!(!output.contains("fn beta()"), "{output}");
    }
}

#[test]
fn rust_module_functions_and_trait_signatures_keep_their_kinds() {
    let s = Sandbox::new();
    s.write("items.rs", "mod nested { pub fn alpha() {} mod deeper { fn beta() {} } }\ntrait Trait { fn required(&self); fn defaulted(&self) {} }\nstruct Thing; impl Thing { fn method(&self) {} }\n");
    let output = s.text(&["outline", "items.rs"]);
    for (kind, name) in [
        ("function", "nested::alpha"),
        ("function", "nested::deeper::beta"),
        ("method", "Trait::required"),
        ("method", "Trait::defaulted"),
        ("method", "Thing::method"),
    ] {
        let row = output.lines().find(|line| line.contains(name)).expect(name);
        assert!(row.contains(kind), "{row}");
    }
}

#[test]
fn repeated_show_bounds_are_rejected() {
    let s = Sandbox::new();
    s.write("source.py", "def alpha(): pass\n");
    for flag in ["--max-items", "--max-bytes"] {
        let output = s.run(&["show", "source.py", flag, "20", flag, "30"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("may be specified only once"));
    }
    assert!(
        s.text(&[
            "show",
            "source.py",
            "source.py",
            "--max-items",
            "2",
            "--max-bytes",
            "1024"
        ])
        .contains("def alpha")
    );
}

#[cfg(unix)]
#[test]
fn selector_paths_round_trip_without_display_sanitization() {
    let s = Sandbox::new();
    for name in [
        "back\\slash.rs",
        "new\nline.rs",
        "tab\tname.rs",
        "bidi\u{202e}é.rs",
        "percent%23.rs",
    ] {
        s.write(name, "fn alpha() {}\n");
        for args in [
            vec!["outline", name, "--selectors"],
            vec!["symbols", "alpha", name, "--selectors"],
        ] {
            let output = s.text(&args);
            let selector = output
                .split_whitespace()
                .find_map(|word| word.strip_prefix("selector="))
                .unwrap();
            assert!(
                s.text(&["show", selector]).contains("fn alpha() {}"),
                "{selector}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn parent_cancellation_does_not_hide_symlinks() {
    let s = Sandbox::new();
    fs::create_dir(s.0.join("real")).unwrap();
    std::os::unix::fs::symlink("real", s.0.join("linked")).unwrap();
    s.write("source.py", "def alpha(): pass\n");
    for path in ["linked/../source.py", "real/../linked/../source.py"] {
        let output = s.run(&["show", path]);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("does not follow symlinks"),
            "{output:?}"
        );
    }
    assert!(s.text(&["show", "real/../source.py"]).contains("def alpha"));
}

#[cfg(unix)]
#[test]
fn rust_module_edges_reject_linked_files_and_directories() {
    let s = Sandbox::new();
    s.write("lib.rs", "mod ordinary; mod linked; mod folder;\n");
    s.write("ordinary.rs", "fn alpha() {}\n");
    fs::create_dir(s.0.join("real")).unwrap();
    s.write("real/mod.rs", "fn beta() {}\n");
    std::os::unix::fs::symlink("ordinary.rs", s.0.join("linked.rs")).unwrap();
    std::os::unix::fs::symlink("real", s.0.join("folder")).unwrap();
    let config = serde_json::json!({"positions": {"lib.rs:0:4":["ordinary.rs"], "lib.rs:0:18":["linked.rs"], "lib.rs:0:30":["folder/mod.rs"]}});
    let output = common::success(common::lsp(&s.0, &["imports", "lib.rs"], config.clone()));
    assert_eq!(output.matches("resolution=blocked").count(), 2, "{output}");
    assert!(
        output.contains("target=\"ordinary.rs\" resolution=lsp"),
        "{output}"
    );
    let output = common::success(common::lsp(&s.0, &["deps", "lib.rs"], config));
    assert!(!output.contains("to=\"linked.rs\""), "{output}");
    assert!(!output.contains("to=\"folder/mod.rs\""), "{output}");
}

#[cfg(unix)]
#[test]
fn rust_module_declarations_accept_server_identity_across_module_layouts() {
    for (source, declaration, expected) in [
        ("lib.rs", "pub mod child;", "child.rs"),
        ("main.rs", "mod child;", "child.rs"),
        ("outer/mod.rs", "pub mod child;", "outer/child.rs"),
        ("outer.rs", "pub mod child;", "outer/child.rs"),
        (
            "outer.rs",
            "mod inner { mod deep { mod child; } }",
            "outer/inner/deep/child/mod.rs",
        ),
        ("lib.rs", "mod inner { mod r#type; }", "inner/type.rs"),
    ] {
        let s = Sandbox::new();
        for path in [source, expected] {
            fs::create_dir_all(s.0.join(path).parent().unwrap()).unwrap();
        }
        s.write("lib.rs", "pub mod outer;\n");
        s.write("child.rs", "pub fn unrelated() {}\n");
        s.write("type.rs", "pub fn unrelated() {}\n");
        s.write(expected, "pub fn correct() {}\n");
        s.write(source, declaration);
        let config = serde_json::json!({"default": [expected]});
        let output = common::success(common::lsp(&s.0, &["imports", source], config.clone()));
        assert!(
            output.contains(&format!("target=\"{expected}\" resolution=lsp")),
            "{source}: {output}"
        );
        if source == "outer.rs" && declaration == "pub mod child;" {
            let output = common::success(common::lsp(
                &s.0,
                &["deps", source, "--direction", "imports"],
                config.clone(),
            ));
            assert!(output.contains("to=\"outer/child.rs\""), "{output}");
            assert!(!output.contains("to=\"child.rs\""), "{output}");
            assert!(
                common::success(common::lsp(
                    &s.0,
                    &["dependents", "child.rs"],
                    config.clone()
                ))
                .contains("count=0")
            );
            assert!(
                common::success(common::lsp(&s.0, &["dependents", expected], config.clone()))
                    .contains("dependent=\"outer.rs\"")
            );
            fs::remove_file(s.0.join(expected)).unwrap();
            let output = common::success(common::lsp(&s.0, &["imports", source], config.clone()));
            assert!(output.contains("resolution=unsupported"), "{output}");
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink("../child.rs", s.0.join(expected)).unwrap();
                let output =
                    common::success(common::lsp(&s.0, &["imports", source], config.clone()));
                assert!(output.contains("resolution=blocked"), "{output}");
            }
        }
    }
}
