#![cfg(unix)]
mod common;

use common::{lsp, success};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nav-semantics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, path: &str, text: &str) {
        fs::create_dir_all(self.0.join(path).parent().unwrap()).unwrap();
        fs::write(self.0.join(path), text).unwrap();
    }
    fn run(&self, args: &[&str], config: Value) -> String {
        success(lsp(&self.0, args, config))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn deps_admits_explicit_ignored_and_unknown_suffix_targets_once() {
    for name in [
        "custom.inc",
        "ignored.cpp",
        "excluded/target.cpp",
        "ordinary.cpp",
    ] {
        let f = Fixture::new();
        f.write(name, "#include \"a.h\"\n");
        f.write("a.h", "int value;\n");
        f.write("peer.cpp", "int peer;\n");
        f.write("ignored_peer.cpp", "this is not valid C++");
        f.write(".gitignore", "ignored.cpp\nignored_peer.cpp\nexcluded/\n");
        let key = format!("{name}:0:10");
        let config = json!({"positions":{&key:["a.h"]}});
        let out = f.run(
            &["deps", name, "--language", "cpp", "--direction", "imports"],
            config,
        );
        assert!(out.contains("edges=1"), "{out}");
        assert!(out.contains("to=\"a.h\""), "{out}");
        assert!(!out.contains("failed="), "{out}");
        assert_eq!(
            fs::read_to_string(f.0.join(".requests")).unwrap(),
            format!("{key}\n")
        );
    }
}

#[test]
fn lsp_apply_edit_is_refused_without_source_mutation() {
    let f = Fixture::new();
    let source = "int visible(void) { return 1; }";
    f.write("source.c", source);
    let config = json!({"request_edit":true,"log_requests":false,
        "symbols":[server_symbol("visible",0,source.len(),4)]});
    let out = f.run(&["outline", "source.c"], config);
    assert!(out.contains("function visible"), "{out}");
    assert_eq!(fs::read_to_string(f.0.join("source.c")).unwrap(), source);
    let files = fs::read_dir(&f.0).unwrap().count();
    assert_eq!(
        files, 2,
        "only source and the supplied peer config should exist"
    );
}

#[test]
fn structural_lsp_root_blocks_exposure_but_not_native_reads() {
    let f = Fixture::new();
    f.write("allowed/inside.c", "int visible(void) { return 1; }");
    f.write("allowed-extra/outside.c", "int visible(void) { return 2; }");
    let started = f.0.join("allowed/.started");
    let opened = f.0.join("allowed/.opened");
    let config = json!({"startup_log":started,"open_log":opened,
        "symbols":[server_symbol("visible",0,30,4)]});
    f.write("allowed/.nav-lsp.json", &config.to_string());
    for args in [
        vec!["outline", "allowed-extra/outside.c"],
        vec!["symbols", "visible", "allowed-extra/outside.c"],
        vec!["show", "allowed-extra/outside.c::visible"],
        vec!["query", "--show", "allowed-extra/outside.c::visible"],
    ] {
        let mut args = args;
        args.extend(["--lsp-root", "allowed"]);
        let out = lsp(&f.0, &args, config.clone());
        assert!(!out.status.success(), "{out:?}");
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            diagnostic.contains("outside the selected LSP root"),
            "{out:?}"
        );
        assert!(!started.exists(), "server started for rejected target");
        assert!(!opened.exists(), "source disclosed");
    }
    let native = Command::new(env!("CARGO_BIN_EXE_pira_nav"))
        .current_dir(&f.0)
        .args([
            "show",
            "allowed-extra/outside.c::visible",
            "--lsp-root",
            "allowed",
        ])
        .output()
        .unwrap();
    assert!(success(native).contains("return 2"));
    let out = f.run(
        &["outline", "allowed/inside.c", "--lsp-root", "allowed"],
        config.clone(),
    );
    assert!(out.contains("backend=lsp"));
    assert_eq!(fs::read_to_string(&opened).unwrap().lines().count(), 1);
    fs::remove_file(&opened).unwrap();
    let out = f.run(
        &["symbols", "visible", ".", "--lsp-root", "allowed"],
        config,
    );
    assert!(out.contains("complete=0"), "{out}");
    let disclosed = fs::read_to_string(opened).unwrap();
    assert!(disclosed.contains("/allowed/inside.c"));
    assert!(!disclosed.contains("outside.c"));
}

