#[test]
fn caller_visible_contracts() {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let output = std::process::Command::new(python)
        .args([
            "-B",
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/interface_contracts.py"),
        ])
        .arg(env!("CARGO_BIN_EXE_pira_ctx"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
