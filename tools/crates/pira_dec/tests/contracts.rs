#![cfg(any(target_os = "macos", target_os = "linux", windows))]

use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "fixtures/v1.rs"]
mod v1;

const OLD: &str = "D-19700101-000001-0000000000000001";
const LOW: &str = "D-19700101-000002-0000000000000002";
const HIGH: &str = "D-19700101-000002-0000000000000003";
const MISSING: &str = "D-19700101-000003-0000000000000004";
// Frozen v1 bytes, not output from the current encoder. See fixtures/README.md.
const FIXTURES: [(&str, &[u8]); 3] = [(OLD, v1::OLD), (HIGH, v1::HIGH), (LOW, v1::LOW)];
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pira-dec-contracts-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pira_dec"));
        cmd.env_clear().current_dir(&self.0);
        for key in ["SYSTEMROOT", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        cmd.env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
            .env("LOCALAPPDATA", &self.0)
            .arg(args[0])
            .arg("--store-dir")
            .arg(self.0.join("store"))
            .args(&args[1..]);
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn add(&self, context: &str, choices: &[&str], decision: &str) -> String {
        let args = add_args(context, choices, decision);
        let out = self.run(&args);
        assert!(out.status.success(), "{out:?}");
        assert!(out.stderr.is_empty(), "{out:?}");
        String::from_utf8(out.stdout)
            .unwrap()
            .split(" | ")
            .next()
            .unwrap()
            .to_owned()
    }

    fn records(&self) -> PathBuf {
        let namespaces: Vec<_> = fs::read_dir(self.0.join("store")).unwrap().collect();
        assert_eq!(namespaces.len(), 1);
        namespaces[0].as_ref().unwrap().path().join("records")
    }

    fn seeded() -> Self {
        let s = Self::new();
        // Let the real CLI establish the physical workspace and private store.
        let seed = s.add("initialize", &["one", "two"], "1");
        assert!(s.run(&["forget", &seed, "--yes"]).status.success());
        for (id, bytes) in FIXTURES {
            let path = s.records().join(format!("{id}.piradec"));
            fs::write(&path, bytes).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        s
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn add_args<'a>(context: &'a str, choices: &[&'a str], decision: &'a str) -> Vec<&'a str> {
    let mut args = vec!["add", "--context", context];
    for choice in choices {
        args.extend(["--choice", choice]);
    }
    args.extend(["--decision", decision, "--maker", "human"]);
    args
}

fn expected(id: &str) -> Value {
    let (ms, time, context, choices, decision, maker, supersedes, related) = match id {
        OLD => (
            1000,
            "1970-01-01T00:00:01.000Z",
            "First context",
            ["Keep one", "Drop one"],
            1,
            "agent",
            None,
            vec![],
        ),
        LOW => (
            2000,
            "1970-01-01T00:00:02.000Z",
            "Second <context> & detail",
            ["Reject second", "Choose second"],
            2,
            "human",
            None,
            vec![OLD],
        ),
        HIGH => (
            2000,
            "1970-01-01T00:00:02.000Z",
            "Third context",
            ["Keep third", "Drop third"],
            1,
            "agent",
            Some(OLD),
            vec![],
        ),
        _ => panic!("unknown fixture {id}"),
    };
    json!({"id":id, "timestamp_ms":ms, "timestamp":time, "context":context,
        "choices":choices, "decision":decision, "decision_text":choices[decision - 1],
        "maker":maker, "supersedes":supersedes, "related":related})
}

fn ids(value: &Value, field: &str) -> Vec<String> {
    value[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn frozen_v1_records_are_readable_without_rewriting() {
    let s = Sandbox::seeded();
    for (id, bytes) in FIXTURES {
        assert_eq!(s.json(&["show", id, "--json"]), expected(id));
        assert_eq!(
            fs::read(s.records().join(format!("{id}.piradec"))).unwrap(),
            bytes
        );
    }
}

#[test]
fn show_resolves_unique_prefixes_and_distinguishes_missing_and_ambiguous_ids() {
    let s = Sandbox::seeded();
    assert_eq!(s.json(&["show", &OLD[..17], "--json"]), expected(OLD));
    for (query, message) in [("D-", "ambiguous"), (MISSING, "no decision matches")] {
        let out = s.run(&["show", query, "--json"]);
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8(out.stderr).unwrap().contains(message));
    }
}

#[test]
fn forget_rejects_even_unique_prefixes_without_deleting_and_reports_missing_exact_ids() {
    let s = Sandbox::seeded();
    let out = s.run(&["forget", &OLD[..17], "--yes"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("invalid decision ID")
    );
    assert_eq!(s.json(&["show", OLD, "--json"]), expected(OLD));
    assert!(s.run(&["forget", OLD, "--yes"]).status.success());
    let missing = s.run(&["forget", OLD, "--yes"]);
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("no exact decision")
    );
    assert_eq!(ids(&s.json(&["list", "--json"]), "decisions"), [HIGH, LOW]);
}

#[test]
fn invalid_records_reject_direct_operations_but_are_reported_and_skipped_by_reads() {
    for (kind, bytes, diagnostic) in [
        ("header", b"corrupt".to_vec(), "header"),
        (
            "identity",
            FIXTURES[0].1.to_vec(),
            "filename does not match embedded ID",
        ),
        (
            "checksum",
            {
                let mut b = FIXTURES[2].1.to_vec();
                b[20] ^= 1;
                b
            },
            "checksum",
        ),
        ("size", vec![0; 65_537], "size limit"),
    ] {
        let s = Sandbox::seeded();
        let path = s.records().join(format!("{LOW}.piradec"));
        fs::write(&path, &bytes).unwrap();
        let mut related = add_args("invalid target", &["one", "two"], "1");
        related.extend(["--related", LOW]);
        for args in [
            vec!["show", LOW, "--json"],
            vec!["forget", LOW, "--yes"],
            related,
        ] {
            let out = s.run(&args);
            assert_eq!(out.status.code(), Some(2), "{kind}: {out:?}");
            assert!(out.stdout.is_empty(), "{kind}");
            assert!(
                String::from_utf8(out.stderr).unwrap().contains(diagnostic),
                "{kind}"
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                bytes,
                "failed operation changed {kind}"
            );
        }
        for (args, field) in [
            (vec!["list", "--json"], "decisions"),
            (vec!["search", "context", "--json"], "matches"),
        ] {
            let out = s.run(&args);
            assert!(out.status.success(), "{kind}: {out:?}");
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(ids(&value, field), [HIGH, OLD], "{kind}");
            assert_eq!(value["skipped_count"], 1);
            assert_eq!(value["has_more"], false);
            assert_eq!(value["skipped"][0]["filename"], format!("{LOW}.piradec"));
            assert!(
                value["skipped"][0]["error"]
                    .as_str()
                    .unwrap()
                    .contains(diagnostic)
            );
            assert!(String::from_utf8(out.stderr).unwrap().contains(diagnostic));
        }
    }
}

#[cfg(unix)]
#[test]
fn symlinked_record_is_not_read_or_deleted() {
    use std::os::unix::fs::symlink;
    let s = Sandbox::seeded();
    let target = s.records().join(format!("{OLD}.piradec"));
    let link = s.records().join(format!("{LOW}.piradec"));
    fs::remove_file(&link).unwrap();
    symlink(&target, &link).unwrap();
    for args in [vec!["show", LOW], vec!["forget", LOW, "--yes"]] {
        let out = s.run(&args);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(
            String::from_utf8(out.stderr)
                .unwrap()
                .contains("symlinked decision record")
        );
    }
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(target).unwrap(), FIXTURES[0].1);
}

#[test]
fn regex_search_uses_the_requested_field_and_case_mode() {
    let s = Sandbox::seeded();
    for (field, pattern, want) in [
        ("id", format!("^{OLD}$"), vec![OLD]),
        ("context", "^Second".into(), vec![LOW]),
        ("context", "^second".into(), vec![]),
        ("context", "(?i)^second".into(), vec![LOW]),
        ("choice", "^Reject second$".into(), vec![LOW]),
        ("decision", "^Reject second$".into(), vec![]),
        ("decision", "^Choose second$".into(), vec![LOW]),
        ("maker", "^human$".into(), vec![LOW]),
        ("relation", format!("^{OLD}$"), vec![HIGH, LOW]),
        (
            "timestamp",
            r"^1970-01-01T00:00:01\.000Z$".into(),
            vec![OLD],
        ),
    ] {
        let out = s.run(&["search", "--field", field, "--regex", &pattern, "--json"]);
        assert_eq!(
            out.status.code(),
            Some(if want.is_empty() { 1 } else { 0 }),
            "{field}: {out:?}"
        );
        assert!(out.stderr.is_empty(), "{out:?}");
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(ids(&value, "matches"), want, "{field} {pattern}");
    }
    let out = s.run(&["search", "--field", "choice", "--regex", "^Reject second$"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("match=Reject second")
    );
}

#[test]
fn search_rejects_invalid_regex_and_field_instead_of_reporting_no_matches() {
    let s = Sandbox::seeded();
    for (field, regex, message) in [
        ("context", "[", "invalid regex"),
        ("unknown", "x", "unknown search field"),
    ] {
        let out = s.run(&["search", "--field", field, "--regex", regex]);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8(out.stderr).unwrap().contains(message));
    }
}

#[test]
fn literal_dash_prefixed_query_is_not_interpreted_as_help() {
    let s = Sandbox::new();
    let id = s.add("literal --help token", &["keep", "drop"], "1");
    assert_eq!(
        ids(&s.json(&["search", "--json", "--", "--help"]), "matches"),
        [id]
    );
}

#[test]
fn list_and_time_only_search_apply_inclusive_exclusive_bounds_before_limit() {
    let s = Sandbox::seeded();
    for (command, field) in [("list", "decisions"), ("search", "matches")] {
        for (since, until, limit, want, more) in [
            (
                "1970-01-01T00:00:01Z",
                "1970-01-01T00:00:02Z",
                "1",
                vec![OLD],
                false,
            ),
            (
                "1970-01-01T00:00:02Z",
                "1970-01-01T00:00:03Z",
                "1",
                vec![HIGH],
                true,
            ),
            (
                "1970-01-01T00:00:02Z",
                "1970-01-01T00:00:03Z",
                "2",
                vec![HIGH, LOW],
                false,
            ),
            (
                "1970-01-01T00:00:03Z",
                "1970-01-01T00:00:04Z",
                "1",
                vec![],
                false,
            ),
        ] {
            let out = s.run(&[
                command, "--since", since, "--until", until, "--limit", limit, "--json",
            ]);
            assert_eq!(
                out.status.code(),
                Some(if command == "search" && want.is_empty() {
                    1
                } else {
                    0
                }),
                "{out:?}"
            );
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(ids(&value, field), want, "{command} {since}..{until}");
            assert_eq!(value["has_more"], more);
            assert!(out.stderr.is_empty(), "{out:?}");
            if command == "list" {
                for (row, id) in value[field].as_array().unwrap().iter().zip(&want) {
                    let record = expected(id);
                    assert_eq!(
                        row,
                        &json!({"id":id, "timestamp":record["timestamp"], "decision":record["decision_text"]})
                    );
                }
            }
        }
    }
    let out = s.run(&["list", "--since", "1970-01-01T00:00:03Z"]);
    assert!(out.status.success());
    assert_eq!(out.stdout, b"decisions=0\n");
}

#[test]
fn export_exposes_each_selected_record_in_order_with_full_semantics() {
    let s = Sandbox::seeded();
    for (index, (filters, want)) in [
        (vec![], vec![HIGH, LOW, OLD]),
        (vec!["--limit", "1"], vec![HIGH]),
        (
            vec![
                "--since",
                "1970-01-01T00:00:01Z",
                "--until",
                "1970-01-01T00:00:02Z",
            ],
            vec![OLD],
        ),
        (
            vec![
                "--since",
                "1970-01-01T00:00:02Z",
                "--until",
                "1970-01-01T00:00:03Z",
                "--limit",
                "2",
            ],
            vec![HIGH, LOW],
        ),
        (vec!["--since", "1970-01-01T00:00:03Z"], vec![]),
    ]
    .into_iter()
    .enumerate()
    {
        let filename = format!("export-{index}.html");
        let mut args = vec!["export", "--output", &filename];
        args.extend(filters);
        let out = s.run(&args);
        assert!(out.status.success(), "{out:?}");
        let output = s.0.join(filename);
        let html = fs::read_to_string(&output).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let articles: Vec<_> = html
            .split("<article ")
            .skip(1)
            .map(|part| part.split("</article>").next().unwrap())
            .collect();
        assert_eq!(articles.len(), want.len());
        if !want.is_empty() {
            let index = html
                .split("<details class=\"index\">")
                .nth(1)
                .expect("decision index")
                .split("</details>")
                .next()
                .unwrap();
            let index_ids: Vec<_> = index
                .split("href=\"#")
                .skip(1)
                .map(|link| link.split('"').next().unwrap())
                .collect();
            assert_eq!(
                index_ids, want,
                "index must link every exported record in order"
            );
        }
        for (article, id) in articles.iter().zip(&want) {
            let record = expected(id);
            assert!(article.contains(&format!("id=\"{id}\"")));
            let context = if *id == LOW {
                "Second &lt;context&gt; &amp; detail"
            } else {
                record["context"].as_str().unwrap()
            };
            assert!(article.contains(context));
            for choice in record["choices"].as_array().unwrap() {
                assert!(article.contains(choice.as_str().unwrap()));
            }
            assert!(article.contains(&format!(
                "<strong>{}</strong>",
                record["decision_text"].as_str().unwrap()
            )));
            assert!(article.contains(&format!(
                "class=\"maker\">{}<",
                record["maker"].as_str().unwrap()
            )));
            assert!(article.contains(&format!(
                "datetime=\"{}\"",
                record["timestamp"].as_str().unwrap()
            )));
        }
        assert!(
            !html.contains(s.0.to_str().unwrap()),
            "workspace path leaked"
        );
        if want.is_empty() {
            assert!(html.contains("No decisions in this export"));
        }
    }
}

#[test]
fn skipped_record_details_are_complete_even_when_human_warnings_are_bounded() {
    let s = Sandbox::seeded();
    for index in 0..9 {
        fs::write(s.records().join(format!("bad-{index}.piradec")), b"invalid").unwrap();
    }
    let out = s.run(&["search", "will-not-match", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["skipped_count"], 9);
    assert_eq!(value["skipped"].as_array().unwrap().len(), 9);
    assert_eq!(value["has_more"], false);
    let warnings = String::from_utf8(out.stderr).unwrap();
    assert_eq!(warnings.lines().count(), 9);
    assert!(warnings.contains("skipped=9 warnings_omitted=1"));
    let out = s.run(&["search", "will-not-match"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, b"decisions_matched=0 complete=0 skipped=9\n");
    assert!(
        s.run(&["export", "--output", "skipped.html"])
            .status
            .success()
    );
    let html = fs::read_to_string(s.0.join("skipped.html")).unwrap();
    assert!(html.contains("9 invalid records were skipped"));
    assert_eq!(html.matches("<article ").count(), 3);
}

#[test]
fn add_normalizes_text_and_persists_the_one_based_selection() {
    let s = Sandbox::new();
    let id = s.add("  context 前\n", &[" first ", "\tsecond 前  "], "2");
    let value = s.json(&["show", &id, "--json"]);
    assert_eq!(value["context"], "context 前");
    assert_eq!(value["choices"], json!(["first", "second 前"]));
    assert_eq!(value["decision"], 2);
    assert_eq!(value["decision_text"], "second 前");
}

#[test]
fn add_rejections_do_not_publish_a_record_or_change_existing_decisions() {
    let s = Sandbox::new();
    let existing = s.add("existing context", &["first", "selected"], "2");
    let before = s.json(&["show", &existing, "--json"]);
    let baseline = s.json(&["list", "--json"]);
    let mut missing_maker = add_args("valid", &["one", "two"], "1");
    missing_maker.truncate(missing_maker.len() - 2);
    let mut missing_context = add_args("valid", &["one", "two"], "1");
    missing_context.drain(1..3);
    let mut missing_decision = add_args("valid", &["one", "two"], "1");
    let position = missing_decision
        .iter()
        .position(|arg| *arg == "--decision")
        .unwrap();
    missing_decision.drain(position..position + 2);
    let mut unknown_maker = add_args("valid", &["one", "two"], "1");
    *unknown_maker.last_mut().unwrap() = "robot";
    let mut repeated_context = add_args("valid", &["one", "two"], "1");
    repeated_context.extend(["--context", "another"]);
    for (name, args, message) in [
        ("maker", missing_maker, "requires --maker"),
        ("missing context", missing_context, "requires --context"),
        ("missing decision", missing_decision, "requires --decision"),
        ("unknown maker", unknown_maker, "invalid maker"),
        (
            "non-integer index",
            add_args("valid", &["one", "two"], "x"),
            "unsigned integer",
        ),
        (
            "context singleton",
            repeated_context,
            "--context may appear only once",
        ),
        (
            "empty context",
            add_args(" \t", &["one", "two"], "1"),
            "context must not be empty",
        ),
        (
            "one choice",
            add_args("valid", &["one"], "1"),
            "2 through 32 choices",
        ),
        (
            "empty choice",
            add_args("valid", &["one", " \n"], "1"),
            "choices must not be empty",
        ),
        (
            "duplicate",
            add_args("valid", &["one", " one "], "1"),
            "unique after trimming",
        ),
        (
            "zero index",
            add_args("valid", &["one", "two"], "0"),
            "one-based",
        ),
        (
            "past last index",
            add_args("valid", &["one", "two"], "3"),
            "one-based",
        ),
    ] {
        let out = s.run(&args);
        assert_eq!(out.status.code(), Some(2), "{name}: {out:?}");
        assert!(
            String::from_utf8(out.stderr).unwrap().contains(message),
            "{name}"
        );
        assert_eq!(s.json(&["list", "--json"]), baseline, "{name}");
        assert_eq!(s.json(&["show", &existing, "--json"]), before, "{name}");
    }
}

#[test]
fn add_accepts_utf8_byte_and_choice_count_limits_but_rejects_the_next_value() {
    for (field, maximum, prefix) in [
        ("context", 16_384, "前".repeat(5461)),
        ("choice", 4096, "前".repeat(1365)),
    ] {
        for extra in [0, 1] {
            let s = Sandbox::new();
            let text = format!("{prefix}{}", "x".repeat(1 + extra));
            assert_eq!(text.len(), maximum + extra);
            let args = if field == "context" {
                add_args(&text, &["one", "two"], "1")
            } else {
                add_args("valid", &[&text, "two"], "1")
            };
            let out = s.run(&args);
            assert_eq!(
                out.status.code(),
                Some(if extra == 0 { 0 } else { 2 }),
                "{field}: {out:?}"
            );
            if extra == 1 {
                assert!(
                    String::from_utf8(out.stderr)
                        .unwrap()
                        .contains(&format!("{field} exceeds {maximum} UTF-8 bytes"))
                );
                assert!(ids(&s.json(&["list", "--json"]), "decisions").is_empty());
            }
        }
    }
    for count in [32, 33] {
        let s = Sandbox::new();
        let choices: Vec<_> = (0..count).map(|i| format!("choice-{i}")).collect();
        let choices: Vec<_> = choices.iter().map(String::as_str).collect();
        let out = s.run(&add_args("valid", &choices, "1"));
        assert_eq!(
            out.status.code(),
            Some(if count == 32 { 0 } else { 2 }),
            "{out:?}"
        );
        if count == 33 {
            assert!(
                String::from_utf8(out.stderr)
                    .unwrap()
                    .contains("2 through 32 choices")
            );
        }
    }
}

#[test]
fn add_rejects_invalid_relationships_without_mutating_targets() {
    let s = Sandbox::seeded();
    let baseline = s.json(&["list", "--json"]);
    for (flags, message) in [
        (vec!["--related", MISSING], "does not exist"),
        (vec!["--related", &OLD[..17]], "invalid decision ID"),
        (vec!["--related", OLD, "--related", OLD], "must be unique"),
        (
            vec!["--supersedes", OLD, "--related", OLD],
            "both superseded and related",
        ),
        (
            vec!["--supersedes", OLD, "--supersedes", LOW],
            "may appear only once",
        ),
    ] {
        let mut args = add_args("relations", &["one", "two"], "1");
        args.extend(flags);
        let out = s.run(&args);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(String::from_utf8(out.stderr).unwrap().contains(message));
        assert_eq!(s.json(&["list", "--json"]), baseline);
        for (id, _) in FIXTURES {
            assert_eq!(s.json(&["show", id, "--json"]), expected(id));
        }
        assert_eq!(
            fs::read_dir(s.records().parent().unwrap().join(".tmp"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn add_enforces_the_complete_record_byte_limit() {
    // V1 framing plus these non-choice payloads and 17 choice headers use 206 bytes.
    // 16 * 4000 + 1330 bytes of alternatives make an exactly 65536-byte record.
    for extra in [0, 1] {
        let s = Sandbox::new();
        let mut choices: Vec<_> = (0..16)
            .map(|i| format!("{i:02}{}", "x".repeat(3998)))
            .collect();
        choices.push("z".repeat(1330 + extra));
        let choices: Vec<_> = choices.iter().map(String::as_str).collect();
        let out = s.run(&add_args("valid", &choices, "1"));
        assert_eq!(
            out.status.code(),
            Some(if extra == 0 { 0 } else { 2 }),
            "{out:?}"
        );
        if extra == 0 {
            let text = String::from_utf8(out.stdout).unwrap();
            let id = text.split(" | ").next().unwrap();
            assert_eq!(
                fs::metadata(s.records().join(format!("{id}.piradec")))
                    .unwrap()
                    .len(),
                65536
            );
            assert_eq!(s.json(&["show", id, "--json"])["choices"], json!(choices));
        } else {
            assert!(
                String::from_utf8(out.stderr)
                    .unwrap()
                    .contains("encoded decision exceeds 65536 bytes")
            );
            assert!(ids(&s.json(&["list", "--json"]), "decisions").is_empty());
        }
        assert_eq!(
            fs::read_dir(s.records().parent().unwrap().join(".tmp"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn staged_concurrent_adds_are_invisible_until_published_and_neither_write_is_lost() {
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    struct Children(Vec<Child>);
    impl Drop for Children {
        fn drop(&mut self) {
            for child in &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    fn until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for Dec writers"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    let s = Sandbox::seeded();
    let workspace = s.records().parent().unwrap().to_path_buf();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(workspace.join(".write.lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut children = Children(Vec::new());
    for context in ["simultaneous one", "simultaneous two"] {
        children.0.push(
            s.command(&add_args(context, &["keep", "drop"], "2"))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    // Two staged paths establish overlapping writes while publication is blocked.
    until(|| fs::read_dir(workspace.join(".tmp")).unwrap().count() == 2);
    assert_eq!(
        ids(&s.json(&["list", "--json"]), "decisions"),
        [HIGH, LOW, OLD]
    );
    drop(lock);
    until(|| {
        children
            .0
            .iter_mut()
            .all(|child| child.try_wait().unwrap().is_some())
    });
    let mut published = Vec::new();
    for (child, context) in children
        .0
        .drain(..)
        .zip(["simultaneous one", "simultaneous two"])
    {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(out.stderr.is_empty(), "{out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        let id = text.split(" | ").next().unwrap();
        let record = s.json(&["show", id, "--json"]);
        assert_eq!(record["context"], context);
        assert_eq!(record["choices"], json!(["keep", "drop"]));
        assert_eq!(record["decision_text"], "drop");
        published.push(id.to_owned());
    }
    let all = ids(&s.json(&["list", "--json"]), "decisions");
    assert_eq!(all.len(), 5);
    assert_ne!(published[0], published[1]);
    assert!(published.iter().all(|id| all.contains(id)));
    assert_eq!(fs::read_dir(workspace.join(".tmp")).unwrap().count(), 0);
}
