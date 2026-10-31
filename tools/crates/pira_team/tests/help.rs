use std::process::{Command, Output};

fn invoke(args: &[&str]) -> Output {
    // Help must work before any session/store/backend/authentication discovery.
    Command::new(env!("CARGO_BIN_EXE_pira_team"))
        .env_clear()
        .env("HOME", "")
        .env("USERPROFILE", "")
        .env("LOCALAPPDATA", "")
        .env("CODEX_HOME", "missing-team-help-session")
        .env("CODEX_THREAD_ID", "invalid/session")
        .env("PIRA_TEAM_DIR", "")
        .env("PIRA_TEAM_BUILD_ROOTS", "invalid JSON")
        .env("PATH", "")
        .args(args)
        .output()
        .unwrap()
}

fn success(args: &[&str]) -> String {
    let output = invoke(args);
    assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn overview_is_concise_and_all_spellings_agree() {
    let overview = success(&[]);
    for args in [["help"], ["--help"], ["-h"]] {
        assert_eq!(success(&args), overview);
    }
    for args in [["help", "help"], ["help", "--help"], ["help", "-h"]] {
        assert_eq!(success(&args), overview);
    }
    for heading in [
        "USAGE",
        "COMMANDS",
        "OPTIONS",
        "TOPICS",
        "EXAMPLES",
        "EXIT STATUS",
    ] {
        assert!(overview.contains(heading), "{heading}");
    }
    assert!(overview.lines().count() < 70);
    assert!(!overview.contains("allowed-signers"));
    for topic in ["safety", "recovery", "sessions", "remote", "manual"] {
        assert!(overview.contains(topic), "{topic}");
    }
}

#[test]
fn supported_commands_have_focused_help_in_each_explicit_form() {
    for command in [
        "run",
        "resume",
        "steer",
        "interrupt",
        "tier",
        "read",
        "path",
        "wait",
        "bookmark",
        "unbookmark",
        "bookmarks",
        "search",
        "config",
        "resolve",
        "remote-keygen",
        "remote-check",
        "attach",
    ] {
        let help = success(&["help", command]);
        assert!(help.contains("USAGE"), "{command}");
        assert!(help.contains("EXAMPLES"), "{command}");
        assert!(help.contains("EXIT STATUS"), "{command}");
        for args in [
            vec![command, "--help"],
            vec![command, "-h"],
            vec!["--help", command],
            vec!["-h", command],
        ] {
            assert_eq!(success(&args), help, "{args:?}");
        }
        if !matches!(command, "remote-keygen" | "remote-check" | "attach") {
            assert!(!help.contains("allowed-signers"), "{command}");
        }
    }
}

#[test]
fn config_subcommands_share_the_exact_config_contract() {
    let help = success(&["config", "--help"]);
    for action in ["show", "set", "reset"] {
        for args in [
            vec!["help", "config", action],
            vec!["config", action, "--help"],
            vec!["config", action, "-h"],
            vec!["--help", "config", action],
            vec!["-h", "config", action],
        ] {
            assert_eq!(success(&args), help);
        }
    }
    assert!(help.contains("Set requires both model"));
    assert!(help.contains("worker_defaults.json"));
}

#[test]
fn unknown_topics_and_extra_help_arguments_fail_without_normal_dispatch() {
    for (args, expected) in [
        (vec!["help", "no-such-topic"], "unknown help topic"),
        (vec!["--help", "no-such-topic"], "unknown help topic"),
        (vec!["-h", "no-such-topic"], "unknown help topic"),
        (vec!["no-such-topic", "--help"], "unknown help topic"),
        (
            vec!["help", "config", "no-such-action"],
            "unknown help topic",
        ),
        (vec!["config", "no-such-action", "-h"], "unknown help topic"),
        (vec!["help", "run", "extra"], "help accepts one topic"),
        (
            vec!["help", "config", "set", "extra"],
            "help accepts one topic",
        ),
        (
            vec!["--help-all", "extra"],
            "--help-all does not accept arguments",
        ),
    ] {
        let output = invoke(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.starts_with("pira_team: "), "{error}");
        assert!(error.contains(expected), "{args:?}: {error}");
        assert!(error.contains("pira_team"));
    }
}

#[test]
fn version_uses_package_metadata_without_discovery() {
    assert_eq!(
        success(&["--version"]),
        format!("pira_team {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn manual_preserves_full_reference_and_remote_recovery_interfaces() {
    let manual = success(&["help", "manual"]);
    assert_eq!(
        manual,
        include_str!("../../../src/pira_team/help_manual.md")
    );
    for args in [
        vec!["help", "all"],
        vec!["help", "--all"],
        vec!["--help-all"],
    ] {
        assert_eq!(success(&args), manual);
    }
    for needle in [
        "--request-id",
        "--run-request",
        "--operation-id",
        "remote-check",
        "remote-keygen",
        "attach REMOTE",
        "--artifact",
        "--file",
        "--detach",
        "allowed-signers",
        "remote-endpoint.json",
        "failed_or_unknown",
        "No persisted PID/group",
        "exclusive owner lock",
        "same main-session-scoped",
        "--inject-write",
        "--allow-subteam",
        "PIRA_TEAM_BUILD_ROOTS",
    ] {
        assert!(manual.contains(needle), "{needle}");
    }
    for topic in ["remote", "recovery", "safety", "sessions"] {
        let focused = success(&["help", topic]);
        assert!(focused.len() < manual.len());
        assert!(focused.contains("help manual"), "{topic}");
    }
}

#[test]
fn documented_defaults_and_key_error_contracts_are_visible() {
    let run = success(&["run", "--help"]);
    for needle in [
        "Default artifact",
        "Default auto",
        "default to standard",
        "--timeout is rejected",
        "--schema",
        "--format json",
        "--columns",
        "--format csv",
        "--task-file",
        "--code-review",
        "--allow-fix",
        "--navigation",
        "deprecated",
        "--host",
    ] {
        assert!(run.contains(needle), "{needle}");
    }
    assert!(success(&["resume", "--help"]).contains("retains its gate"));
    assert!(success(&["bookmarks", "--help"]).contains("1..1000 (default 20)"));
    assert!(success(&["search", "--help"]).contains("Only labels are searched"));
    assert!(success(&["tier", "--help"]).contains("Remote live tier is unsupported"));
}