#[test]
fn python_members_and_unicode_aliases_use_original_reference_positions() {
    let f = Fixture::new();
    f.write(
        "pkg/main.py",
        "from . import child\nfrom pkg import café as renamed, child\n",
    );
    f.write("pkg/__init__.py", "");
    f.write("pkg/child.py", "value=1\n");
    f.write("pkg/value.py", "café=1\n");
    let config = json!({"positions": {"pkg/main.py:0:14": ["pkg/child.py"], "pkg/main.py:1:5": ["pkg/__init__.py"], "pkg/main.py:1:16": ["pkg/value.py"], "pkg/main.py:1:33": ["pkg/child.py"]}});
    let out = f.run(&["imports", "pkg/main.py"], config.clone());
    assert!(out.contains("local=4 external=0 unresolved=0"), "{out}");
    let requests = fs::read_to_string(f.0.join(".requests")).unwrap();
    assert_eq!(
        requests,
        "pkg/main.py:0:14\npkg/main.py:1:5\npkg/main.py:1:16\npkg/main.py:1:33\n"
    );
    let out = f.run(&["dependents", "pkg/child.py"], config.clone());
    assert!(out.contains("dependent=\"pkg/main.py\""), "{out}");
    let out = f.run(&["deps", "pkg/main.py", "--direction", "imports"], config);
    assert!(out.contains("to=\"pkg/child.py\""), "{out}");
}

#[test]
fn rust_grouped_imports_resolve_members_not_filename_prefixes() {
    let f = Fixture::new();
    f.write(
        "foo.rs",
        "mod child;\nuse self::child::{Thing, Other as Alias};\n",
    );
    f.write("child.rs", "pub struct Wrong;\n");
    f.write("foo/child.rs", "pub struct Thing;\n");
    f.write("renamed.rs", "pub struct Other;\n");
    let out = f.run(&["imports", "foo.rs"], json!({"positions": {
        "foo.rs:0:4": ["foo/child.rs"], "foo.rs:1:4": null,
        "foo.rs:1:10": ["foo/child.rs"], "foo.rs:1:18": ["foo/child.rs"], "foo.rs:1:25": ["renamed.rs"]}}));
    assert!(
        out.contains("target=\"renamed.rs\" resolution=lsp"),
        "{out}"
    );
    assert!(!out.contains("target=\"child.rs\""), "{out}");
    assert_eq!(out.matches("resolution=lsp").count(), 4, "{out}");
    assert_eq!(
        fs::read_to_string(f.0.join(".requests")).unwrap(),
        "foo.rs:0:4\nfoo.rs:1:4\nfoo.rs:1:10\nfoo.rs:1:18\nfoo.rs:1:25\n"
    );
}

#[test]
fn dotted_ecmascript_paths_follow_server_even_when_guesses_exist() {
    let f = Fixture::new();
    f.write(
        "main.ts",
        "import x from './foo.bar';\nimport y from './chosen';\nimport('./dynamic');\n",
    );
    for p in [
        "foo.ts",
        "foo.bar.ts",
        "chosen.js",
        "chosen.ts",
        "actual.ts",
    ] {
        f.write(p, "export default 1;\n");
    }
    let out = f.run(&["imports", "main.ts"], json!({"positions": {
        "main.ts:0:15": ["foo.bar.ts", "foo.bar.ts"], "main.ts:1:15": ["actual.ts"], "main.ts:2:8": ["actual.ts"]}}));
    assert!(out.contains("local=3 external=0 unresolved=0"), "{out}");
    assert!(out.contains("target=\"foo.bar.ts\""), "{out}");
    assert!(!out.contains("target=\"foo.ts\""), "{out}");
}

#[test]
fn capability_required_even_for_empty_import_inventory() {
    let f = Fixture::new();
    f.write("empty.py", "pass\n");
    for capabilities in [
        json!({}),
        json!({"definitionProvider":false}),
        json!({"documentSymbolProvider":true}),
    ] {
        let out = lsp(
            &f.0,
            &["imports", "empty.py"],
            json!({"capabilities": capabilities}),
        );
        assert_eq!(out.status.code(), Some(3), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("definitionProvider"),
            "{out:?}"
        );
    }
    let missing = Command::new(env!("CARGO_BIN_EXE_pira_nav"))
        .current_dir(&f.0)
        .args(["imports", "empty.py"])
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--lsp"));
}

