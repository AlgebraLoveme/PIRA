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

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn isolated_add(sandbox: &Sandbox) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pira_dec"));
    command
        .current_dir(sandbox.path())
        .env_remove("PIRA_DEC_STORE_DIR")
        .env_remove("XDG_DATA_HOME")
        .env("HOME", sandbox.path().join("home"))
        .env("LOCALAPPDATA", sandbox.path().join("local"))
        .args([
            "add",
            "--context",
            "root selection",
            "--choice",
            "one",
            "--choice",
            "two",
            "--decision",
            "1",
            "--maker",
            "agent",
        ]);
    command
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn assert_one_published_record(root: &Path) {
    let workspaces: Vec<_> = fs::read_dir(root).unwrap().collect();
    assert_eq!(workspaces.len(), 1);
    let records = workspaces[0].as_ref().unwrap().path().join("records");
    assert_eq!(fs::read_dir(records).unwrap().count(), 1);
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[test]
fn platform_default_store_root() {
    #[cfg(target_os = "linux")]
    let cases = [None, Some(""), Some("relative"), Some("absolute")];
    #[cfg(not(target_os = "linux"))]
    let cases = [Some("absolute")]; // XDG must not override macOS/Windows defaults.
    for case in cases {
        let sandbox = Sandbox::new();
        let xdg = sandbox.path().join("xdg-数据");
        let mut command = isolated_add(&sandbox);
        if let Some(value) = case {
            command.env(
                "XDG_DATA_HOME",
                if value == "absolute" {
                    xdg.as_os_str()
                } else {
                    value.as_ref()
                },
            );
        }
        #[cfg(target_os = "macos")]
        let expected = sandbox
            .path()
            .join("home/Library/Application Support/PIRA/decision");
        #[cfg(windows)]
        let expected = sandbox.path().join("local/PIRA/decision");
        #[cfg(target_os = "linux")]
        let expected = if case == Some("absolute") {
            xdg.join("pira/decision")
        } else {
            sandbox.path().join("home/.local/share/pira/decision")
        };
        let output = command.output().unwrap();
        assert!(output.status.success(), "case={case:?}: {output:?}");
        assert_one_published_record(&expected);
        assert!(!sandbox.path().join("relative").exists());
        assert!(!sandbox.path().join("pira").exists());
        if case != Some("absolute") || !cfg!(target_os = "linux") {
            assert!(!xdg.exists());
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[test]
fn explicit_store_overrides_precede_platform_defaults() {
    for cli in [false, true] {
        let sandbox = Sandbox::new();
        let environment = sandbox.path().join("override-数据");
        let mut command = isolated_add(&sandbox);
        command.env("PIRA_DEC_STORE_DIR", &environment);
        if cli {
            command.args(["--store-dir", "relative-cli"]);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_one_published_record(&if cli {
            sandbox.path().join("relative-cli")
        } else {
            environment.clone()
        });
        if cli {
            assert!(!environment.exists());
        }
        assert!(!sandbox.path().join("home").exists());
        assert!(!sandbox.path().join("local").exists());
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

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[test]
fn limits_are_disclosed_and_context_matches_are_visible() {
    let s = Sandbox::new();
    let mut ids = Vec::new();
    // Two matches distinguish a full one-row page from the exact-limit boundary.
    for context in ["older cache rationale", "newer cache rationale"] {
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
            "search", "--field", "context", "--regex", "cache", "--limit", "1",
        ],
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("has_more=1") && text.contains("match="),
        "{text}"
    );
    let out = run(&s, &["list", "--limit", "1", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["has_more"], true);
    assert_eq!(json["decisions"].as_array().unwrap().len(), 1);
    let out = run(
        &s,
        &[
            "search", "--field", "context", "--regex", "cache", "--limit", "2", "--json",
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

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[test]
fn literal_search_matches_all_choices_and_context_once_without_regex_semantics() {
    let s = Sandbox::new();
    for (context, selected, alternative) in [
        ("Cache[1] rationale", "Use CACHE[1]", "Other"),
        ("Choose storage", "Use disk", "Use cache[1]"),
        ("Choose latency", "Use Cache[1]", "Use memory"),
        ("Cache1 is not the literal", "Keep", "Drop"),
    ] {
        let out = run(
            &s,
            &[
                "add",
                "--context",
                context,
                "--choice",
                selected,
                "--choice",
                alternative,
                "--decision",
                "1",
                "--maker",
                "agent",
            ],
        );
        assert!(out.status.success(), "{:?}", out);
    }
    let out = run(&s, &["search", "cAcHe[1]", "--since", "1h"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 3, "{text}");
    assert_eq!(text.matches("match=").count(), 3, "{text}");
    assert!(!text.contains("Cache1"));
    println!("literal cross-field output:\n{text}");

    let out = run(&s, &["search", "cache[1]", "--limit", "2", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["matches"].as_array().unwrap().len(), 2);
    assert_eq!(json["has_more"], true);
    let out = run(&s, &["search", "cache[1]", "--until", "1h"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "decisions_matched=0 complete=1\n"
    );

    // Existing field-specific regex remains case-sensitive and field-restricted.
    let out = run(
        &s,
        &["search", "--field", "context", "--regex", "^Cache[1]"],
    );
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains("Cache1 is not the literal"), "{text}");
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[test]
fn literal_search_has_bounded_unicode_evidence_and_no_match_status() {
    let s = Sandbox::new();
    let context = format!("{}ПРИВЕТ{}", "前".repeat(400), "後".repeat(400));
    assert!(
        run(
            &s,
            &[
                "add",
                "--context",
                &context,
                "--choice",
                "Keep",
                "--choice",
                "Drop",
                "--decision",
                "1",
                "--maker",
                "agent"
            ]
        )
        .status
        .success()
    );
    let out = run(&s, &["search", "привет"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("ПРИВЕТ") && text.contains("match="), "{text}");
    assert!(
        text.len() < 1000,
        "excerpt must stay bounded: {} bytes",
        text.len()
    );
    println!("bounded Unicode output:\n{text}");
    let out = run(&s, &["search", "missing"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "decisions_matched=0 complete=1\n"
    );
    assert!(out.stderr.is_empty());
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[test]
fn bidi_rows_are_safe_while_record_json_preserves_text() {
    let s = Sandbox::new();
    let choice = "safe\u{202e}spoof\u{2069}";
    let out = run(
        &s,
        &[
            "add",
            "--context",
            "bidi",
            "--choice",
            choice,
            "--choice",
            "other",
            "--decision",
            "1",
            "--maker",
            "agent",
        ],
    );
    assert!(out.status.success(), "{:?}", out);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("safe\\u{202e}spoof\\u{2069}"), "{text}");
    let id = text.split_whitespace().next().unwrap();
    for args in [vec!["list"], vec!["search", "spoof"]] {
        let out = run(&s, &args);
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(!text.contains('\u{202e}') && !text.contains('\u{2069}'));
        assert!(text.contains("safe\\u{202e}spoof\\u{2069}"));
    }
    let out = run(&s, &["show", id, "--json"]);
    assert!(out.status.success());
    let record: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["choices"][0], choice);
}

#[cfg(unix)]
#[test]
fn invalid_utf8_argument_is_an_ordinary_error() {
    use std::os::unix::ffi::OsStrExt;
    let s = Sandbox::new();
    for bytes in [b"\xff".as_slice(), b"valid-\xfe-tail".as_slice()] {
        let out = Command::new(env!("CARGO_BIN_EXE_pira_dec"))
            .current_dir(s.path())
            .arg("search")
            .arg(std::ffi::OsStr::from_bytes(bytes))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(
            String::from_utf8(out.stderr).unwrap(),
            "pira_dec: command-line arguments must be valid UTF-8\n"
        );
        assert!(out.stdout.is_empty());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn fatal_filename_diagnostics_escape_without_overwriting() {
    let s = Sandbox::new();
    for filename in ["unsafe-\u{1b}[31m\u{202e}.html", "line\nnext\tname.html"] {
        let path = s.path().join(filename);
        fs::write(&path, b"keep").unwrap();
        let out = run(&s, &["export", "--output", path.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(2));
        let error = String::from_utf8(out.stderr).unwrap();
        assert_eq!(error.lines().count(), 1);
        assert!(!error.contains('\u{1b}') && !error.contains('\u{202e}') && !error.contains('\t'));
        let escaped = if filename.starts_with("unsafe") {
            "unsafe-\\u{1b}[31m\\u{202e}.html"
        } else {
            "line\\nnext\\tname.html"
        };
        assert!(error.contains(escaped), "{error}");
        assert_eq!(fs::read(path).unwrap(), b"keep");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
#[test]
fn unsupported_platform_rejects_writes_without_creating_files() {
    let sandbox = Sandbox::new();
    for args in [
        vec![
            "add",
            "--context",
            "test",
            "--choice",
            "one",
            "--choice",
            "two",
            "--decision",
            "1",
            "--maker",
            "agent",
        ],
        vec!["export", "--output", "out.html"],
        vec!["forget", "D-20260717-123953-d48c0473bd052414", "--yes"],
    ] {
        let output = run(&sandbox, &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("supported only on macOS, Linux and Windows")
        );
        assert_eq!(fs::read_dir(sandbox.path()).unwrap().count(), 0);
    }
    assert!(run(&sandbox, &["list"]).status.success());
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[test]
fn private_store_add_export_and_forget_lifecycle() {
    let sandbox = Sandbox::new();
    let added = run(
        &sandbox,
        &[
            "add",
            "--context",
            "lifecycle",
            "--choice",
            "one",
            "--choice",
            "two",
            "--decision",
            "1",
            "--maker",
            "agent",
        ],
    );
    assert!(added.status.success(), "{:?}", added);
    let text = String::from_utf8(added.stdout).unwrap();
    let id = text.split(" | ").next().unwrap();
    let exported = run(&sandbox, &["export", "--output", "export.html"]);
    assert!(exported.status.success(), "{:?}", exported);
    let path = sandbox.path().join("export.html");
    let html = fs::read_to_string(&path).unwrap();
    assert!(html.contains("<!doctype html>") && html.contains(id) && html.contains("lifecycle"));
    assert_eq!(
        run(&sandbox, &["export", "--output", "export.html"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), html);
    // Confirmation remains mandatory and must not remove the record.
    assert_eq!(run(&sandbox, &["forget", id]).status.code(), Some(2));
    assert!(run(&sandbox, &["show", id]).status.success());
    let forgotten = run(&sandbox, &["forget", id, "--yes"]);
    assert!(forgotten.status.success(), "{:?}", forgotten);
    let listed = run(&sandbox, &["list", "--json"]);
    assert!(listed.status.success(), "{:?}", listed);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&listed.stdout).unwrap(),
        serde_json::json!({"decisions": [], "has_more": false, "skipped": [], "skipped_count": 0})
    );
}
