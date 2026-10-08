#[cfg(unix)]
#[test]
fn non_unicode_interpreter_never_launches_replacement_sibling() {
    let output = std::process::Command::new("python3")
        .args([
            "-c",
            include_str!("interpreter_alias_probe.py"),
            env!("CARGO_BIN_EXE_pira_ctx"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
