use std::process::Command;

fn check(mode: &str) {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let output = Command::new(python)
        .args([
            "-c",
            include_str!("native_cwd_probe.py"),
            env!("CARGO_BIN_EXE_pira_ctx"),
            mode,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ordinary_and_ambiguous_legacy_cwd_remain_truthful_and_safe() {
    check("legacy");
}

// macOS sandbox filesystems may deny invalid-UTF-8 names; Linux validates real execution.
#[cfg(target_os = "linux")]
#[test]
fn native_cwd_never_executes_in_existing_replacement_sibling() {
    check("native");
}

#[cfg(unix)]
#[test]
fn live_cwd_reconstruction_across_workspaces() {
    check("live");
}