#[test]
fn unknown_and_conflicting_definitions_are_not_complete_negative_edges() {
    let f = Fixture::new();
    f.write("main.py", "import a\nimport b\nimport c\nfrom d import *\n");
    f.write("a.py", "");
    f.write("b.py", "");
    let config = json!({"positions": {"main.py:0:7": null, "main.py:1:7": ["a.py", "b.py"], "main.py:2:7": ["generated://c"]}});
    let out = f.run(&["imports", "main.py"], config.clone());
    for state in ["unresolved", "ambiguous", "unsupported"] {
        assert!(out.contains(&format!("resolution={state}")), "{out}");
    }
    assert!(out.contains("wildcard-members-not-enumerated"), "{out}");
    let reverse = f.run(&["dependents", "a.py"], config);
    assert!(reverse.contains("unresolved=5 count=0"), "{reverse}");
}

#[test]
fn server_targets_cannot_escape_root_or_traverse_symlinks() {
    let f = Fixture::new();
    f.write("main.py", "import linked\nimport escaped\n");
    f.write("real.py", "");
    std::os::unix::fs::symlink("real.py", f.0.join("linked.py")).unwrap();
    let config = json!({"positions":{"main.py:0:7":["linked.py"],"main.py:1:7":["../escape.py"]}});
    let out = f.run(&["imports", "main.py"], config.clone());
    assert_eq!(out.matches("resolution=blocked").count(), 2, "{out}");
    let graph = f.run(&["deps", "main.py"], config);
    assert!(graph.contains("edges=0"), "{graph}");
}

#[test]
fn forced_server_inventory_round_trips_and_does_not_merge_native_names() {
    let f = Fixture::new();
    f.write("source.py", "def native(): pass\n");
    let out = f.run(&["outline", "source.py", "--selectors"], json!({}));
    assert!(!out.contains("function native"), "{out}");
    let selector = out
        .split_whitespace()
        .find_map(|v| v.strip_prefix("selector="))
        .unwrap();
    for target in ["source.py::server_only", selector] {
        assert!(f.run(&["show", target], json!({})).contains("def native()"));
        assert!(
            f.run(&["definition", target], json!({}))
                .contains("count=0")
        );
    }
    assert!(
        f.run(&["symbols", "server_only", "source.py"], json!({}))
            .contains("server_only")
    );
    assert!(f.run(&["map", "."], json!({})).contains("server_only"));
}

#[test]
fn forced_server_bypasses_native_depth_failure_in_all_inventory_paths() {
    let f = Fixture::new();
    f.write(
        "source.py",
        &format!(
            "def native(): pass\nx = {}1{}\n",
            "[".repeat(300),
            "]".repeat(300)
        ),
    );
    let native = Command::new(env!("CARGO_BIN_EXE_pira_nav"))
        .current_dir(&f.0)
        .args(["outline", "source.py", "--native"])
        .output()
        .unwrap();
    assert!(!native.status.success());
    assert!(
        String::from_utf8_lossy(&native.stderr).contains("nesting"),
        "{native:?}"
    );
    for args in [
        vec!["outline", "source.py"],
        vec!["map", "."],
        vec!["symbols", "server_only", "source.py"],
        vec!["show", "source.py::server_only"],
        vec!["definition", "source.py::server_only"],
    ] {
        f.run(&args, json!({}));
    }
}

#[test]
fn language_qualified_override_preserves_other_native_inventories() {
    let f = Fixture::new();
    f.write("source.py", "def native(): pass\n");
    f.write("source.rs", "fn native_rs() {}\n");
    f.write(".nav-lsp.json", "{}");
    let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/semantic_server.py");
    let output = Command::new(env!("CARGO_BIN_EXE_pira_nav"))
        .current_dir(&f.0)
        .args([
            "map",
            ".",
            "--lsp",
            "python=/usr/bin/env",
            "--lsp-arg",
            "python=python3",
            "--lsp-arg",
        ])
        .arg(format!("python={}", server.display()))
        .output()
        .unwrap();
    let text = success(output);
    assert!(text.contains("server_only"), "{text}");
    assert!(text.contains("native_rs"), "{text}");
}

fn selector_rows(outline: &str) -> Vec<&str> {
    outline
        .split_whitespace()
        .filter_map(|part| part.strip_prefix("selector="))
        .collect()
}

fn server_symbol(name: &str, row: usize, width: usize, column: usize) -> Value {
    json!({"name":name,"kind":12,
        "range":{"start":{"line":row,"character":0},"end":{"line":row,"character":width}},
        "selectionRange":{"start":{"line":row,"character":column},"end":{"line":row,"character":column+name.len()}}})
}

