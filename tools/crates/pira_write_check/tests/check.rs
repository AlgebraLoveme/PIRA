use pira_write_check::{ADVISORY, DEFAULT_LEXICON, Lexicon, PROMPT, VERSION};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

fn scan(text: &str, inventory: &str) -> Value {
    serde_json::to_value(
        Lexicon::parse(inventory)
            .unwrap()
            .check_text(text, "fixture.md"),
    )
    .unwrap()
}
fn located(groups: &Value) -> Vec<Value> {
    groups.as_array().unwrap().iter().flat_map(|group| {
        group["matches"].as_array().unwrap().iter().map(move |hit| {
            json!({"line": group["line"], "column": hit["column"], "span": hit["span"], "category": hit["category"]})
        })
    }).collect()
}
#[test]
fn retained_independent_lexical_expectations() {
    let cases: Value = serde_json::from_str(include_str!("evaluation.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let actual = located(&scan(case["text"].as_str().unwrap(), DEFAULT_LEXICON));
        let mut actual: Vec<_> = actual.iter().map(Value::to_string).collect();
        let mut expected: Vec<_> = case["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::to_string)
            .collect();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "{}", case["name"]);
    }
}
#[test]
fn every_configured_marker_case_repetition_and_boundaries() {
    let data: Value = serde_json::from_str(DEFAULT_LEXICON).unwrap();
    let lexicon = Lexicon::parse(DEFAULT_LEXICON).unwrap();
    for (category, markers) in data.as_object().unwrap() {
        for marker in markers.as_array().unwrap() {
            let marker = marker.as_str().unwrap();
            let groups =
                lexicon.check_text(&format!("({marker}); {}!", marker.to_uppercase()), "x");
            let hits: Vec<_> = groups
                .iter()
                .flat_map(|g| &g.matches)
                .filter(|h| h.category == *category && h.marker == marker)
                .collect();
            assert_eq!(
                hits.iter()
                    .map(|h| (h.column, h.span.as_str()))
                    .collect::<Vec<_>>(),
                vec![
                    (2, marker),
                    (marker.chars().count() + 5, marker.to_uppercase().as_str())
                ]
            );
            let groups = lexicon.check_text(
                &format!("x{marker} {marker}x _{marker} {marker}_ π{marker} {marker}²"),
                "x",
            );
            assert!(
                !groups
                    .iter()
                    .flat_map(|g| &g.matches)
                    .any(|h| h.category == *category && h.marker == marker),
                "{marker}"
            );
        }
    }
}
#[test]
fn literal_phrases_self_overlap_transitivity_and_adjacent_groups() {
    let groups = scan(
        "A.B acb go\tgo go go x-y",
        r#"{"z":["a.b","go go","x","y"],"a":["go"]}"#,
    );
    assert_eq!(groups.as_array().unwrap().len(), 4);
    assert_eq!(groups[0]["span"], "A.B");
    assert_eq!(groups[1]["span"], "go\tgo go go");
    assert_eq!(groups[1]["matches"].as_array().unwrap().len(), 7);
    assert_eq!(groups[1]["matches"][0]["category"], "a");
    assert_eq!(groups[1]["matches"][1]["column"], 9);
    assert_eq!(groups[2]["span"], "x");
    assert_eq!(groups[3]["span"], "y");
    let adjacent = scan("a-b", r#"{"x":["a","b"]}"#);
    assert_eq!(adjacent.as_array().unwrap().len(), 2);
}
#[test]
fn unicode_scalar_columns_exact_context_and_python_line_breaks() {
    let text = "\u{feff}😀e\u{301}\tNOT necessarily valid; but useful.";
    let groups = scan(text, DEFAULT_LEXICON);
    assert_eq!(groups[0]["column"], 6);
    assert_eq!(groups[0]["span"], "NOT necessarily");
    assert_eq!(groups[0]["context"], text);
    assert_eq!(groups[0]["prompt"], PROMPT);
    assert_eq!(groups[0]["matches"].as_array().unwrap().len(), 2);
    for separator in [
        "\n", "\r", "\r\n", "\u{b}", "\u{c}", "\u{1c}", "\u{1d}", "\u{1e}", "\u{85}", "\u{2028}",
        "\u{2029}",
    ] {
        let groups = scan(
            &format!("{separator}not{separator}no{separator}"),
            DEFAULT_LEXICON,
        );
        assert_eq!(groups[0]["line"], 2, "{separator:?}");
        assert_eq!(groups[1]["line"], 3, "{separator:?}");
    }
}
#[test]
fn unicode_boundaries_are_letters_numbers_underscore_not_marks() {
    let groups = scan(
        "πnot not² _not not_ not\u{301} \u{301}not",
        r#"{"x":["not"]}"#,
    );
    assert_eq!(groups.as_array().unwrap().len(), 2);
    assert_eq!(groups[0]["span"], "not");
    assert_eq!(
        scan("İS lıKely ſlightly", DEFAULT_LEXICON)
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(scan("İS", r#"{"x":["is"]}"#)[0]["span"], "İS");
}
#[test]
fn phrase_whitespace_does_not_cross_lines_or_non_ascii_spaces() {
    let inventory = r#"{"x":["hold back"]}"#;
    let groups = scan(
        "hold \t back hold\nback hold\u{a0}back hold\u{b}back",
        inventory,
    );
    assert_eq!(groups.as_array().unwrap().len(), 1);
    assert_eq!(groups[0]["span"], "hold \t back");
}
#[test]
fn raw_markdown_and_legitimate_qualifications_are_not_suppressed() {
    let groups = scan(
        "`not`\n> no\nAlthough exact, only two samples remain.\nThe result may hold.",
        DEFAULT_LEXICON,
    );
    assert_eq!(
        located(&groups),
        vec![
            json!({"line":1,"column":2,"span":"not","category":"negation"}),
            json!({"line":2,"column":3,"span":"no","category":"negation"}),
            json!({"line":3,"column":1,"span":"Although","category":"contrast/concession"}),
            json!({"line":3,"column":17,"span":"only","category":"minimization"}),
            json!({"line":4,"column":12,"span":"may","category":"hedging/modality"}),
        ]
    );
}
#[test]
fn malformed_lexicons_and_duplicate_matching_equivalents_rejected() {
    for invalid in [
        "{",
        "[]",
        "{}",
        r#"{"": ["not"]}"#,
        r#"{"x": []}"#,
        r#"{"x":"not"}"#,
        r#"{"x":[null]}"#,
        r#"{"x":[""]}"#,
        r#"{"x":[" not"]}"#,
        r#"{"x":["a  b"]}"#,
        r#"{"x":["a\tb"]}"#,
        r#"{"x":["a\u001fb"]}"#,
        r#"{"x":["a-"]}"#,
        r#"{"x":["not","NOT"]}"#,
        r#"{"x":["is","İS"]}"#,
        r#"{"x":["\ud800"]}"#,
    ] {
        assert!(Lexicon::parse(invalid).is_err(), "{invalid}");
    }
    // Full-fold expansions are not matching equivalences (documented difference).
    assert!(Lexicon::parse(r#"{"x":["ss","ß"]}"#).is_ok());
    assert_eq!(
        scan("not", r#"{"a":["not"],"b":["not"]}"#)[0]["matches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn empty_and_unconfigured_text_produces_no_quality_verdict() {
    for text in [
        "",
        "\n",
        "The sensor records six distinct pulses.",
        "can or appear",
    ] {
        assert!(
            Lexicon::parse(DEFAULT_LEXICON)
                .unwrap()
                .check_text(text, "x")
                .is_empty()
        );
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "pira-write-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, content).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pira_write_check"))
        .args(args)
        .output()
        .unwrap()
}
fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn direct_help_is_structured_and_embeds_separate_guidance() {
    for option in ["--help", "-h"] {
        let out = cli(&[option]);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            text,
            format!(
                "pira_write_check {VERSION}\n\n{}",
                include_str!("../../../src/pira_write_check/help.md")
            )
        );
        // Heading extraction accepts checkout newlines; emitted-byte equality stays exact above.
        let normalized = text.replace("\r\n", "\n");
        let mut remainder = normalized.as_str();
        for section in [
            "USAGE",
            "OPTIONS",
            "INPUT AND REVIEW",
            "EXAMPLES",
            "EXIT STATUS",
        ] {
            remainder = remainder.split_once(&format!("\n{section}\n")).unwrap().1;
        }
        for required in [
            "--json",
            "--lexicon FILE",
            "--lexicon=FILE",
            "-h, --help",
            "-V, --version",
            "default: text report",
            "default: embedded English lexicon",
            "regular UTF-8 files",
            "Unicode scalar columns",
            "raw source",
            "No stdin",
            "rewriting",
            ADVISORY,
        ] {
            assert!(text.contains(required), "missing help contract: {required}");
        }
    }
}
#[test]
fn direct_version_uses_package_version() {
    for option in ["--version", "-V"] {
        let out = cli(&[option]);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert_eq!(
            out.stdout,
            format!("pira_write_check {VERSION}\n").as_bytes()
        );
    }
}
#[test]
fn usage_errors_keep_stderr_and_exit_two() {
    for (args, reason) in [
        (vec![], "at least one explicit FILE is required"),
        (vec!["--unknown"], "unknown option: --unknown"),
        (vec!["--lexicon"], "--lexicon requires a file"),
        (vec!["--json"], "at least one explicit FILE is required"),
        (vec!["--unknown", "--help"], "unknown option: --unknown"),
        (
            vec!["--lexicon", "--help"],
            "at least one explicit FILE is required",
        ),
    ] {
        let out = cli(&args);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        let error = String::from_utf8(out.stderr).unwrap();
        assert!(error.contains(reason), "{error}");
        assert!(error.contains("Usage:"));
    }
}
#[test]
fn help_version_short_circuit_in_encounter_order() {
    let help = cli(&["--help"]).stdout;
    let version = cli(&["--version"]).stdout;
    for (args, expected) in [
        (vec!["--help", "--unknown"], &help),
        (vec!["--help", "--version"], &help),
        (vec!["--version", "--help"], &version),
        (vec!["--json", "--help"], &help),
        (vec!["--lexicon=", "--help"], &help),
    ] {
        let out = cli(&args);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert_eq!(&out.stdout, expected);
    }
    let tmp = Fixture::new();
    for (option, expected) in [("-h", &help), ("-V", &version)] {
        let out = cli(&[arg(&tmp.0.join("missing")), option]);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert_eq!(&out.stdout, expected);
    }
}
#[test]
fn delimiter_and_lexicon_values_preserve_help_and_version_filenames() {
    let tmp = Fixture::new();
    tmp.file("draft.md", b"sensor");
    tmp.file("custom.json", br#"{"custom":["sensor"]}"#);
    for name in ["--help", "-h", "--version", "-V", "--"] {
        tmp.file(name, b"not");
        let out = Command::new(env!("CARGO_BIN_EXE_pira_write_check"))
            .current_dir(&tmp.0)
            .args(["--json", "--", name])
            .output()
            .unwrap();
        assert!(out.status.success(), "{name}");
        assert_eq!(report(&out)["warnings"][0]["path"], name);
        assert_eq!(report(&out)["warnings"][0]["span"], "not");

        tmp.file(name, br#"{"named":["sensor"]}"#);
        let out = Command::new(env!("CARGO_BIN_EXE_pira_write_check"))
            .current_dir(&tmp.0)
            .args([
                "draft.md",
                "--json",
                "--lexicon",
                "custom.json",
                "--lexicon",
                name,
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "{name}");
        assert_eq!(
            report(&out)["warnings"][0]["matches"][0]["category"],
            "named"
        );
    }
}
#[test]
fn documented_examples_execute_with_default_and_custom_markers() {
    let tmp = Fixture::new();
    tmp.file("draft.md", b"not sensor");
    tmp.file("notes.txt", b"perhaps");
    tmp.file("-draft.md", b"not");
    tmp.file("custom.json", br#"{"custom":["sensor"]}"#);
    let help = String::from_utf8(cli(&["--help"]).stdout).unwrap();
    let help = help.replace("\r\n", "\n");
    let examples = help
        .split_once("\nEXAMPLES\n")
        .unwrap()
        .1
        .split_once("\nEXIT STATUS\n")
        .unwrap()
        .0;
    for line in examples.lines().filter(|line| !line.trim().is_empty()) {
        let args: Vec<_> = line
            .trim()
            .strip_prefix("pira_write_check ")
            .unwrap()
            .split_whitespace()
            .collect();
        let out = Command::new(env!("CARGO_BIN_EXE_pira_write_check"))
            .current_dir(&tmp.0)
            .args(&args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{line}");
        assert!(out.stderr.is_empty());
        if args.contains(&"--json") {
            assert_eq!(report(&out)["errors"], json!([]));
            assert_eq!(report(&out)["warnings"][0]["span"], "not");
            assert_eq!(report(&out)["warnings"].as_array().unwrap().len(), 1);
        } else {
            let text = String::from_utf8(out.stdout).unwrap();
            let count = if args.contains(&"notes.txt") { 2 } else { 1 };
            assert!(text.contains(&format!(
                "{count} marker occurrence(s), {count} review group(s), 0 input error(s)."
            )));
            let marker = if args.iter().any(|arg| arg.starts_with("--lexicon")) {
                "sensor"
            } else {
                "not"
            };
            assert!(
                text.contains(&format!("(marker: \"{marker}\")")),
                "{line}: {text}"
            );
        }
    }
}
#[test]
fn default_text_clean_scan_is_advisory_and_exits_zero() {
    let tmp = Fixture::new();
    let source = tmp.file("clean.md", b"The sensor records six distinct pulses.");
    let out = cli(&[arg(&source)]);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("0 marker occurrence(s), 0 review group(s), 0 input error(s). {ADVISORY}\n")
    );
}
#[test]
fn cli_json_exact_expectations_determinism_and_unchanged_files() {
    let tmp = Fixture::new();
    let path = tmp.file(
        "α file.md",
        "😀\tNo exception, but it may fail.\r\n".as_bytes(),
    );
    let before = (
        fs::read(&path).unwrap(),
        fs::metadata(&path).unwrap().modified().unwrap(),
    );
    let out = cli(&["--json", arg(&path)]);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    let data = report(&out);
    assert_eq!(data["advisory"], ADVISORY);
    assert_eq!(data["errors"], json!([]));
    assert_eq!(
        located(&data["warnings"]),
        vec![
            json!({"line":1,"column":3,"span":"No","category":"negation"}),
            json!({"line":1,"column":17,"span":"but","category":"contrast/concession"}),
            json!({"line":1,"column":24,"span":"may","category":"hedging/modality"}),
        ]
    );
    assert_eq!(data["warnings"][0]["path"], arg(&path));
    assert_eq!(out.stdout, cli(&["--json", arg(&path)]).stdout);
    assert_eq!(
        before,
        (
            fs::read(&path).unwrap(),
            fs::metadata(&path).unwrap().modified().unwrap()
        )
    );
}
#[test]
fn cli_text_escapes_context_and_reports_every_occurrence() {
    let tmp = Fixture::new();
    let path = tmp.file("source.txt", b"no no no\t\"safe\"");
    let lex = tmp.file("lex.json", br#"{"x":["no no"]}"#);
    let out = cli(&["--lexicon", arg(&lex), arg(&path)]);
    assert!(out.status.success());
    let out = String::from_utf8(out.stdout).unwrap();
    assert!(out.contains(":1:1: \"no no no\":"));
    assert!(out.contains("column 4 [x] \"no no\""));
    assert!(out.contains("context: \"no no no\\t\\\"safe\\\"\""));
    assert!(out.contains("2 marker occurrence(s), 1 review group(s), 0 input error(s)."));
}
#[test]
fn cli_mixed_file_errors_retain_success_and_argument_order() {
    let tmp = Fixture::new();
    let missing = tmp.0.join("missing");
    let bad = tmp.file("bad", b"\xff");
    let good = tmp.file("good", b"perhaps");
    let out = cli(&[
        "--json",
        arg(&missing),
        arg(&bad),
        arg(&tmp.0),
        arg(&good),
        arg(&good),
    ]);
    assert_eq!(out.status.code(), Some(2));
    let data = report(&out);
    let errors = data["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 3);
    assert_eq!(
        errors
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![arg(&missing), arg(&bad), arg(&tmp.0)]
    );
    assert_eq!(data["warnings"].as_array().unwrap().len(), 2);
    assert_eq!(data["warnings"][0]["span"], "perhaps");
}
#[test]
fn cli_invalid_lexicon_stops_scan_and_is_reported_in_selected_format() {
    let tmp = Fixture::new();
    let good = tmp.file("good", b"not");
    for bytes in [b"\xff".as_slice(), b"{}", b"{", br#"{"x":[null]}"#] {
        let lex = tmp.file("bad.json", bytes);
        let out = cli(&["--json", "--lexicon", arg(&lex), arg(&good)]);
        assert_eq!(out.status.code(), Some(2));
        let data = report(&out);
        assert_eq!(data["warnings"], json!([]));
        assert_eq!(data["errors"][0]["path"], arg(&lex));
    }
    let out = cli(&["--lexicon", arg(&tmp.0.join("missing")), arg(&good)]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("input error:")
    );
}
#[test]
fn cli_custom_literals_option_equals_and_end_of_options() {
    let tmp = Fixture::new();
    let lex = tmp.file("lex", br#"{"custom":["a[b]c"]}"#);
    let good = tmp.file("good", b"A[B]C abc");
    let out = cli(&["--json", &format!("--lexicon={}", arg(&lex)), arg(&good)]);
    assert!(out.status.success());
    assert_eq!(report(&out)["warnings"][0]["span"], "A[B]C");
    let source = tmp.file("--json", b"not");
    let out = Command::new(env!("CARGO_BIN_EXE_pira_write_check"))
        .current_dir(&tmp.0)
        .args(["--json", "--", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(report(&out)["warnings"][0]["path"], "--json");
    assert!(source.exists());
}
#[cfg(unix)]
#[test]
fn explicit_symlinks_read_regular_targets_without_rewriting() {
    let tmp = Fixture::new();
    let good = tmp.file("target", b"without");
    let link = tmp.0.join("link");
    std::os::unix::fs::symlink(&good, &link).unwrap();
    let out = cli(&["--json", arg(&link)]);
    assert!(out.status.success());
    assert_eq!(report(&out)["warnings"][0]["path"], arg(&link));
    assert_eq!(fs::read(&good).unwrap(), b"without");
}
#[cfg(unix)]
#[test]
fn malformed_unicode_arguments_are_usage_errors() {
    use std::os::unix::ffi::OsStrExt;
    let out = Command::new(env!("CARGO_BIN_EXE_pira_write_check"))
        .arg(std::ffi::OsStr::from_bytes(b"bad\xff"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("valid Unicode")
    );
}
