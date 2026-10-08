#[test]
fn defaults_overrides_and_legacy_discovery_use_only_disposable_stores() {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let output = std::process::Command::new(python)
        .args([
            "-c",
            include_str!("store_defaults_probe.py"),
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