#[test]
fn semantic_selector_hash_case_matches_show() {
    let f = Fixture::new();
    let source = "int answer(void) { return 42; }";
    f.write("a.c", source);
    let config = json!({"symbols":[server_symbol("answer",0,source.len(),4)]});
    let outline = f.run(&["outline", "a.c", "--selectors"], config.clone());
    let selector = selector_rows(&outline)[0];
    let (prefix, hash) = selector.rsplit_once('@').unwrap();
    let upper = format!("{prefix}@{}", hash.to_ascii_uppercase());
    assert_ne!(upper, selector);
    assert!(f.run(&["show", &upper], config.clone()).contains(source));
    f.run(&["definition", &upper], config.clone());
    assert_eq!(
        fs::read_to_string(f.0.join(".requests")).unwrap(),
        "a.c:0:4\n"
    );
    let malformed = format!("{prefix}@not-a-valid-hash!");
    assert_eq!(
        lsp(&f.0, &["definition", &malformed], config).status.code(),
        Some(2)
    );
}

#[test]
fn selector_hash_disambiguates_overloads_but_names_remain_ambiguous() {
    let f = Fixture::new();
    let lines = [
        "int twice(int x) { return x * 2; }",
        "double twice(double x) { return x * 2; }",
    ];
    f.write("overloads.cpp", &format!("{}\n{}\n", lines[0], lines[1]));
    let config = json!({"symbols":[server_symbol("twice",0,lines[0].len(),4),server_symbol("twice",1,lines[1].len(),7)]});
    let outline = f.run(&["outline", "overloads.cpp", "--selectors"], config.clone());
    let selectors = selector_rows(&outline);
    assert_eq!(selectors.len(), 2);
    for (index, selector) in selectors.iter().enumerate() {
        assert!(
            f.run(&["show", selector], config.clone())
                .contains(lines[index])
        );
        f.run(&["definition", selector], config.clone());
    }
    assert_eq!(
        fs::read_to_string(f.0.join(".requests")).unwrap(),
        "overloads.cpp:0:4\noverloads.cpp:1:7\n"
    );
    let named = lsp(&f.0, &["definition", "overloads.cpp::twice"], config);
    assert_eq!(named.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&named.stderr).contains("ambiguous"));
}

#[test]
fn selectors_supply_language_for_headers_and_explicit_suffix_overrides() {
    let f = Fixture::new();
    let source = "int helper(void);";
    let config = json!({"symbols":[server_symbol("helper",0,source.len(),4)]});
    for file in ["dep.h", "dep.txt"] {
        f.write(file, &format!("{source}\n"));
        let outline = f.run(
            &["outline", file, "--language", "c", "--selectors"],
            config.clone(),
        );
        let selectors = selector_rows(&outline);
        assert_eq!(selectors.len(), 1);
        for command in ["show", "definition"] {
            f.run(&[command, selectors[0]], config.clone());
            f.run(&[command, selectors[0], "--language", "c"], config.clone());
            let conflict = lsp(
                &f.0,
                &[command, selectors[0], "--language", "cpp"],
                config.clone(),
            );
            assert_eq!(conflict.status.code(), Some(2), "{conflict:?}");
            assert!(
                String::from_utf8_lossy(&conflict.stderr).contains("language mismatch"),
                "{conflict:?}"
            );
        }
    }
}

#[test]
fn stale_semantic_selectors_use_exit_four_for_changed_renamed_and_removed_items() {
    let f = Fixture::new();
    let original = "int helper(void) { return 1; }";
    let config = json!({"symbols":[server_symbol("helper",0,original.len(),4)]});
    f.write("source.c", &format!("{original}\n"));
    let outline = f.run(&["outline", "source.c", "--selectors"], config.clone());
    let selector = selector_rows(&outline)[0];
    for (source, symbols) in [
        ("int helper(void) { return 2; }", config["symbols"].clone()),
        (
            "int renamed(void) { return 1; }",
            json!([server_symbol("renamed", 0, 30, 4)]),
        ),
        ("// removed", json!([])),
    ] {
        f.write("source.c", &format!("{source}\n"));
        for command in ["show", "definition"] {
            let out = lsp(&f.0, &[command, selector], json!({"symbols":symbols}));
            assert_eq!(out.status.code(), Some(4), "{command}: {out:?}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("stale selector"),
                "{out:?}"
            );
        }
    }
    assert!(
        !f.0.join(".requests").exists(),
        "stale identities must fail before definition requests"
    );
}
